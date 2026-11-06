//! Shrinking a PDF by re-encoding its images as JPEG.
//!
//! Only images a JPEG round trip cannot damage are touched: deflated, eight
//! bits, grey or RGB, without a mask, and large enough to be worth it. Images
//! already in a lossy format would only lose more, CMYK does not survive the
//! trip, and JPEG has no transparency. An image that does not end up smaller
//! is left as it was.

use ::lopdf::{Document as Pdf, Object, ObjectId, Stream};
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;

use super::Deadline;
use crate::error::Result;

/// Smaller images are not worth a lossy pass.
const MIN_BYTES: usize = 30 << 10;
/// Pixels decoded for one image, so a small deflated stream cannot claim more
/// memory than a large page scan would.
const MAX_PIXELS: u64 = 200_000_000;

/// Re-encodes what qualifies at `quality` and returns how many images changed.
pub(super) fn reencode(pdf: &mut Pdf, quality: u8, deadline: Deadline) -> Result<usize> {
    let ids: Vec<ObjectId> = pdf
        .objects
        .iter()
        .filter(|(_, object)| matches!(object, Object::Stream(s) if is_image(s)))
        .map(|(id, _)| *id)
        .collect();
    let mut changed = 0;
    for id in ids {
        deadline.check()?;
        let Some(jpeg) = pdf
            .objects
            .get(&id)
            .and_then(|object| object.as_stream().ok())
            .and_then(|stream| jpeg_of(pdf, stream, quality))
        else {
            continue;
        };
        if let Some(Object::Stream(stream)) = pdf.objects.get_mut(&id) {
            stream
                .dict
                .set("Filter", Object::Name(b"DCTDecode".to_vec()));
            stream.dict.remove(b"DecodeParms");
            stream.set_content(jpeg);
            stream.allows_compression = false;
            changed += 1;
        }
    }
    Ok(changed)
}

fn is_image(stream: &Stream) -> bool {
    stream
        .dict
        .get(b"Subtype")
        .and_then(Object::as_name)
        .is_ok_and(|s| s == b"Image")
}

/// The name of the only filter, when there is exactly one.
fn only_filter(stream: &Stream) -> Option<&[u8]> {
    match stream.dict.get(b"Filter").ok()? {
        Object::Name(name) => Some(name),
        Object::Array(list) => match list.as_slice() {
            [Object::Name(name)] => Some(name),
            _ => None,
        },
        _ => None,
    }
}

/// Components per pixel of a colour space JPEG can carry unchanged.
fn components(pdf: &Pdf, space: &Object) -> Option<u32> {
    let (_, space) = pdf.dereference(space).ok()?;
    match space {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" | b"CalGray" => Some(1),
            b"DeviceRGB" | b"CalRGB" => Some(3),
            _ => None,
        },
        Object::Array(list) => {
            let family = list.first()?.as_name().ok()?;
            match family {
                b"CalGray" => Some(1),
                b"CalRGB" => Some(3),
                b"ICCBased" => {
                    let (_, profile) = pdf.dereference(list.get(1)?).ok()?;
                    let n = profile
                        .as_stream()
                        .ok()?
                        .dict
                        .get(b"N")
                        .ok()?
                        .as_i64()
                        .ok()?;
                    matches!(n, 1 | 3).then_some(n as u32)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The image as JPEG, when it qualifies and comes out smaller.
fn jpeg_of(pdf: &Pdf, stream: &Stream, quality: u8) -> Option<Vec<u8>> {
    if stream.content.len() < MIN_BYTES || only_filter(stream)? != b"FlateDecode" {
        return None;
    }
    let dict = &stream.dict;
    let masked = [&b"SMask"[..], b"Mask", b"SMaskInData"]
        .iter()
        .any(|key| dict.has(key))
        || dict
            .get(b"ImageMask")
            .and_then(Object::as_bool)
            .unwrap_or(false);
    if masked || dict.get(b"BitsPerComponent").ok()?.as_i64().ok()? != 8 {
        return None;
    }
    let n = components(pdf, dict.get(b"ColorSpace").ok()?)?;
    let width = u32::try_from(dict.get(b"Width").ok()?.as_i64().ok()?).ok()?;
    let height = u32::try_from(dict.get(b"Height").ok()?.as_i64().ok()?).ok()?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    let pixels = stream.decompressed_content().ok()?;
    let expected = u64::from(width) * u64::from(height) * u64::from(n);
    if pixels.len() as u64 != expected {
        return None;
    }
    let color = if n == 1 {
        ExtendedColorType::L8
    } else {
        ExtendedColorType::Rgb8
    };
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, quality.clamp(1, 100))
        .encode(&pixels, width, height, color)
        .ok()?;
    (jpeg.len() < stream.content.len()).then_some(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::lopdf::dictionary;
    use std::time::Duration;

    fn image_pdf(extra: Option<&str>) -> Pdf {
        // A smooth gradient deflates badly enough to be worth re-encoding.
        let (w, h) = (400i64, 400i64);
        let mut pixels = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let noise = ((x * 7919 + y * 104_729) % 251) as u8;
                pixels.extend_from_slice(&[noise, (x % 256) as u8, (y % 256) as u8]);
            }
        }
        let mut dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w,
            "Height" => h,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
        };
        if let Some(key) = extra {
            dict.set(key.as_bytes().to_vec(), Object::Boolean(true));
        }
        let mut stream = Stream::new(dict, pixels);
        stream.compress().unwrap();
        let mut pdf = Pdf::with_version("1.7");
        pdf.add_object(stream);
        pdf
    }

    #[test]
    fn deflated_images_become_smaller_jpegs() {
        let mut pdf = image_pdf(None);
        let before: usize = pdf
            .objects
            .values()
            .filter_map(|o| o.as_stream().ok())
            .map(|s| s.content.len())
            .sum();
        let deadline = Deadline::new(Duration::from_secs(30));
        assert_eq!(reencode(&mut pdf, 40, deadline).unwrap(), 1);
        let stream = pdf
            .objects
            .values()
            .find_map(|o| o.as_stream().ok())
            .unwrap();
        assert_eq!(only_filter(stream), Some(&b"DCTDecode"[..]));
        assert!(stream.content.len() < before);
        assert!(stream.content.starts_with(&[0xff, 0xd8]));
        // A second pass finds nothing left to do.
        assert_eq!(reencode(&mut pdf, 40, deadline).unwrap(), 0);
    }

    #[test]
    fn masked_images_stay() {
        let mut pdf = image_pdf(Some("SMask"));
        let deadline = Deadline::new(Duration::from_secs(30));
        assert_eq!(reencode(&mut pdf, 40, deadline).unwrap(), 0);
    }
}
