//! Image values: PNG, JPEG and GIF as they are, WebP turned into PNG, and
//! SVG rebuilt without anything that reaches out, with a PNG beside it.

use std::io::Cursor;

use base64::alphabet::STANDARD;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine;
use image::codecs::png::PngEncoder;
use image::{ImageEncoder, ImageFormat, ImageReader, Limits};
use minijinja::value::{Value, ValueKind};
use sha2::{Digest, Sha256};

use crate::rich::svg;

/// The most bytes one image may hold.
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// The most images and barcodes one render may place.
pub const MAX_IMAGES: usize = 200;
/// The page text width that caps an image's natural size.
pub const TEXT_WIDTH_PT: f64 = 450.0;
/// The longest side accepted, in points (200 inches).
pub const MAX_SIDE_PT: f64 = 14400.0;
/// A pixel at the 96 dpi images are assumed to have.
const PT_PER_PX: f64 = 0.75;
/// The most pixels a WebP image may decode to.
const MAX_WEBP_PIXELS: u64 = 25_000_000;
/// The longest side of a WebP image, in pixels.
const MAX_WEBP_SIDE: u32 = 16_384;

/// Verified image bytes.
#[derive(Debug)]
pub struct Media {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    pub mime: &'static str,
    pub hash: [u8; 32],
    /// The natural size in pixels at 96 dpi.
    pub px: (u32, u32),
    /// The size an office suite gives the file when it honours the
    /// resolution the file records, in points.
    pub physical_pt: (f64, f64),
    /// For SVG, a PNG of the same drawing for readers without SVG.
    pub fallback: Option<Box<Media>>,
}

impl Media {
    /// The bytes it adds to a package, its fallback included.
    pub fn stored_bytes(&self) -> u64 {
        self.bytes.len() as u64 + self.fallback.as_ref().map_or(0, |f| f.bytes.len() as u64)
    }

    /// Width over height.
    pub fn aspect(&self) -> f64 {
        self.physical_pt.0 / self.physical_pt.1
    }
}

/// What a value of the `image` filter carries.
#[derive(Debug, Default)]
pub struct Input {
    pub bytes: Vec<u8>,
    pub mime: Option<String>,
    pub width: Option<f64>,
    pub height: Option<f64>,
}

pub(crate) const BASE64: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

fn unbase64(text: &str) -> Result<Vec<u8>, String> {
    if text.len() / 4 * 3 > MAX_IMAGE_BYTES + 3 {
        return Err(format!("the image is larger than {MAX_IMAGE_BYTES} bytes"));
    }
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    BASE64
        .decode(compact)
        .map_err(|e| format!("the value is not valid base64: {e}"))
}

fn from_text(text: &str) -> Result<Input, String> {
    let text = text.trim();
    let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https:/") {
        return Err("the value must carry the image's bytes: URLs are not fetched".into());
    }
    if !lower.starts_with("data:") {
        return Ok(Input {
            bytes: unbase64(text)?,
            ..Input::default()
        });
    }
    let (head, body) = text
        .split_once(',')
        .ok_or("a data URI needs a comma before its data")?;
    let head = head["data:".len()..].to_ascii_lowercase();
    let (mime, encoding) = head.split_once(';').unwrap_or((&head, ""));
    if !encoding.split(';').any(|p| p.trim() == "base64") {
        return Err("a data URI must be base64 encoded".into());
    }
    Ok(Input {
        bytes: unbase64(body)?,
        mime: (!mime.is_empty()).then(|| mime.to_string()),
        ..Input::default()
    })
}

fn side(object: &Value, key: &str) -> Result<Option<f64>, String> {
    let v = object.get_attr(key).map_err(|e| e.to_string())?;
    if v.is_undefined() || v.is_none() {
        return Ok(None);
    }
    let n = v
        .is_number()
        .then(|| f64::try_from(v).ok())
        .flatten()
        .filter(|n| n.is_finite() && *n > 0.0 && *n <= MAX_SIDE_PT)
        .ok_or_else(|| format!("{key} must be a number of points up to {MAX_SIDE_PT}"))?;
    Ok(Some(n))
}

/// Reads a data URI, raw base64, or a `{data, mime, width, height}` object.
pub fn decode(value: &Value) -> Result<Input, String> {
    if let Some(text) = value.as_str() {
        return from_text(text);
    }
    if value.kind() != ValueKind::Map {
        return Err(format!(
            "expected a data URI, base64 or an object with data, got {}",
            value.kind()
        ));
    }
    let data = value.get_attr("data").map_err(|e| e.to_string())?;
    let data = data.as_str().ok_or("the object's data must be a string")?;
    let mut input = from_text(data)?;
    let mime = value.get_attr("mime").map_err(|e| e.to_string())?;
    if let Some(mime) = mime.as_str() {
        input.mime = Some(mime.trim().to_ascii_lowercase());
    }
    input.width = side(value, "width")?;
    input.height = side(value, "height")?;
    Ok(input)
}

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

