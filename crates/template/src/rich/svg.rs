//! SVG images: a copy that keeps only drawing, and a PNG of it for readers
//! that do not show SVG.
//!
//! The copy is rebuilt element by element from an allowlist. Scripts, event
//! handlers, animation, foreign content and every reference that leaves the
//! file are dropped, so neither the service nor whoever opens the document
//! loads anything from elsewhere. Embedded pictures stay only as PNG, JPEG or
//! GIF data whose size is checked before anything decodes them.

use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use base64::Engine as _;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageReader};
use resvg::tiny_skia;
use resvg::usvg;

/// The most bytes an SVG image may hold.
pub const MAX_SVG_BYTES: usize = 5 * 1024 * 1024;
/// The most XML nodes an SVG image may hold.
const MAX_NODES: u32 = 200_000;
/// The most pixels a picture embedded in an SVG may have.
const MAX_EMBEDDED_PIXELS: u64 = 16_000_000;
/// The longest side of the PNG drawn for readers without SVG.
const FALLBACK_SIDE: f32 = 2000.0;
/// The most the PNG enlarges a small drawing.
const FALLBACK_SCALE: f32 = 4.0;

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Elements that only draw. Anything else is dropped with its content,
/// except `a`, whose content stays in a group.
const ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "switch",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "image",
    "linearGradient",
    "radialGradient",
    "stop",
    "pattern",
    "clipPath",
    "mask",
    "marker",
    "filter",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDistantLight",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
    "fePointLight",
    "feSpecularLighting",
    "feSpotLight",
    "feTile",
    "feTurbulence",
    "title",
    "desc",
    "style",
];

/// A checked SVG image.
pub struct Svg {
    /// The rebuilt document.
    pub clean: String,
    /// Its natural size in CSS pixels.
    pub size: (f64, f64),
    /// A PNG rendering and its size in pixels.
    pub png: Vec<u8>,
    pub png_px: (u32, u32),
}

/// Whether the bytes look like an SVG document rather than another format.
pub fn sniff(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let head = String::from_utf8_lossy(head);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with('<') && head.contains("<svg")
}

pub fn inspect(bytes: &[u8]) -> Result<Svg, String> {
    if bytes.len() > MAX_SVG_BYTES {
        return Err(format!(
            "the SVG image is larger than {MAX_SVG_BYTES} bytes"
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "the SVG image is not UTF-8")?;
    let clean = sanitize(text.trim_start_matches('\u{feff}'))?;
    let tree = parse(&clean)?;
    let size = tree.size();
    let (w, h) = (f64::from(size.width()), f64::from(size.height()));
    let largest = super::media::MAX_SIDE_PT / 0.75;
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && w <= largest && h <= largest) {
        return Err(format!(
            "the SVG image's size must be above 0 and at most {largest} pixels a side"
        ));
    }
    let (png, png_px) = render(&tree)?;
    Ok(Svg {
        clean,
        size: (w, h),
        png,
        png_px,
    })
}

fn parse(clean: &str) -> Result<usvg::Tree, String> {
    let mut opts = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            // Never a path or an address: the copy holds none, and nothing is
            // read from disk or the network even if one slipped through.
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    let fonts = fonts();
    opts.font_family = fonts.1.clone();
    opts.fontdb = fonts.0.clone();
    usvg::Tree::from_str(clean, &opts).map_err(|e| format!("the SVG image cannot be drawn: {e}"))
}

/// The system's fonts, read once, for text in the PNG rendering.
fn fonts() -> &'static (Arc<usvg::fontdb::Database>, String) {
    static FONTS: OnceLock<(Arc<usvg::fontdb::Database>, String)> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        let has = |family: &str| {
            db.faces()
                .any(|f| f.families.iter().any(|(name, _)| name == family))
        };
        let pick = |names: &[&str]| names.iter().find(|n| has(n)).map(|n| n.to_string());
        let sans = pick(&["Liberation Sans", "Arial", "DejaVu Sans", "Helvetica"]);
        let serif = pick(&[
            "Liberation Serif",
            "Times New Roman",
            "DejaVu Serif",
            "Times",
        ]);
        let mono = pick(&[
            "Liberation Mono",
            "Courier New",
            "DejaVu Sans Mono",
            "Courier",
        ]);
        if let Some(name) = &sans {
            db.set_sans_serif_family(name.clone());
        }
        if let Some(name) = &serif {
            db.set_serif_family(name.clone());
        }
        if let Some(name) = &mono {
            db.set_monospace_family(name.clone());
        }
        let default = sans.or(serif).unwrap_or_else(|| "sans-serif".to_string());
        (Arc::new(db), default)
    })
}

