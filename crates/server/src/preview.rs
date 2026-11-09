//! A page of a rendered document as a PNG, drawn by poppler's `pdftoppm`.
//!
//! Poppler is already in the image as LibreOffice's PDF import library, so the
//! binary costs little, and it draws what viewers draw. The page is measured
//! before anything is drawn, so a request cannot ask for an image larger than
//! [`MAX_SIDE`] on a side, whatever the page size.

use std::path::Path;
use std::time::Duration;

use axum::http::StatusCode;
use lopdf::{Object, ObjectId};
use rusticdoc_core::Error;

use crate::error::{ApiError, ApiResult};

/// Most pixels an image may have on either side.
pub const MAX_SIDE: u32 = 4000;
/// The widths a request may ask for.
pub const MIN_WIDTH: u32 = 16;
pub const MAX_WIDTH: u32 = 2000;
/// The resolutions a request may ask for.
pub const MIN_DPI: u32 = 18;
pub const MAX_DPI: u32 = 300;
/// The highest page number a request may name.
pub const MAX_PAGE: u32 = 100_000;
/// The width when a request names neither.
pub const DEFAULT_WIDTH: u32 = 800;

/// How large to draw the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// This many pixels wide, as tall as the page's shape makes it.
    Width(u32),
    /// At this resolution, as large as the page is.
    Dpi(u32),
}

impl Size {
    /// The size `width` and `dpi` ask for, within the limits. Asking for both
    /// is asking for two things.
    pub fn parse(width: Option<&str>, dpi: Option<&str>) -> ApiResult<Self> {
        let number = |name: &str, text: &str, min: u32, max: u32| -> ApiResult<u32> {
            match text.trim().parse::<u32>() {
                Ok(n) if (min..=max).contains(&n) => Ok(n),
                _ => Err(ApiError::bad_request(format!(
                    "'{name}' is a whole number from {min} to {max}"
                ))),
            }
        };
        match (width, dpi) {
            (Some(_), Some(_)) => Err(ApiError::bad_request("send 'width' or 'dpi', not both")),
            (Some(w), None) => Ok(Size::Width(number("width", w, MIN_WIDTH, MAX_WIDTH)?)),
            (None, Some(d)) => Ok(Size::Dpi(number("dpi", d, MIN_DPI, MAX_DPI)?)),
            (None, None) => Ok(Size::Width(DEFAULT_WIDTH)),
        }
    }

    /// The image's size for a page of `width` by `height` points.
    fn pixels(self, width: f32, height: f32) -> (f32, f32) {
        match self {
            Size::Width(w) => (w as f32, w as f32 * height / width.max(1.0)),
            Size::Dpi(d) => (width / 72.0 * d as f32, height / 72.0 * d as f32),
        }
    }
}

/// A drawn page, and how many the document has.
#[derive(Debug)]
pub struct Drawn {
    pub png: Vec<u8>,
    pub pages: u32,
}

/// The number of pages, and the size in points of page `page` as shown, with
/// its rotation applied.
fn measure(pdf: &[u8], page: u32) -> rusticdoc_core::Result<(u32, Option<(f32, f32)>)> {
    let doc = lopdf::Document::load_mem(pdf).map_err(|e| Error::Engine {
        engine: "preview",
        message: format!("the rendered PDF cannot be read: {e}"),
    })?;
    let pages = doc.get_pages();
    let count = pages.len() as u32;
    let Some(&id) = pages.get(&page) else {
        return Ok((count, None));
    };
    let media = inherited(&doc, id, b"MediaBox")
        .and_then(|o| o.as_array().ok().cloned())
        .and_then(|a| {
            let n: Vec<f32> = a.iter().filter_map(|o| o.as_float().ok()).collect();
            (n.len() == 4).then(|| ((n[2] - n[0]).abs(), (n[3] - n[1]).abs()))
        })
        .unwrap_or((612.0, 792.0));
    let rotate = inherited(&doc, id, b"Rotate")
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0);
    let (w, h) = if rotate.rem_euclid(180) == 90 {
        (media.1, media.0)
    } else {
        media
    };
    Ok((count, Some((w, h))))
}