fn same_mime(declared: &str, actual: &str) -> bool {
    declared == actual
        || (actual == "image/jpeg" && declared == "image/jpg")
        || (actual == "image/svg+xml" && declared == "image/svg")
}

/// Verifies the magic bytes and reads the dimensions from the header. PNG,
/// JPEG and GIF pixels are never decoded, so a small file claiming huge
/// dimensions costs nothing. WebP is decoded within limits and stored as
/// PNG; SVG is rebuilt and drawn once for its PNG.
pub fn inspect(bytes: Vec<u8>, declared: Option<&str>) -> Result<Media, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!("the image is larger than {MAX_IMAGE_BYTES} bytes"));
    }
    let (format, ext, mime) = if bytes.starts_with(&PNG_MAGIC) {
        (ImageFormat::Png, "png", "image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        (ImageFormat::Jpeg, "jpeg", "image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        (ImageFormat::Gif, "gif", "image/gif")
    } else if is_webp(&bytes) {
        if let Some(declared) = declared.filter(|d| *d != "image/webp") {
            return Err(format!(
                "the bytes are image/webp but the value says {declared}"
            ));
        }
        return webp(&bytes);
    } else if svg::sniff(&bytes) {
        if let Some(declared) = declared.filter(|d| !same_mime(d, "image/svg+xml")) {
            return Err(format!(
                "the bytes are image/svg+xml but the value says {declared}"
            ));
        }
        return from_svg(&bytes);
    } else {
        return Err("only PNG, JPEG, GIF, SVG and WebP images are supported".into());
    };
    if let Some(declared) = declared {
        if !same_mime(declared, mime) {
            return Err(format!(
                "the bytes are {mime} but the value says {declared}"
            ));
        }
    }
    let px = ImageReader::with_format(Cursor::new(&bytes), format)
        .into_dimensions()
        .map_err(|e| format!("cannot read the image's dimensions: {e}"))?;
    if px.0 == 0 || px.1 == 0 {
        return Err("the image has no pixels".into());
    }
    let dpi = match format {
        ImageFormat::Png => png_dpi(&bytes),
        ImageFormat::Jpeg => jpeg_dpi(&bytes),
        _ => None,
    }
    .unwrap_or((96.0, 96.0));
    Ok(raster(bytes, ext, mime, px, dpi))
}

fn raster(
    bytes: Vec<u8>,
    ext: &'static str,
    mime: &'static str,
    px: (u32, u32),
    dpi: (f64, f64),
) -> Media {
    let hash = Sha256::digest(&bytes).into();
    Media {
        bytes,
        ext,
        mime,
        hash,
        px,
        physical_pt: (
            f64::from(px.0) * 72.0 / dpi.0,
            f64::from(px.1) * 72.0 / dpi.1,
        ),
        fallback: None,
    }
}

/// The resolution a PNG's `pHYs` chunk records.
fn png_dpi(bytes: &[u8]) -> Option<(f64, f64)> {
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        let kind = &bytes[at + 4..at + 8];
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        if kind == b"pHYs" && len == 9 && at + 17 <= bytes.len() {
            let d = &bytes[at + 8..at + 17];
            let x = u32::from_be_bytes(d[0..4].try_into().ok()?);
            let y = u32::from_be_bytes(d[4..8].try_into().ok()?);
            if d[8] != 1 || x == 0 || y == 0 {
                return None;
            }
            return Some((f64::from(x) * 0.0254, f64::from(y) * 0.0254));
        }
        at = at.checked_add(12 + len)?;
    }
    None
}

/// The resolution a JPEG's JFIF header records.
fn jpeg_dpi(bytes: &[u8]) -> Option<(f64, f64)> {
    let app0 = bytes.get(2..18)?;
    if app0[0] != 0xff || app0[1] != 0xe0 || &app0[4..9] != b"JFIF\0" {
        return None;
    }
    let units = app0[11];
    let x = f64::from(u16::from_be_bytes([app0[12], app0[13]]));
    let y = f64::from(u16::from_be_bytes([app0[14], app0[15]]));
    if x == 0.0 || y == 0.0 {
        return None;
    }
    match units {
        1 => Some((x, y)),
        2 => Some((x * 2.54, y * 2.54)),
        _ => None,
    }
}