fn render(tree: &usvg::Tree) -> Result<(Vec<u8>, (u32, u32)), String> {
    let size = tree.size();
    let longest = size.width().max(size.height());
    let scale = (FALLBACK_SIDE / longest).min(FALLBACK_SCALE);
    let w = ((size.width() * scale).ceil() as u32).max(1);
    let h = ((size.height() * scale).ceil() as u32).max(1);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("the SVG image cannot be drawn")?;
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&rgba, w, h, ExtendedColorType::Rgba8)
        .map_err(|e| format!("cannot encode the SVG image's PNG: {e}"))?;
    Ok((out, (w, h)))
}

/// Rebuilds the document from what may stay.
pub fn sanitize(text: &str) -> Result<String, String> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_NODES,
    };
    let doc = roxmltree::Document::parse_with_options(text, opts)
        .map_err(|e| format!("the SVG image cannot be read: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" || root.tag_name().namespace() != Some(SVG_NS) {
        return Err("an SVG image must have an svg root element in the SVG namespace".into());
    }
    let mut out = String::with_capacity(text.len());
    element(root, &mut out, true)?;
    Ok(out)
}

fn element(node: roxmltree::Node, out: &mut String, root: bool) -> Result<(), String> {
    let name = node.tag_name().name();
    out.push('<');
    out.push_str(name);
    if root {
        let _ = write!(out, " xmlns=\"{SVG_NS}\" xmlns:xlink=\"{XLINK_NS}\"");
    }
    for attr in node.attributes() {
        let key = match attr.namespace() {
            None => attr.name().to_string(),
            Some(XLINK_NS) if attr.name() == "href" => "xlink:href".to_string(),
            Some(XML_NS) if matches!(attr.name(), "space" | "lang") => {
                format!("xml:{}", attr.name())
            }
            Some(_) => continue,
        };
        if keep_attribute(name, &key, attr.value())? {
            let _ = write!(out, " {key}=\"{}\"", escape(attr.value(), true));
        }
    }
    out.push('>');
    for child in node.children() {
        match child.node_type() {
            roxmltree::NodeType::Text => {
                let text = child.text().unwrap_or_default();
                if name == "style" && !safe_css(text) {
                    continue;
                }
                out.push_str(&escape(text, false));
            }
            roxmltree::NodeType::Element => {
                let child_name = child.tag_name().name();
                if child.tag_name().namespace() != Some(SVG_NS) {
                    continue;
                }
                if child_name == "a" {
                    out.push_str("<g>");
                    for inner in child.children().filter(|c| c.is_element()) {
                        if inner.tag_name().namespace() == Some(SVG_NS)
                            && ELEMENTS.contains(&inner.tag_name().name())
                        {
                            element(inner, out, false)?;
                        }
                    }
                    out.push_str("</g>");
                } else if ELEMENTS.contains(&child_name) {
                    element(child, out, false)?;
                }
            }
            _ => {}
        }
    }
    let _ = write!(out, "</{name}>");
    Ok(())
}

/// Whether an attribute may stay: no event handlers, and references only to
/// the document itself or, on `image`, to embedded picture data.
fn keep_attribute(element: &str, key: &str, value: &str) -> Result<bool, String> {
    let lower = key.to_ascii_lowercase();
    if lower.starts_with("on") {
        return Ok(false);
    }
    if lower == "href" || lower == "xlink:href" {
        let value = value.trim();
        if value.starts_with('#') {
            return Ok(true);
        }
        if element == "image" && value.len() > 5 && value[..5].eq_ignore_ascii_case("data:") {
            check_embedded(value)?;
            return Ok(true);
        }
        return Ok(false);
    }
    Ok(safe_css(value))
}

/// Whether a value or stylesheet refers only to the document itself.
fn safe_css(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if lower.contains("@import") || lower.contains("expression(") || lower.contains("javascript:") {
        return false;
    }
    lower.match_indices("url(").all(|(at, _)| {
        let rest = lower[at + 4..].trim_start();
        let rest = rest.trim_start_matches(['"', '\'']).trim_start();
        rest.starts_with('#')
    })
}

/// Checks a picture embedded as a data URL before anything decodes it.
fn check_embedded(url: &str) -> Result<(), String> {
    let (head, body) = url
        .split_once(',')
        .ok_or("an embedded picture in the SVG image is not a data URL")?;
    let head = head.to_ascii_lowercase();
    let mime = head
        .trim_start_matches("data:")
        .split(';')
        .next()
        .unwrap_or("");
    let format = match mime {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" | "image/jpg" => image::ImageFormat::Jpeg,
        "image/gif" => image::ImageFormat::Gif,
        _ => {
            return Err("an embedded picture in the SVG image must be PNG, JPEG or GIF data".into())
        }
    };
    if !head.contains(";base64") {
        return Err("an embedded picture in the SVG image must be base64 data".into());
    }
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = super::media::BASE64
        .decode(compact)
        .map_err(|_| "an embedded picture in the SVG image is not valid base64")?;
    let (w, h) = ImageReader::with_format(Cursor::new(&bytes), format)
        .into_dimensions()
        .map_err(|e| format!("an embedded picture in the SVG image cannot be read: {e}"))?;
    if u64::from(w) * u64::from(h) > MAX_EMBEDDED_PIXELS {
        return Err(format!(
            "an embedded picture in the SVG image has more than {MAX_EMBEDDED_PIXELS} pixels"
        ));
    }
    Ok(())
}

fn escape(s: &str, attr: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOT: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"10\">\
                       <rect width=\"20\" height=\"10\" fill=\"#c00\"/></svg>";

    #[test]
    fn keeps_drawing_and_drops_everything_that_reaches_out() {
        let dirty = "<svg xmlns=\"http://www.w3.org/2000/svg\" \
            xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"10\" height=\"10\" onload=\"x()\">\
            <script>alert(1)</script><foreignObject><p>hi</p></foreignObject>\
            <style>@import url(http://evil/a.css);</style>\
            <rect width=\"5\" height=\"5\" fill=\"url(http://evil/p)\" onclick=\"y()\"/>\
            <rect width=\"5\" height=\"5\" fill=\"url(#g)\"/>\
            <image xlink:href=\"file:///etc/passwd\" width=\"1\" height=\"1\"/>\
            <image href=\"https://example.com/a.png\" width=\"1\" height=\"1\"/>\
            <use xlink:href=\"#r\"/><a href=\"javascript:z()\"><circle r=\"2\"/></a>\
            <animate attributeName=\"href\" to=\"javascript:z()\"/></svg>";
        let clean = sanitize(dirty).unwrap();
        for bad in [
            "script",
            "alert",
            "foreignObject",
            "onload",
            "onclick",
            "evil",
            "passwd",
            "example.com",
            "javascript",
            "animate",
            "@import",
        ] {
            assert!(!clean.contains(bad), "{bad} survived: {clean}");
        }
        assert!(clean.contains("fill=\"url(#g)\""), "{clean}");
        assert!(clean.contains("xlink:href=\"#r\""), "{clean}");
        assert!(
            clean.contains("<g><circle r=\"2\"></circle></g>"),
            "{clean}"
        );
    }

    #[test]
    fn refuses_what_is_not_a_plain_svg() {
        assert!(sanitize("<!DOCTYPE svg [<!ENTITY a \"aaaa\">]><svg xmlns=\"http://www.w3.org/2000/svg\">&a;</svg>").is_err());
        assert!(sanitize("<html xmlns=\"http://www.w3.org/1999/xhtml\"/>").is_err());
        assert!(sanitize("<svg>no namespace</svg>").is_err());
        assert!(inspect(&vec![b' '; MAX_SVG_BYTES + 1]).is_err());
        let huge = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100000\" height=\"1\"/>";
        assert!(inspect(huge.as_bytes()).is_err());
    }

    #[test]
    fn an_embedded_picture_is_checked_before_decoding() {
        let bomb = {
            // A PNG header claiming 100,000 by 100,000 pixels.
            let chunk = |png: &mut Vec<u8>, kind: &[u8], data: &[u8]| {
                let mut body = kind.to_vec();
                body.extend_from_slice(data);
                let mut crc = 0xffff_ffffu32;
                for &b in &body {
                    crc ^= u32::from(b);
                    for _ in 0..8 {
                        crc = if crc & 1 != 0 {
                            0xedb8_8320 ^ (crc >> 1)
                        } else {
                            crc >> 1
                        };
                    }
                }
                png.extend_from_slice(&(data.len() as u32).to_be_bytes());
                png.extend_from_slice(&body);
                png.extend_from_slice(&(!crc).to_be_bytes());
            };
            let mut ihdr = 100_000u32.to_be_bytes().to_vec();
            ihdr.extend_from_slice(&100_000u32.to_be_bytes());
            ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
            let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            chunk(&mut png, b"IHDR", &ihdr);
            chunk(&mut png, b"IDAT", &[]);
            chunk(&mut png, b"IEND", &[]);
            base64::engine::general_purpose::STANDARD.encode(png)
        };
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">\
             <image href=\"data:image/png;base64,{bomb}\" width=\"10\" height=\"10\"/></svg>"
        );
        let err = sanitize(&svg).unwrap_err();
        assert!(err.contains("pixels"), "{err}");
    }

    #[test]
    fn draws_a_png_for_readers_without_svg() {
        let svg = inspect(DOT.as_bytes()).unwrap();
        assert_eq!(svg.size, (20.0, 10.0));
        assert_eq!(svg.png_px, (80, 40));
        let img = image::load_from_memory(&svg.png).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(40, 20).0, [204, 0, 0, 255]);
        assert!(sniff(DOT.as_bytes()));
        assert!(sniff(
            format!("\u{feff}<?xml version=\"1.0\"?>\n{DOT}").as_bytes()
        ));
        assert!(!sniff(b"\x89PNG"));
    }
}
