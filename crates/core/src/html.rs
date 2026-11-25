//! HTML to PDF with Fulgur, a layout engine with no browser and no network.
//!
//! Fulgur parses the HTML and CSS with Servo's style engine, lays it out in
//! pages and writes the PDF itself. It runs no script and fetches nothing: an
//! image, stylesheet or font reaches it only as an asset, sent with the request
//! and named by its file name, or held by the service and named by the page. It
//! reads no other file, because no base directory is ever set. Fonts come from the system's fontconfig, which in the image
//! includes the mounted font directory, and from font assets.
//!
//! A service runs this in the render worker, under its memory and CPU limits,
//! because the layout of a hostile page is unbounded work.

use fulgur::asset::AssetBundle;
use fulgur::engine::Engine;
use rusticdoc_template::assets::{self, AssetDirs, AssetKind as HeldKind};
use serde::{Deserialize, Serialize};

use std::collections::BTreeSet;
use std::sync::LazyLock;

use bytes::Bytes;
use regex::Regex;

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
    /// Where the images and stylesheets the page names are found, when they
    /// were not sent with the request.
    #[serde(default)]
    pub library: AssetDirs,
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

/// The stylesheets a page links to, in order, by the name in `href`. A link in
/// a comment or only for screens is not one.
fn linked_stylesheets(html: &str) -> Vec<String> {
    static COMMENT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("a valid pattern"));
    static LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<link\s[^>]*>").expect("a valid pattern"));
    static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)\s(rel|href|media)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#)
            .expect("a valid pattern")
    });
    let html = COMMENT.replace_all(html, "");
    let mut found = Vec::new();
    for link in LINK.find_iter(&html) {
        let (mut sheet, mut href, mut screen) = (false, None, false);
        for attribute in ATTRIBUTE.captures_iter(link.as_str()) {
            let value = (2..=4)
                .find_map(|i| attribute.get(i))
                .map_or("", |m| m.as_str().trim());
            match attribute[1].to_ascii_lowercase().as_str() {
                "rel" => {
                    sheet = value
                        .split_ascii_whitespace()
                        .any(|v| v.eq_ignore_ascii_case("stylesheet"));
                }
                "href" => href = Some(value.strip_prefix("./").unwrap_or(value)),
                _ => screen = value.eq_ignore_ascii_case("screen"),
            }
        }
        if let Some(href) = href.filter(|_| sheet && !screen)
            && assets::check_name(href) == Ok(HeldKind::Stylesheet)
            && !found.iter().any(|f| f == href)
        {
            found.push(href.to_string());
        }
    }
    found
}

/// The image names in a page or a stylesheet: whatever stands between quotes,
/// brackets or spaces and reads as one. An address never does.
fn named_images(text: &str, found: &mut BTreeSet<String>) {
    let separator = |c: char| c.is_whitespace() || "\"'()<>=,;&".contains(c);
    for word in text.split(separator) {
        let name = word.strip_prefix("./").unwrap_or(word);
        // The extension first: it settles nearly every word cheaply.
        if assets::kind_of(name) == Some(HeldKind::Image) && assets::check_name(name).is_ok() {
            found.insert(name.to_string());
        }
    }
}

/// The assets a page names, in itself, in `sheets` and in the stylesheets it
/// links to, that the library holds and the request did not send: stylesheets
/// in the page's order, then images.
fn from_library(
    html: &str,
    sheets: &[&str],
    sent: &[Document],
    library: &AssetDirs,
) -> Result<Vec<Document>> {
    if library.is_empty() {
        return Ok(Vec::new());
    }
    let sent: BTreeSet<&str> = sent.iter().map(|d| d.name.as_str()).collect();
    let held = |name: &str, count: usize| -> Result<Option<Document>> {
        if sent.contains(name) {
            return Ok(None);
        }
        let Some(bytes) = library.read(name).map_err(Error::InvalidRequest)? else {
            return Ok(None);
        };
        if count >= MAX_ASSETS {
            return Err(Error::InvalidRequest(format!(
                "the page names more than the {MAX_ASSETS} assets one conversion may carry"
            )));
        }
        Ok(Some(Document::new(name, bytes)))
    };
    let mut found = Vec::new();
    // A template that prints a name escapes its slashes.
    static SLASH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new("(?i)&(?:#x0*2f|#0*47|sol);").expect("a valid pattern"));
    let html = &*SLASH.replace_all(html, "/");
    for name in linked_stylesheets(html) {
        found.extend(held(&name, found.len())?);
    }
    let mut images = BTreeSet::new();
    named_images(html, &mut images);
    for sheet in sheets {
        named_images(sheet, &mut images);
    }
    for sheet in &found {
        named_images(&String::from_utf8_lossy(&sheet.bytes), &mut images);
    }
    for name in images {
        // Fulgur draws no WebP.
        if kind_of(&Document::new(name.as_str(), Bytes::new())).is_some() {
            found.extend(held(&name, found.len())?);
        }
    }
    Ok(found)
}