/// Decodes a WebP image within limits and stores it as PNG, which every
/// office suite reads.
fn webp(bytes: &[u8]) -> Result<Media, String> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::WebP);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_WEBP_SIDE);
    limits.max_image_height = Some(MAX_WEBP_SIDE);
    limits.max_alloc = Some(MAX_WEBP_PIXELS * 4 * 2);
    reader.limits(limits);
    let (w, h) = ImageReader::with_format(Cursor::new(bytes), ImageFormat::WebP)
        .into_dimensions()
        .map_err(|e| format!("cannot read the WebP image: {e}"))?;
    if w == 0 || h == 0 {
        return Err("the image has no pixels".into());
    }
    if u64::from(w) * u64::from(h) > MAX_WEBP_PIXELS {
        return Err(format!(
            "the WebP image has more than {MAX_WEBP_PIXELS} pixels"
        ));
    }
    let decoded = reader
        .decode()
        .map_err(|e| format!("cannot decode the WebP image: {e}"))?;
    let rgba = decoded.to_rgba8();
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("cannot convert the WebP image: {e}"))?;
    if out.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "the WebP image converts to more than {MAX_IMAGE_BYTES} bytes of PNG"
        ));
    }
    Ok(raster(out, "png", "image/png", (w, h), (96.0, 96.0)))
}

fn from_svg(bytes: &[u8]) -> Result<Media, String> {
    let found = svg::inspect(bytes)?;
    let px = (
        (found.size.0.round() as u32).max(1),
        (found.size.1.round() as u32).max(1),
    );
    let pt = (found.size.0 * PT_PER_PX, found.size.1 * PT_PER_PX);
    let fallback = raster(found.png, "png", "image/png", found.png_px, (96.0, 96.0));
    let fallback = Media {
        physical_pt: pt,
        ..fallback
    };
    let bytes = found.clean.into_bytes();
    let hash = Sha256::digest(&bytes).into();
    Ok(Media {
        bytes,
        ext: "svg",
        mime: "image/svg+xml",
        hash,
        px,
        physical_pt: pt,
        fallback: Some(Box::new(fallback)),
    })
}

/// The printed size in points: as asked, keeping the aspect when one side is
/// given, otherwise the natural size capped at the text width.
pub fn fit(px: (u32, u32), width: Option<f64>, height: Option<f64>) -> Result<(f64, f64), String> {
    let (w, h) = (f64::from(px.0) * PT_PER_PX, f64::from(px.1) * PT_PER_PX);
    let size = match (width, height) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) => (a, a * h / w),
        (None, Some(b)) => (b * w / h, b),
        (None, None) if w > TEXT_WIDTH_PT => (TEXT_WIDTH_PT, TEXT_WIDTH_PT * h / w),
        (None, None) => (w, h),
    };
    if size.0 > MAX_SIDE_PT || size.1 > MAX_SIDE_PT {
        return Err(format!(
            "the image would print larger than {MAX_SIDE_PT} points; give both width and height"
        ));
    }
    Ok(size)
}

/// How an image sits in a frame of another shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// All of the image, centred, with the aspect kept.
    Contain,
    /// The frame filled, the image cropped, with the aspect kept.
    Cover,
    /// The frame's width kept and its height following the image.
    Width,
    /// The image stretched to the frame.
    Stretch,
}

impl Fit {
    pub fn parse(name: &str) -> Option<Fit> {
        match name {
            "contain" => Some(Fit::Contain),
            "cover" | "fill" => Some(Fit::Cover),
            "width" => Some(Fit::Width),
            "stretch" => Some(Fit::Stretch),
            _ => None,
        }
    }
}

/// Where an image sits in a frame: the share of the image cut from each
/// side (`left, top, right, bottom`; negative adds empty space), and the
/// frame height to use instead of the frame's own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub crop: [f64; 4],
    pub height: Option<f64>,
}

