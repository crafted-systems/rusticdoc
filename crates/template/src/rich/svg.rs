//! SVG images: a copy that keeps only drawing, and a PNG of it for readers
//! that do not show SVG.
//!
//! The copy is rebuilt element by element from an allowlist. Scripts, event
//! handlers, animation, foreign content and every reference that leaves the
//! file are dropped, so neither the service nor whoever opens the document
//! loads anything from elsewhere. Embedded pictures stay only as PNG, JPEG or
//! GIF data whose size is checked before anything decodes them.
//!
//! CSS is never filtered by looking for bad substrings, which escapes and
//! comments defeat. `<style>` elements are dropped; a `style` attribute keeps
//! only listed presentation properties, written out as attributes; and every
//! value must be plain: no backslash, `@`, comment or brace, and no function
//! but colours, transforms and `url(#id)`. References are followed before
//! drawing, so `use` and patterns cannot multiply a small file into millions
//! of elements, and the PNG is at most 2000 pixels a side.

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
/// The most elements an SVG image may draw once its references are expanded.
const MAX_EXPANDED: u64 = 1_000_000;
/// The deepest elements may nest.
const MAX_DEPTH: u32 = 256;
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

/// The deepest the elements of `text` nest, read without parsing, so that
/// no parser or walk sees nesting it cannot take. Comments, CDATA,
/// processing instructions and quoted attribute values are skipped; a
/// malformed document may count wrong, and the parser refuses it anyway.
fn nesting(text: &str) -> u32 {
    let b = text.as_bytes();
    let (mut depth, mut deepest) = (0u32, 0u32);
    let mut i = 0;
    let skip_to = |from: usize, end: &[u8]| -> usize {
        b[from..]
            .windows(end.len())
            .position(|w| w == end)
            .map_or(b.len(), |p| from + p + end.len())
    };
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            i = skip_to(i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = skip_to(i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = skip_to(i + 2, b"?>");
        } else if rest.starts_with(b"<!") {
            i = skip_to(i + 2, b">");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = skip_to(i + 2, b">");
        } else {
            // A start tag: find its end outside quotes.
            let mut j = i + 1;
            let mut quote = None;
            while j < b.len() {
                match (quote, b[j]) {
                    (None, b'"' | b'\'') => quote = Some(b[j]),
                    (Some(q), c) if c == q => quote = None,
                    (None, b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            if j > i && b.get(j - 1) != Some(&b'/') {
                depth += 1;
                deepest = deepest.max(depth);
            }
            i = j + 1;
        }
    }
    deepest
}

/// Rebuilds the document from what may stay.
pub fn sanitize(text: &str) -> Result<String, String> {
    if nesting(text) > MAX_DEPTH {
        return Err(format!(
            "the SVG image nests elements more than {MAX_DEPTH} deep"
        ));
    }

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
    check_structure(&doc)?;
    check_expansion(&doc)?;
    let mut out = String::with_capacity(text.len());
    element(root, &mut out, true)?;
    Ok(out)
}

/// The ids an element refers to: its `href` and every `url(#id)`, in any
/// case, as [`plain_value`] accepts it.
fn references<'a>(node: roxmltree::Node<'a, 'a>) -> Vec<&'a str> {
    let mut out = Vec::new();
    for attr in node.attributes() {
        let value = attr.value().trim();
        if attr.name() == "href" {
            if let Some(id) = value.strip_prefix('#') {
                out.push(id);
            }
            continue;
        }
        // ASCII lower-casing keeps every byte where it was.
        let lower = value.to_ascii_lowercase();
        let mut from = 0;
        while let Some(at) = lower[from..].find("url(").map(|p| p + from) {
            let start = at + 4;
            let end = value[start..].find(')').map_or(value.len(), |p| p + start);
            let inner = value[start..end].trim().trim_matches(['"', '\'']);
            if let Some(id) = inner.strip_prefix('#') {
                out.push(id);
            }
            from = end;
        }
    }
    out
}

/// Refuses elements nested deeper than [`MAX_DEPTH`], which the recursive
/// walks here and in the renderer could not take, and ids used twice, whose
/// references a renderer may resolve to either element.
fn check_structure(doc: &roxmltree::Document) -> Result<(), String> {
    use std::collections::HashSet;
    let mut depth = vec![0u32; doc.descendants().count() + 1];
    let mut seen = HashSet::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        let parent = node.parent().map_or(0, |p| depth[p.id().get_usize()]);
        let mine = parent + 1;
        if mine > MAX_DEPTH {
            return Err(format!(
                "the SVG image nests elements more than {MAX_DEPTH} deep"
            ));
        }
        depth[node.id().get_usize()] = mine;
        if let Some(id) = node.attribute("id") {
            if !seen.insert(id) {
                return Err(format!("the SVG image has two elements with the id {id:?}"));
            }
        }
    }
    Ok(())
}

/// Refuses a document whose references expand it past [`MAX_EXPANDED`]
/// elements once every `use`, pattern, marker, filter, mask and clip path is
/// drawn where it is referred to, as a renderer would. The walk keeps its own
/// stack, so a long chain of references cannot exhaust the thread's.
fn check_expansion(doc: &roxmltree::Document) -> Result<(), String> {
    use roxmltree::NodeId;
    use std::collections::{HashMap, HashSet};
    let ids: HashMap<&str, NodeId> = doc
        .descendants()
        .filter(|n| n.is_element())
        .filter_map(|n| Some((n.attribute("id")?, n.id())))
        .collect();
    let deps = |id: NodeId| -> Vec<NodeId> {
        let Some(node) = doc.get_node(id) else {
            return Vec::new();
        };
        let mut out: Vec<NodeId> = node
            .children()
            .filter(|c| c.is_element())
            .map(|c| c.id())
            .collect();
        out.extend(
            references(node)
                .into_iter()
                .filter_map(|r| ids.get(r).copied()),
        );
        out
    };
    let mut memo: HashMap<NodeId, u64> = HashMap::new();
    // On the current path: a reference back to one is a loop, which
    // renderers refuse to draw, so it adds nothing.
    let mut active: HashSet<NodeId> = HashSet::new();
    let mut stack: Vec<(NodeId, Option<Vec<NodeId>>)> = vec![(doc.root_element().id(), None)];
    while let Some((id, done)) = stack.pop() {
        match done {
            None => {
                if memo.contains_key(&id) || !active.insert(id) {
                    continue;
                }
                let next = deps(id);
                let pending: Vec<NodeId> = next
                    .iter()
                    .copied()
                    .filter(|d| !memo.contains_key(d) && !active.contains(d))
                    .collect();
                stack.push((id, Some(next)));
                stack.extend(pending.into_iter().map(|d| (d, None)));
            }
            Some(next) => {
                let total = next
                    .iter()
                    .map(|d| memo.get(d).copied().unwrap_or(0))
                    .fold(1u64, u64::saturating_add)
                    .min(MAX_EXPANDED + 1);
                if total > MAX_EXPANDED {
                    return Err(format!(
                        "the SVG image expands to more than {MAX_EXPANDED} elements through \
                         its references"
                    ));
                }
                active.remove(&id);
                memo.insert(id, total);
            }
        }
    }
    Ok(())
}

fn element(node: roxmltree::Node, out: &mut String, root: bool) -> Result<(), String> {
    let name = node.tag_name().name();
    out.push('<');
    out.push_str(name);
    if root {
        let _ = write!(out, " xmlns=\"{SVG_NS}\" xmlns:xlink=\"{XLINK_NS}\"");
    }
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut styled: Vec<(String, String)> = Vec::new();
    for attr in node.attributes() {
        let key = match attr.namespace() {
            None => attr.name().to_string(),
            Some(XLINK_NS) if attr.name() == "href" => "xlink:href".to_string(),
            Some(XML_NS) if matches!(attr.name(), "space" | "lang") => {
                format!("xml:{}", attr.name())
            }
            Some(_) => continue,
        };
        if key == "style" {
            styled = style_declarations(attr.value());
            continue;
        }
        if keep_attribute(name, &key, attr.value())? {
            attrs.push((key, attr.value().to_string()));
        }
    }
    // Declarations from `style` win over presentation attributes, as in CSS.
    for (key, value) in styled {
        attrs.retain(|(k, _)| *k != key);
        attrs.push((key, value));
    }
    for (key, value) in &attrs {
        let _ = write!(out, " {key}=\"{}\"", escape(value, true));
    }
    out.push('>');
    for child in node.children() {
        match child.node_type() {
            roxmltree::NodeType::Text => {
                out.push_str(&escape(child.text().unwrap_or_default(), false));
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

/// Properties a `style` attribute may set, which become attributes.
const STYLE_PROPERTIES: &[&str] = &[
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "stop-color",
    "stop-opacity",
    "color",
    "display",
    "visibility",
    "clip-path",
    "clip-rule",
    "mask",
    "filter",
    "marker-start",
    "marker-mid",
    "marker-end",
    "paint-order",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "font-variant",
    "text-anchor",
    "dominant-baseline",
    "letter-spacing",
    "word-spacing",
    "text-decoration",
    "flood-color",
    "flood-opacity",
    "lighting-color",
];

/// The declarations of a `style` attribute that may stay, as attributes.
/// Anything else, and any value that is not plainly safe, is dropped.
fn style_declarations(style: &str) -> Vec<(String, String)> {
    style
        .split(';')
        .filter_map(|decl| {
            let (prop, value) = decl.split_once(':')?;
            let prop = prop.trim().to_ascii_lowercase();
            let value = value.trim();
            (STYLE_PROPERTIES.contains(&prop.as_str()) && plain_value(value))
                .then(|| (prop, value.to_string()))
        })
        .collect()
}

/// Functions a value may call.
const FUNCTIONS: &[&str] = &[
    "url",
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "matrix",
    "translate",
    "scale",
    "rotate",
    "skewx",
    "skewy",
];

/// Whether a value is plainly safe: no escapes, no at-rules or comments, and
/// no function but colours, transforms and `url(#id)`.
fn plain_value(value: &str) -> bool {
    if value.contains(['\\', '@', '<', '>', '{', '}']) || value.contains("/*") {
        return false;
    }
    let bytes = value.as_bytes();
    let mut i = 0;
    while let Some(open) = value[i..].find('(').map(|p| p + i) {
        let start = value[..open]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .map_or(0, |p| p + 1);
        let name = value[start..open].to_ascii_lowercase();
        if !FUNCTIONS.contains(&name.as_str()) {
            return false;
        }
        let Some(close) = value[open..].find(')').map(|p| p + open) else {
            return false;
        };
        let inner = &value[open + 1..close];
        if name == "url" {
            let id = inner.trim().trim_matches(['"', '\'']);
            let ok = id.strip_prefix('#').is_some_and(|id| {
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
            });
            if !ok {
                return false;
            }
        } else if inner.contains('(') {
            return false;
        }
        i = close + 1;
        if i >= bytes.len() {
            break;
        }
    }
    true
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
        let data = value
            .as_bytes()
            .get(..5)
            .is_some_and(|p| p.eq_ignore_ascii_case(b"data:"));
        if element == "image" && data {
            check_embedded(value)?;
            return Ok(true);
        }
        return Ok(false);
    }
    Ok(plain_value(value))
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
    fn css_tricks_do_not_reach_out() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">\
            <style>rect { fill: u\\rl(http://evil/a) }</style>\
            <rect fill=\"\\75 rl(http://evil/b)\" width=\"1\" height=\"1\"/>\
            <rect fill=\"u\\rl(http://evil/c)\" width=\"1\" height=\"1\"/>\
            <rect fill=\"image-set(&quot;http://evil/d&quot; 1x)\" width=\"1\" height=\"1\"/>\
            <rect fill=\"url(/**/http://evil/e)\" width=\"1\" height=\"1\"/>\
            <rect fill=\"url( 'http://evil/f' )\" width=\"1\" height=\"1\"/>\
            <rect style=\"fill:url(http://evil/g);stroke:#c00;behavior:url(x)\" width=\"1\" height=\"1\"/>\
            <rect style=\"fill:#0c0;background:url(http://evil/h)\" fill=\"#00c\" width=\"1\" height=\"1\"/>\
            <rect style=\"fill:url(#ok)\" width=\"1\" height=\"1\"/></svg>";
        let clean = sanitize(svg).unwrap();
        assert!(!clean.contains("evil"), "{clean}");
        assert!(!clean.contains("<style"), "{clean}");
        assert!(!clean.contains("style="), "{clean}");
        assert!(clean.contains("stroke=\"#c00\""), "{clean}");
        // A style declaration wins over the attribute it repeats.
        assert!(
            clean.contains("fill=\"#0c0\"") && !clean.contains("#00c"),
            "{clean}"
        );
        assert!(clean.contains("fill=\"url(#ok)\""), "{clean}");
        for value in ["rgb(1,2,3)", "#fff", "none", "matrix(1 0 0 1 2 3)"] {
            assert!(plain_value(value), "{value}");
        }
        for value in [
            "url(#a b)",
            "var(--x)",
            "rgb(url(#a))",
            "url(#a",
            "a\\b",
            "@x",
            "x/*y*/",
        ] {
            assert!(!plain_value(value), "{value}");
        }
    }

    #[test]
    fn a_multibyte_href_is_refused_not_a_crash() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">\
                   <image href=\"dat\u{e9}:x\" width=\"1\" height=\"1\"/>\
                   <image href=\"\u{1f600}\u{1f600}\" width=\"1\" height=\"1\"/></svg>";
        let clean = sanitize(svg).unwrap();
        assert!(!clean.contains("href"), "{clean}");
    }

    #[test]
    fn references_cannot_multiply_the_drawing() {
        // Each level draws the one below ten times: 10^7 rectangles from a
        // file of a few hundred bytes.
        let mut svg = String::from(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" \
             xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"10\" height=\"10\"><defs>\
             <rect id=\"l0\" width=\"1\" height=\"1\"/>",
        );
        for level in 1..=7 {
            svg.push_str(&format!("<g id=\"l{level}\">"));
            for _ in 0..10 {
                svg.push_str(&format!("<use xlink:href=\"#l{}\"/>", level - 1));
            }
            svg.push_str("</g>");
        }
        svg.push_str("</defs><use xlink:href=\"#l7\"/></svg>");
        let err = sanitize(&svg).unwrap_err();
        assert!(err.contains("expands to more than"), "{err}");
        // The same through patterns filled by url(#id).
        let mut svg = String::from(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><defs>\
             <pattern id=\"p0\"><rect width=\"1\" height=\"1\"/></pattern>",
        );
        for level in 1..=7 {
            svg.push_str(&format!("<pattern id=\"p{level}\">"));
            for _ in 0..10 {
                svg.push_str(&format!("<rect fill=\"url(#p{})\"/>", level - 1));
            }
            svg.push_str("</pattern>");
        }
        svg.push_str("</defs><rect fill=\"url(#p7)\"/></svg>");
        assert!(sanitize(&svg).is_err());
        // A loop is not an expansion.
        let looped = "<svg xmlns=\"http://www.w3.org/2000/svg\" \
            xmlns:xlink=\"http://www.w3.org/1999/xlink\"><g id=\"a\"><use xlink:href=\"#a\"/></g></svg>";
        assert!(sanitize(looped).is_ok());
    }

    #[test]
    fn deep_nesting_and_long_chains_are_refused_not_a_crash() {
        let open = "<svg xmlns=\"http://www.w3.org/2000/svg\" \
                    xmlns:xlink=\"http://www.w3.org/1999/xlink\">";
        // Nesting far past what a recursive walk takes.
        let deep = format!(
            "{open}{}{}</svg>",
            "<g>".repeat(90_000),
            "</g>".repeat(90_000)
        );
        let err = sanitize(&deep).unwrap_err();
        assert!(err.contains("nests elements more than"), "{err}");
        let fine = format!("{open}{}{}</svg>", "<g>".repeat(200), "</g>".repeat(200));
        assert!(sanitize(&fine).is_ok());
        // A chain of 60,000 references, each to the next.
        let mut chain = String::from(open);
        chain.push_str("<defs>");
        for i in 0..60_000 {
            chain.push_str(&format!("<use id=\"u{i}\" xlink:href=\"#u{}\"/>", i + 1));
        }
        chain.push_str(
            "<rect id=\"u60000\" width=\"1\" height=\"1\"/></defs><use xlink:href=\"#u0\"/></svg>",
        );
        // Walked without recursion. Each use counts what it points at, and
        // defs count too, so this many is refused as an expansion rather
        // than overflowing the stack.
        match sanitize(&chain) {
            Ok(_) => {}
            Err(e) => assert!(e.contains("expands to more than"), "{e}"),
        }
    }

    #[test]
    fn duplicate_ids_and_upper_case_urls_cannot_hide_an_expansion() {
        let dup = "<svg xmlns=\"http://www.w3.org/2000/svg\">\
                   <rect id=\"a\"/><circle id=\"a\"/></svg>";
        let err = sanitize(dup).unwrap_err();
        assert!(err.contains("two elements with the id \"a\""), "{err}");
        let mut svg = String::from(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><defs>\
             <pattern id=\"p0\"><rect width=\"1\" height=\"1\"/></pattern>",
        );
        for level in 1..=7 {
            svg.push_str(&format!("<pattern id=\"p{level}\">"));
            for _ in 0..10 {
                svg.push_str(&format!("<rect fill=\"URL(#p{})\"/>", level - 1));
            }
            svg.push_str("</pattern>");
        }
        svg.push_str("</defs><rect fill=\"Url( #p7 )\"/></svg>");
        let err = sanitize(&svg).unwrap_err();
        assert!(err.contains("expands to more than"), "{err}");
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