/// Lays out `html` and writes it as a PDF. Images are found by the file name
/// in `src`; every stylesheet applies to the whole document; fonts join the
/// system's. What the page names and the request did not send comes from the
/// library, and its stylesheets come before those sent.
pub fn to_pdf(html: &[u8], assets: &[Document], options: &HtmlOptions) -> Result<Vec<u8>> {
    check_assets(assets)?;
    let html = std::str::from_utf8(html)
        .map_err(|_| Error::InvalidRequest("the HTML is not valid UTF-8".into()))?;
    let mut bundle = AssetBundle::new();
    let sheets: Vec<&str> = assets
        .iter()
        .filter(|doc| kind_of(doc) == Some(AssetKind::Css))
        .filter_map(|doc| std::str::from_utf8(&doc.bytes).ok())
        .collect();
    let held = from_library(html, &sheets, assets, &options.library)?;
    for doc in held.iter().chain(assets) {
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

    const DOT: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0,
        0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0x90, 0x77, 0x53, 0xde, 0, 0, 0, 12, b'I', b'D', b'A',
        b'T', 8, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0, 0, 3, 1, 1, 0, 0x18, 0xdd, 0x8d, 0xb0, 0, 0, 0,
        0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82,
    ];

    fn library(files: &[(&str, &[u8])]) -> (tempfile::TempDir, AssetDirs) {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let dirs = AssetDirs {
            mounted: Some(dir.path().to_path_buf()),
            stored: None,
        };
        (dir, dirs)
    }

    fn names(docs: &[Document]) -> Vec<&str> {
        docs.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn only_what_a_page_names_comes_from_the_library() {
        let (_dir, dirs) = library(&[
            ("logo.png", DOT),
            ("brand/mark.svg", b"<svg/>"),
            ("bg.jpg", b"jpg"),
            ("unused.png", DOT),
            ("cdn.png", DOT),
            ("photo.webp", b"webp"),
            ("brand.css", b"body { background: url('bg.jpg') }"),
            ("unlinked.css", b"p { color: red }"),
            ("screen.css", b""),
            ("off.css", b""),
        ]);
        let html = r#"<link rel="stylesheet" href="./brand.css"><link href=brand.css rel=stylesheet>
            <!-- <link rel="stylesheet" href="off.css"> -->
            <link rel="stylesheet" media="screen" href="screen.css">
            <link rel="icon" href="unlinked.css"> unlinked.css
            <img src="logo.png"><i style="background:url(&quot;brand/mark.svg&quot;)"></i>
            <img src="https://example.com/cdn.png"><img src="file:///etc/cdn.png">
            <img src="../logo.png"><img src="/logo.png"><img src="photo.webp"><img src="gone.png">"#;
        let found = from_library(html, &[], &[], &dirs).unwrap();
        assert_eq!(
            names(&found),
            ["brand.css", "bg.jpg", "brand/mark.svg", "logo.png"]
        );
        // What the request sends wins.
        let sent = [Document::new("logo.png", DOT.to_vec())];
        let found = from_library("<img src=logo.png>", &[], &sent, &dirs).unwrap();
        assert!(found.is_empty());
        // A name from a template's data has its slashes escaped.
        let found = from_library("<img src=\"brand&#x2f;mark.svg\">", &[], &[], &dirs).unwrap();
        assert_eq!(names(&found), ["brand/mark.svg"]);
        // A stylesheet that was sent may name an image that was not.
        let found = from_library("", &["a { content: url(logo.png) }"], &[], &dirs).unwrap();
        assert_eq!(names(&found), ["logo.png"]);
    }

    #[test]
    fn a_page_shows_an_image_from_the_library() {
        let (_dir, dirs) = library(&[("brand/logo.png", DOT)]);
        let html = br#"<img src="brand/logo.png" style="width:40px;height:40px">"#;
        let options = HtmlOptions {
            library: dirs,
            ..HtmlOptions::default()
        };
        let drawn = |pdf: &[u8]| {
            let doc = lopdf::Document::load_mem(pdf).unwrap();
            doc.objects.values().any(|object| {
                object.as_stream().is_ok_and(|stream| {
                    let subtype = stream.dict.get(b"Subtype");
                    subtype
                        .and_then(|s| s.as_name())
                        .is_ok_and(|s| s == b"Image")
                })
            })
        };
        assert!(drawn(&to_pdf(html, &[], &options).unwrap()));
        assert!(!drawn(&to_pdf(html, &[], &HtmlOptions::default()).unwrap()));
    }

    #[test]
    fn a_linked_stylesheet_applies_and_a_sent_one_comes_after_it() {
        let (_dir, dirs) = library(&[("page.css", b"@page { size: 100pt 200pt }")]);
        let html = br#"<link rel="stylesheet" href="page.css"><p>x</p>"#;
        let options = HtmlOptions {
            library: dirs,
            ..HtmlOptions::default()
        };
        let width = |pdf: &[u8]| {
            let doc = lopdf::Document::load_mem(pdf).unwrap();
            let (_, page) = doc.get_pages().into_iter().next().unwrap();
            let media = doc.get_dictionary(page).unwrap().get(b"MediaBox").unwrap();
            let right = &media.as_array().unwrap()[2];
            right
                .as_float()
                .unwrap_or_else(|_| right.as_i64().unwrap() as f32)
                .round()
        };
        assert_eq!(width(&to_pdf(html, &[], &options).unwrap()), 100.0);
        let sent = [Document::new(
            "other.css",
            b"@page { size: 300pt 400pt }".to_vec(),
        )];
        assert_eq!(width(&to_pdf(html, &sent, &options).unwrap()), 300.0);
    }
}