/// Places an image of `aspect` (width over height) in a `frame` of points.
pub fn place(frame: (f64, f64), aspect: f64, fit: Fit) -> Placement {
    let none = Placement {
        crop: [0.0; 4],
        height: None,
    };
    let frame_aspect = frame.0 / frame.1;
    if !(aspect.is_finite() && frame_aspect.is_finite() && aspect > 0.0 && frame_aspect > 0.0) {
        return none;
    }
    match fit {
        Fit::Stretch => none,
        Fit::Width => Placement {
            crop: [0.0; 4],
            height: Some(frame.0 / aspect),
        },
        Fit::Cover | Fit::Contain => {
            // The share of the image's width (or height) the frame shows.
            let (horizontal, share) = if aspect > frame_aspect {
                (true, frame_aspect / aspect)
            } else {
                (false, aspect / frame_aspect)
            };
            let cut = match fit {
                // Cutting (1 - share) / 2 from each side shows `share` of it.
                Fit::Cover => (1.0 - share) / 2.0,
                // Padding so the image is `share` of the frame on the other axis.
                _ => {
                    let pad = (1.0 / share - 1.0) / 2.0;
                    return Placement {
                        crop: if horizontal {
                            [0.0, -pad, 0.0, -pad]
                        } else {
                            [-pad, 0.0, -pad, 0.0]
                        },
                        height: None,
                    };
                }
            };
            Placement {
                crop: if horizontal {
                    [cut, 0.0, cut, 0.0]
                } else {
                    [0.0, cut, 0.0, cut]
                },
                height: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Vec<u8> {
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(&[0, 255, 255, 0], 2, 2, image::ExtendedColorType::L8)
            .unwrap();
        out
    }

    #[test]
    fn sizes_keep_the_aspect() {
        assert_eq!(fit((200, 100), None, None).unwrap(), (150.0, 75.0));
        assert_eq!(fit((200, 100), Some(50.0), None).unwrap(), (50.0, 25.0));
        assert_eq!(fit((200, 100), None, Some(50.0)).unwrap(), (100.0, 50.0));
        assert_eq!(fit((200, 100), Some(9.0), Some(9.0)).unwrap(), (9.0, 9.0));
        assert_eq!(fit((1200, 600), None, None).unwrap(), (450.0, 225.0));
        assert!(fit((1, 100_000), Some(450.0), None).is_err());
    }

    #[test]
    fn refuses_what_is_not_an_image_it_knows() {
        assert!(inspect(tiny(), None).is_ok());
        assert!(inspect(tiny(), Some("image/jpeg")).is_err());
        assert!(inspect(b"GIF89a....".to_vec(), None).is_err());
        assert!(inspect(b"BM\x00\x00".to_vec(), None).is_err());
        assert!(inspect(b"\x89PNG\r\n\x1a\nbroken".to_vec(), None).is_err());
        assert!(inspect(vec![0xff, 0xd8, 0xff, 0xe0], None).is_err());
        assert!(from_text("http://example.com/a.png").is_err());
        assert!(from_text("data:image/png,abc").is_err());
    }

    #[test]
    fn gif_stays_as_it_is_and_webp_becomes_png() {
        let mut gif = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut gif)
            .encode(
                &[255, 0, 0, 255].repeat(6),
                3,
                2,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let media = inspect(gif.clone(), Some("image/gif")).unwrap();
        assert_eq!(
            (media.ext, media.px, media.bytes == gif),
            ("gif", (3, 2), true)
        );

        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(
                &[0, 0, 255, 255].repeat(12),
                4,
                3,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let media = inspect(webp.clone(), Some("image/webp")).unwrap();
        assert_eq!(
            (media.ext, media.mime, media.px),
            ("png", "image/png", (4, 3))
        );
        assert!(media.bytes.starts_with(&PNG_MAGIC));
        assert!(inspect(webp, Some("image/png")).is_err());
    }

    #[test]
    fn reads_the_resolution_a_file_records() {
        let mut png = tiny();
        // pHYs: 300 dpi is 11811 pixels a metre.
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&9u32.to_be_bytes());
        chunk.extend_from_slice(b"pHYs");
        chunk.extend_from_slice(&11811u32.to_be_bytes());
        chunk.extend_from_slice(&11811u32.to_be_bytes());
        chunk.push(1);
        chunk.extend_from_slice(&[0, 0, 0, 0]);
        png.splice(33..33, chunk);
        let dpi = png_dpi(&png).unwrap();
        assert!((dpi.0 - 300.0).abs() < 0.1, "{dpi:?}");
        assert_eq!(png_dpi(&tiny()), None);
    }

    #[test]
    fn places_an_image_in_a_frame() {
        // A 2:1 image in a square frame.
        let contain = place((100.0, 100.0), 2.0, Fit::Contain);
        assert_eq!(contain.crop, [0.0, -0.5, 0.0, -0.5]);
        let cover = place((100.0, 100.0), 2.0, Fit::Cover);
        assert_eq!(cover.crop, [0.25, 0.0, 0.25, 0.0]);
        assert_eq!(place((100.0, 100.0), 2.0, Fit::Width).height, Some(50.0));
        // A 1:2 image in a square frame.
        assert_eq!(
            place((100.0, 100.0), 0.5, Fit::Cover).crop,
            [0.0, 0.25, 0.0, 0.25]
        );
        assert_eq!(
            place((100.0, 100.0), 0.5, Fit::Contain).crop,
            [-0.5, 0.0, -0.5, 0.0]
        );
        assert_eq!(place((100.0, 50.0), 2.0, Fit::Cover).crop, [0.0; 4]);
        assert_eq!(place((100.0, 50.0), 2.0, Fit::Stretch).crop, [0.0; 4]);
    }
}
