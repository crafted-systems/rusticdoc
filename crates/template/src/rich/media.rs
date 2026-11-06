//! Image values and generated barcodes.

use std::io::Cursor;

use base64::alphabet::STANDARD;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageFormat, ImageReader};
use minijinja::value::{Value, ValueKind};
use sha2::{Digest, Sha256};

/// The most bytes one image may hold.
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// The most images and barcodes one render may place.
pub const MAX_IMAGES: usize = 200;
/// The longest value a barcode accepts.
pub const MAX_BARCODE_INPUT: usize = 4096;
/// The widest a printed linear barcode gets, in modules.
const MAX_BARCODE_MODULES: usize = 4096;
/// The page text width that caps an image's natural size.
pub const TEXT_WIDTH_PT: f64 = 450.0;
/// The longest side accepted, in points (200 inches).
pub const MAX_SIDE_PT: f64 = 14400.0;
/// A pixel at the 96 dpi images are assumed to have.
const PT_PER_PX: f64 = 0.75;

/// Verified image bytes.
#[derive(Debug)]
pub struct Media {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    pub mime: &'static str,
    pub hash: [u8; 32],
    pub px: (u32, u32),
}

/// What a value of the `image` filter carries.
#[derive(Debug, Default)]
pub struct Input {
    pub bytes: Vec<u8>,
    pub mime: Option<String>,
    pub width: Option<f64>,
    pub height: Option<f64>,
}

