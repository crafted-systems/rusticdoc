//! HTML to PDF with Fulgur, a layout engine with no browser and no network.
//!
//! Fulgur parses the HTML and CSS with Servo's style engine, lays it out in
//! pages and writes the PDF itself. It runs no script and fetches nothing: an
//! image, stylesheet or font reaches it only as an asset sent with the request
//! and named by its file name, and it reads no file, because no base directory
//! is ever set. Fonts come from the system's fontconfig, which in the image
//! includes the mounted font directory, and from font assets.
//!
//! A service runs this in the render worker, under its memory and CPU limits,
//! because the layout of a hostile page is unbounded work.

use fulgur::asset::AssetBundle;
use fulgur::engine::Engine;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::job::Document;

/// Extensions sent to Fulgur rather than LibreOffice.
pub const EXTENSIONS: &[&str] = &["html", "htm", "xhtml"];

/// Most assets one conversion may carry.
pub const MAX_ASSETS: usize = 200;

/// True for a document Fulgur converts.
pub fn is_html(doc: &Document) -> bool {
    EXTENSIONS.contains(&doc.extension().as_str())
}

/// The engine that turns HTML into PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HtmlEngine {
    /// Fulgur: CSS layout with paged media, no network.
    #[default]
    Fulgur,
    /// LibreOffice's Writer/Web import, for simple HTML.
    LibreOffice,
}

impl HtmlEngine {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fulgur" => Ok(Self::Fulgur),
            "libreoffice" => Ok(Self::LibreOffice),
            other => Err(Error::InvalidRequest(format!(
                "'htmlEngine' is '{other}'; use 'fulgur' or 'libreoffice'"
            ))),
        }
    }
}

/// How one page is laid out, beyond what its own CSS says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HtmlOptions {
    /// Turn the default page to landscape. An `@page { size }` rule wins.
    pub landscape: bool,
    /// Tagged PDF/UA-1, with bookmarks from the headings.
    pub pdfua: bool,
}

/// What kind of asset a file is, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetKind {
    Image,
    Css,
    Font,
}

fn kind_of(doc: &Document) -> Option<AssetKind> {
    match doc.extension().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "svg" => Some(AssetKind::Image),
        "css" => Some(AssetKind::Css),
        "ttf" | "otf" | "ttc" | "woff2" => Some(AssetKind::Font),
        _ => None,
    }
}

/// Refuses assets Fulgur cannot use, before any work: a typo in a file name
/// should not quietly drop a logo.
pub fn check_assets(assets: &[Document]) -> Result<()> {
    if assets.len() > MAX_ASSETS {
        return Err(Error::InvalidRequest(format!(
            "{} assets is more than the {MAX_ASSETS} one conversion may carry",
            assets.len()
        )));
    }
    for doc in assets {
        if kind_of(doc).is_none() {
            return Err(Error::InvalidRequest(format!(
                "asset '{}' is not an image (PNG, JPEG, GIF, SVG), a stylesheet (CSS) or a font (TTF, OTF, TTC, WOFF2)",
                doc.name
            )));
        }
    }
    Ok(())
}

/// Lays out `html` and writes it as a PDF. Images are found by the file name
/// in `src`; every stylesheet applies to the whole document; fonts join the
/// system's.
pub fn to_pdf(html: &[u8], assets: &[Document], options: &HtmlOptions) -> Result<Vec<u8>> {
    check_assets(assets)?;
    let html = std::str::from_utf8(html)
        .map_err(|_| Error::InvalidRequest("the HTML is not valid UTF-8".into()))?;
    let mut bundle = AssetBundle::new();
    for doc in assets {
        match kind_of(doc) {
            Some(AssetKind::Image) => bundle.add_image(doc.name.clone(), doc.bytes.to_vec()),
            Some(AssetKind::Css) => {
                let css = std::str::from_utf8(&doc.bytes).map_err(|_| {
                    Error::InvalidRequest(format!("stylesheet '{}' is not UTF-8", doc.name))
                })?;
                bundle.add_css(css.to_string());
            }
            Some(AssetKind::Font) => bundle
                .add_font_bytes(doc.bytes.to_vec())
                .map_err(|e| Error::InvalidRequest(format!("font '{}': {e}", doc.name)))?,
            None => unreachable!("checked above"),
        }
    }
    let engine = Engine::builder()
        .assets(bundle)
        .system_fonts(true)
        .landscape(options.landscape)
        .pdf_ua(options.pdfua)
        .producer(format!("rusticdoc {}", crate::VERSION))
        .build();
    engine.render(html).map_err(|e| Error::Engine {
        engine: "fulgur",
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_parse_and_assets_are_checked() {
        assert_eq!(HtmlEngine::parse("Fulgur").unwrap(), HtmlEngine::Fulgur);
        assert_eq!(
            HtmlEngine::parse("libreoffice").unwrap(),
            HtmlEngine::LibreOffice
        );
        assert!(HtmlEngine::parse("chrome").is_err());
        assert!(check_assets(&[Document::new("logo.png", vec![])]).is_ok());
        let err = check_assets(&[Document::new("macro.js", vec![])]).unwrap_err();
        assert!(err.to_string().contains("macro.js"), "{err}");
        assert!(is_html(&Document::new("a.HTML", vec![])));
        assert!(is_html(&Document::new("a.htm", vec![])));
        assert!(!is_html(&Document::new("a.docx", vec![])));
    }

    #[test]
    fn a_page_becomes_a_pdf_and_nothing_outside_is_read() {
        let html = br#"<!doctype html><html><body>
            <h1>Invoice INV-2041</h1>
            <img src="file:///etc/passwd"><img src="http://127.0.0.1:9/x.png">
            <link rel="stylesheet" href="file:///etc/passwd">
            </body></html>"#;
        let pdf = to_pdf(html, &[], &HtmlOptions::default()).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        let doc = lopdf::Document::load_mem(&pdf).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
        assert!(!String::from_utf8_lossy(&pdf).contains("root:"));
    }

    #[test]
    fn the_same_page_gives_the_same_bytes() {
        let html = b"<p>Same every time</p>";
        let a = to_pdf(html, &[], &HtmlOptions::default()).unwrap();
        let b = to_pdf(html, &[], &HtmlOptions::default()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn landscape_turns_the_default_page() {
        let html = b"<p>Wide</p>";
        let pdf = to_pdf(
            html,
            &[],
            &HtmlOptions {
                landscape: true,
                ..HtmlOptions::default()
            },
        )
        .unwrap();
        let doc = lopdf::Document::load_mem(&pdf).unwrap();
        let (_, page) = doc.get_pages().into_iter().next().unwrap();
        let media = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"MediaBox")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_float().unwrap_or_else(|_| v.as_i64().unwrap() as f32))
            .collect::<Vec<_>>();
        assert!(media[2] > media[3], "{media:?}");
    }
}