/// A page attribute, from the page or the nearest of its parents that has it.
fn inherited(doc: &lopdf::Document, mut id: ObjectId, key: &[u8]) -> Option<Object> {
    // The page tree is a client's; a loop in it must not hang the request.
    for _ in 0..64 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(value) = dict.get(key) {
            return match value {
                Object::Reference(r) => doc.get_object(*r).ok().cloned(),
                other => Some(other.clone()),
            };
        }
        id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// Draws page `page` (from 1) of `pdf` as a PNG of `size`.
pub async fn draw(
    bin: Option<&Path>,
    pdf: bytes::Bytes,
    page: u32,
    size: Size,
    timeout: Duration,
) -> ApiResult<Drawn> {
    let Some(bin) = bin else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "engine_unavailable",
            "previews need poppler's pdftoppm, which this service does not have; set RUSTICDOC_PDFTOPPM_BIN",
        ));
    };
    let started = std::time::Instant::now();
    let measured = pdf.clone();
    let (pages, shape) =
        rusticdoc_core::pdfops::blocking(timeout, move |_| measure(&measured, page)).await?;
    let Some((width, height)) = shape else {
        return Err(ApiError::bad_request(format!(
            "page {page} was asked for, and the document has {pages}"
        )));
    };
    let (w, h) = size.pixels(width, height);
    if w > MAX_SIDE as f32 || h > MAX_SIDE as f32 {
        return Err(ApiError::bad_request(format!(
            "this page would be {} by {} pixels; the limit is {MAX_SIDE} on a side, so ask for a smaller 'width' or 'dpi'",
            w.round(),
            h.round()
        )));
    }

    let dir = tempfile::Builder::new()
        .prefix("rusticdoc-preview-")
        .tempdir()
        .map_err(Error::from)?;
    tokio::fs::write(dir.path().join("in.pdf"), &pdf)
        .await
        .map_err(Error::from)?;
    let mut args = vec![
        "-png".to_string(),
        "-f".into(),
        page.to_string(),
        "-l".into(),
        page.to_string(),
        "-singlefile".into(),
    ];
    match size {
        Size::Width(w) => args.extend([
            "-scale-to-x".into(),
            w.to_string(),
            "-scale-to-y".into(),
            "-1".into(),
        ]),
        Size::Dpi(d) => args.extend(["-r".into(), d.to_string()]),
    }
    args.extend(["in.pdf".into(), "out".into()]);
    let left = timeout.saturating_sub(started.elapsed());
    rusticdoc_core::process::run("pdftoppm", bin, &args, dir.path(), &[], left).await?;
    let png = tokio::fs::read(dir.path().join("out.png"))
        .await
        .map_err(|e| {
            ApiError::from(Error::Engine {
                engine: "pdftoppm",
                message: format!("no image came out: {e}"),
            })
        })?;
    Ok(Drawn { png, pages })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_bounded_and_one_at_a_time() {
        assert_eq!(Size::parse(None, None).unwrap(), Size::Width(DEFAULT_WIDTH));
        assert_eq!(Size::parse(Some("640"), None).unwrap(), Size::Width(640));
        assert_eq!(Size::parse(None, Some("150")).unwrap(), Size::Dpi(150));
        for (w, d) in [
            (Some("0"), None),
            (Some("5000"), None),
            (Some("wide"), None),
            (None, Some("1200")),
            (Some("640"), Some("72")),
        ] {
            assert!(Size::parse(w, d).is_err(), "{w:?} {d:?}");
        }
    }

    #[test]
    fn pixels_follow_the_page_shape() {
        let (w, h) = Size::Width(612).pixels(612.0, 792.0);
        assert_eq!((w, h), (612.0, 792.0));
        let (w, h) = Size::Dpi(144).pixels(612.0, 792.0);
        assert_eq!((w, h), (1224.0, 1584.0));
    }
}