const BASE64: GeneralPurpose = GeneralPurpose::new(
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

/// Verifies the magic bytes and reads the dimensions from the header. Pixels
/// are never decoded, so a small file claiming huge dimensions costs nothing.
pub fn inspect(bytes: Vec<u8>, declared: Option<&str>) -> Result<Media, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!("the image is larger than {MAX_IMAGE_BYTES} bytes"));
    }
    let png_magic = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let (format, ext, mime) = if bytes.starts_with(&png_magic) {
        (ImageFormat::Png, "png", "image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        (ImageFormat::Jpeg, "jpeg", "image/jpeg")
    } else {
        return Err("only PNG and JPEG images are supported".into());
    };
    if let Some(declared) = declared {
        let same = declared == mime || (ext == "jpeg" && declared == "image/jpg");
        if !same {
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
    let hash = Sha256::digest(&bytes).into();
    Ok(Media {
        bytes,
        ext,
        mime,
        hash,
        px,
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

fn png(pixels: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(pixels, width, height, ExtendedColorType::L8)
        .map_err(|e| format!("cannot encode the barcode: {e}"))?;
    Ok(out)
}

/// A whole number of device pixels per module, so edges stay crisp.
fn module_px(size_pt: f64, modules: usize) -> u32 {
    ((size_pt * 4.0 / modules as f64).ceil() as u32).clamp(2, 16)
}

fn matrix_png(rows: &[Vec<bool>], size_pt: f64) -> Result<Vec<u8>, String> {
    let (columns, lines) = (rows[0].len(), rows.len());
    let m = module_px(size_pt, columns) as usize;
    let mut pixels = Vec::with_capacity(columns * m * lines * m);
    for row in rows {
        let mut line = Vec::with_capacity(columns * m);
        for dark in row {
            line.extend(std::iter::repeat_n(if *dark { 0u8 } else { 255 }, m));
        }
        for _ in 0..m {
            pixels.extend_from_slice(&line);
        }
    }
    png(&pixels, (columns * m) as u32, (lines * m) as u32)
}

fn linear_modules(kind: &str, value: &str) -> Result<Vec<u8>, String> {
    use barcoders::sym::code128::Code128;
    use barcoders::sym::code39::Code39;
    use barcoders::sym::ean13::EAN13;
    use barcoders::sym::ean8::EAN8;
    let bad = |e: barcoders::error::Error| format!("{value:?} is not a valid {kind} value: {e}");
    match kind {
        "code128" => {
            if !value.bytes().all(|b| (32..127).contains(&b)) {
                return Err("code128 takes printable ASCII only".into());
            }
            // Character set B holds all of printable ASCII.
            Ok(Code128::new(format!("\u{181}{value}"))
                .map_err(bad)?
                .encode())
        }
        "code39" => Ok(Code39::new(value).map_err(bad)?.encode()),
        "ean13" => Ok(EAN13::new(value).map_err(bad)?.encode()),
        "ean8" => Ok(EAN8::new(value).map_err(bad)?.encode()),
        other => Err(format!(
            "unknown symbology {other:?}; use qr, code128, code39, ean13 or ean8"
        )),
    }
}

/// Generates a barcode PNG and its printed size. `size` is the printed width
/// in points.
pub fn barcode(kind: &str, value: &str, size: Option<f64>) -> Result<(Media, (f64, f64)), String> {
    if value.is_empty() {
        return Err("a barcode needs a value".into());
    }
    if value.len() > MAX_BARCODE_INPUT {
        return Err(format!(
            "the value is longer than {MAX_BARCODE_INPUT} bytes"
        ));
    }
    let (bytes, printed) = if kind == "qr" {
        const QUIET: usize = 4;
        let code = qrcode::QrCode::new(value.as_bytes())
            .map_err(|e| format!("cannot encode this value as a QR code: {e}"))?;
        let width = code.width();
        let total = width + 2 * QUIET;
        let mut rows = vec![vec![false; total]; total];
        for (i, color) in code.to_colors().iter().enumerate() {
            rows[QUIET + i / width][QUIET + i % width] = *color == qrcode::Color::Dark;
        }
        let size = size.unwrap_or(144.0);
        (matrix_png(&rows, size)?, (size, size))
    } else {
        const QUIET: usize = 10;
        let modules = linear_modules(kind, value)?;
        let total = modules.len() + 2 * QUIET;
        if total > MAX_BARCODE_MODULES {
            return Err(format!(
                "the value is too long to print as {kind}: {total} modules"
            ));
        }
        let mut row = vec![false; QUIET];
        row.extend(modules.iter().map(|m| *m != 0));
        row.extend(std::iter::repeat_n(false, QUIET));
        let lines = (total / 4).max(50);
        let rows = vec![row; lines];
        let size = size.unwrap_or(200.0);
        let height = size * lines as f64 / total as f64;
        (matrix_png(&rows, size)?, (size, height))
    };
    let media = inspect(bytes, Some("image/png"))?;
    Ok((media, printed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Vec<u8> {
        png(&[0, 255, 255, 0], 2, 2).unwrap()
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
    fn refuses_what_is_not_png_or_jpeg() {
        assert!(inspect(tiny(), None).is_ok());
        assert!(inspect(tiny(), Some("image/jpeg")).is_err());
        assert!(inspect(b"GIF89a....".to_vec(), None).is_err());
        assert!(inspect(b"\x89PNG\r\n\x1a\nbroken".to_vec(), None).is_err());
        assert!(inspect(vec![0xff, 0xd8, 0xff, 0xe0], None).is_err());
        assert!(from_text("http://example.com/a.png").is_err());
        assert!(from_text("data:image/png,abc").is_err());
    }

    #[test]
    fn barcodes_validate_and_are_real_pngs() {
        for (kind, value) in [
            ("qr", "https://example.com"),
            ("code128", "SKU-123 a"),
            ("code39", "ABC-123"),
            ("ean13", "5901234123457"),
            ("ean13", "590123412345"),
            ("ean8", "96385074"),
        ] {
            let (media, size) = barcode(kind, value, None).unwrap();
            assert!(media.px.0 >= 100, "{kind}");
            assert!(size.0 > 0.0 && size.1 > 0.0, "{kind}");
        }
        for (kind, value) in [
            ("ean13", "5901234123458"),
            ("ean13", "12"),
            ("ean8", "96385075"),
            ("code39", "lower"),
            ("code128", "caf\u{e9}"),
            ("aztec", "x"),
            ("qr", ""),
        ] {
            assert!(barcode(kind, value, None).is_err(), "{kind} {value}");
        }
        assert!(barcode("qr", &"x".repeat(MAX_BARCODE_INPUT + 1), None).is_err());
        assert!(barcode("code128", &"x".repeat(MAX_BARCODE_INPUT), None).is_err());
    }

    #[test]
    fn module_size_is_a_whole_number() {
        let (media, _) = barcode("qr", "a", Some(100.0)).unwrap();
        // Version 1 is 21 modules plus an 8 module quiet zone.
        assert_eq!(media.px.0 % 29, 0);
    }
}
