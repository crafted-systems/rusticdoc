//! An HTML page and the files it names, packed into one file: a page with its
//! images as `data:` URIs, or an email with them attached by `cid:`.
//!
//! LibreOffice writes the images of an HTML export as files beside the page,
//! and an HTML template names its images and stylesheets as assets. Either way
//! the page alone shows broken images. [`pack`] puts them inside it.
//!
//! An email is a MIME message, `multipart/alternative` with a plain-text part
//! made from the page and the page itself, which is `multipart/related` with
//! its images when it has any. Email clients drop `data:` images, so the email
//! carries them as parts named by `Content-ID`. It has a `Subject` from the
//! page's `<title>` and no `From`, `To` or `Date`: the sender adds those.
//!
//! Only names the page uses are looked up, and only among the files given, so
//! a reference such as `../secret.png` or `file:///etc/passwd` finds nothing
//! and is left as written, as is any `https:` URL.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::LazyLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use regex::bytes::{Captures, Regex};

use crate::error::{Error, Result};

/// What [`pack`] makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packed {
    /// One HTML file, its images as `data:` URIs.
    Html,
    /// One MIME message, its images as `cid:` parts.
    Eml,
}

/// The files a page may name, by name.
pub type Files = BTreeMap<String, Bytes>;

/// An image attribute or a CSS `url()`, with the name in one of three groups.
static IMAGE_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(?P<pre>\b(?:src|background)\s*=\s*|url\(\s*)(?:"(?P<dq>[^"]*)"|'(?P<sq>[^']*)'|(?P<bare>[^\s"'<>()]+))"#,
    )
    .expect("a valid pattern")
});

/// A `<link>` tag, to find the stylesheets a page links to.
static LINK_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<link\b[^>]*>").expect("a valid pattern"));

static HREF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'<>]+))"#).expect("a valid pattern")
});

static HEAD_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</head\s*>").expect("a valid pattern"));

static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title\b[^>]*>(.*?)</title\s*>").expect("a valid pattern"));

/// The file name a reference means: entities a template writes for a slash
/// or an ampersand decoded, and a leading `./` dropped.
fn file_name(raw: &[u8]) -> Option<String> {
    let raw = std::str::from_utf8(raw).ok()?.trim();
    static ENTITY: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new("(?i)&(?:#x0*2f|#0*47|sol);|&amp;").expect("a valid pattern")
    });
    let name = ENTITY.replace_all(raw, |c: &regex::Captures| {
        if c[0].eq_ignore_ascii_case("&amp;") {
            "&"
        } else {
            "/"
        }
    });
    let name = name.strip_prefix("./").unwrap_or(&name);
    (!name.is_empty()).then(|| name.to_string())
}

fn captured<'a>(c: &Captures<'a>) -> &'a [u8] {
    c.name("dq")
        .or_else(|| c.name("sq"))
        .or_else(|| c.name("bare"))
        .map_or(&[][..], |m| m.as_bytes())
}

/// The images a page or stylesheet names, as file names.
pub fn image_names(text: &[u8]) -> BTreeSet<String> {
    IMAGE_REF
        .captures_iter(text)
        .filter_map(|c| file_name(captured(&c)))
        .collect()
}

/// The stylesheets a page links to, as file names.
pub fn stylesheet_names(html: &[u8]) -> Vec<String> {
    LINK_TAG
        .find_iter(html)
        .filter(|tag| is_stylesheet_link(tag.as_bytes()))
        .filter_map(|tag| href(tag.as_bytes()))
        .collect()
}

fn is_stylesheet_link(tag: &[u8]) -> bool {
    String::from_utf8_lossy(tag)
        .to_ascii_lowercase()
        .contains("stylesheet")
}

fn href(tag: &[u8]) -> Option<String> {
    let c = HREF.captures(tag)?;
    let m = c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3))?;
    file_name(m.as_bytes())
}

/// The content type of an image, by its extension.
fn image_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// Packs `html` with `files`. A stylesheet the page links to and `files` holds
/// becomes a `<style>` in its place, and each of `sheets` a `<style>` at the
/// end of the head, as Fulgur applies every stylesheet sent. Then each image
/// the page or its styles name that `files` holds is put inside, by `data:`
/// URI or `cid:` part. Names `files` doesn't hold are left as written.
///
/// A page can name one large image or stylesheet many times, and each `data:`
/// URI or `<style>` is a copy, so what is made is counted as it is made and
/// refused once it would pass `limit` bytes, before it is all in memory.
pub fn pack(
    html: &[u8],
    files: &Files,
    sheets: &[Bytes],
    to: Packed,
    limit: usize,
) -> Result<Vec<u8>> {
    let mut budget = Budget::new(limit, html.len());
    let html = with_stylesheets(html, files, sheets, &mut budget);
    budget.check()?;
    let out = match to {
        Packed::Html => inline_images(&html, files, &mut budget),
        Packed::Eml => email(&html, files, &mut budget),
    };
    budget.check()?;
    Ok(out)
}

/// The bytes packing may still make. It counts what it adds and not what that
/// replaces, so it errs high.
struct Budget {
    limit: usize,
    used: usize,
}

impl Budget {
    fn new(limit: usize, start: usize) -> Self {
        Budget { limit, used: start }
    }

    /// Counts `bytes` more, and says whether they fit. Once over, nothing fits.
    fn take(&mut self, bytes: usize) -> bool {
        if self.used > self.limit {
            return false;
        }
        self.used = self.used.saturating_add(bytes);
        self.used <= self.limit
    }

    fn check(&self) -> Result<()> {
        if self.used > self.limit {
            return Err(Error::InvalidRequest(format!(
                "the output would pass the limit of {} bytes for one request",
                self.limit
            )));
        }
        Ok(())
    }
}

fn with_stylesheets(html: &[u8], files: &Files, sheets: &[Bytes], budget: &mut Budget) -> Vec<u8> {
    let style = |css: &[u8]| {
        let mut out = b"<style>\n".to_vec();
        // A stylesheet can't close the element it's put in.
        out.extend_from_slice(&replace_ci(css, b"</style", b"<\\/style"));
        out.extend_from_slice(b"\n</style>");
        out
    };
    // Each sheet is made once, however often it is linked.
    let mut made: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut out = LINK_TAG
        .replace_all(html, |c: &Captures| {
            let tag = &c[0];
            if is_stylesheet_link(tag)
                && let Some(name) = href(tag)
                    .filter(|name| name.to_ascii_lowercase().ends_with(".css"))
                    .filter(|name| files.contains_key(name))
            {
                let styled = made
                    .entry(name)
                    .or_insert_with_key(|name| style(&files[name]));
                if budget.take(styled.len()) {
                    return styled.clone();
                }
            }
            tag.to_vec()
        })
        .into_owned();
    if !sheets.is_empty() {
        let mut extra = Vec::new();
        for css in sheets {
            let styled = style(css);
            if !budget.take(styled.len()) {
                return out;
            }
            extra.extend_from_slice(&styled);
        }
        out = match HEAD_END.find(&out) {
            Some(m) => [&out[..m.start()], &extra, &out[m.start()..]].concat(),
            None => [extra, out].concat(),
        };
    }
    out
}

fn replace_ci(haystack: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    let pattern = format!(
        "(?i){}",
        regex::escape(std::str::from_utf8(needle).unwrap_or(""))
    );
    Regex::new(&pattern)
        .expect("an escaped pattern")
        .replace_all(haystack, with)
        .into_owned()
}

/// Rewrites every image reference `files` holds with `to(name)`, while what it
/// adds fits `budget`.
fn rewrite_images(
    html: &[u8],
    files: &Files,
    budget: &mut Budget,
    mut to: impl FnMut(&str) -> std::rc::Rc<str>,
) -> Vec<u8> {
    IMAGE_REF
        .replace_all(html, |c: &Captures| match file_name(captured(c)) {
            Some(name) if files.contains_key(&name) => {
                let with = to(&name);
                if !budget.take(c["pre"].len() + with.len() + 2) {
                    return c[0].to_vec();
                }
                // The reference's own quotes, so one inside a quoted `style`
                // attribute keeps it whole. A `data:` URI or Content-ID needs
                // none in `url()`, and gets double quotes as an attribute.
                let quote: &[u8] = if c.name("dq").is_some() {
                    b"\""
                } else if c.name("sq").is_some() {
                    b"'"
                } else if c["pre"].to_ascii_lowercase().starts_with(b"url") {
                    b""
                } else {
                    b"\""
                };
                let mut out = c["pre"].to_vec();
                out.extend_from_slice(quote);
                out.extend_from_slice(with.as_bytes());
                out.extend_from_slice(quote);
                out
            }
            _ => c[0].to_vec(),
        })
        .into_owned()
}

fn inline_images(html: &[u8], files: &Files, budget: &mut Budget) -> Vec<u8> {
    // Each image is encoded once, however often it is named.
    let mut uris: BTreeMap<String, std::rc::Rc<str>> = BTreeMap::new();
    rewrite_images(html, files, budget, |name| {
        uris.entry(name.to_string())
            .or_insert_with(|| {
                format!(
                    "data:{};base64,{}",
                    image_type(name),
                    STANDARD.encode(&files[name])
                )
                .into()
            })
            .clone()
    })
}

/// The size of `bytes` as a base64 body in 76-column lines.
fn base64_size(bytes: usize) -> usize {
    let encoded = bytes.div_ceil(3) * 4;
    encoded + encoded.div_ceil(76) * 2
}

/// A Content-ID for each name, unique within the message.
fn content_ids(names: &BTreeSet<String>) -> BTreeMap<String, String> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let safe: String = name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || ".-_".contains(c) {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            (name.clone(), format!("{}.{safe}@rusticdoc", i + 1))
        })
        .collect()
}

fn email(html: &[u8], files: &Files, budget: &mut Budget) -> Vec<u8> {
    let used: BTreeSet<String> = image_names(html)
        .into_iter()
        .filter(|n| files.contains_key(n))
        .collect();
    let ids = content_ids(&used);
    let page = rewrite_images(html, files, budget, |name| {
        format!("cid:{}", ids[name]).into()
    });
    let text = to_text(&String::from_utf8_lossy(html));
    // Each image is attached once. The page, its text and the images are all
    // base64 in the message, so they are counted again as that.
    let parts = [page.len(), text.len()]
        .into_iter()
        .chain(used.iter().map(|n| files[n].len()));
    for size in parts {
        if !budget.take(base64_size(size)) {
            return Vec::new();
        }
    }
    let subject = TITLE
        .captures(html)
        .map(|c| decode_entities(&collapse_space(&String::from_utf8_lossy(&c[1]))))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let tag = uuid::Uuid::new_v4().simple().to_string();
    let outer = format!("=_alt_{tag}");
    let inner = format!("=_rel_{tag}");
    let mut m = String::new();
    m.push_str("MIME-Version: 1.0\r\n");
    if let Some(subject) = subject {
        let _ = write!(m, "Subject: {}\r\n", encode_header(&subject));
    }
    // Opens as a draft, ready to address, in clients that know it.
    m.push_str("X-Unsent: 1\r\n");
    let _ = write!(
        m,
        "Content-Type: multipart/alternative; boundary=\"{outer}\"\r\n\r\n"
    );
    let _ = write!(m, "--{outer}\r\n");
    part(&mut m, "text/plain; charset=utf-8", &[], text.as_bytes());
    let _ = write!(m, "--{outer}\r\n");
    if used.is_empty() {
        part(&mut m, "text/html; charset=utf-8", &[], &page);
    } else {
        let _ = write!(
            m,
            "Content-Type: multipart/related; type=\"text/html\"; boundary=\"{inner}\"\r\n\r\n"
        );
        let _ = write!(m, "--{inner}\r\n");
        part(&mut m, "text/html; charset=utf-8", &[], &page);
        for name in &used {
            let _ = write!(m, "--{inner}\r\n");
            let file = quoted_param(name.rsplit('/').next().unwrap_or(name));
            part(
                &mut m,
                image_type(name),
                &[
                    format!("Content-ID: <{}>", ids[name]),
                    format!("Content-Disposition: inline; filename=\"{file}\""),
                ],
                &files[name],
            );
        }
        let _ = write!(m, "--{inner}--\r\n");
    }
    let _ = write!(m, "--{outer}--\r\n");
    m.into_bytes()
}

/// One base64 part: headers, a blank line and the body in 76-column lines.
fn part(m: &mut String, content_type: &str, headers: &[String], body: &[u8]) {
    let _ = write!(m, "Content-Type: {content_type}\r\n");
    m.push_str("Content-Transfer-Encoding: base64\r\n");
    for h in headers {
        let _ = write!(m, "{h}\r\n");
    }
    m.push_str("\r\n");
    let encoded = STANDARD.encode(body);
    for line in encoded.as_bytes().chunks(76) {
        m.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        m.push_str("\r\n");
    }
}

/// A file name fit for a quoted MIME parameter.
fn quoted_param(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii() && !c.is_ascii_control() && *c != '"' && *c != '\\')
        .collect()
}

/// A header value: plain when it is short printable ASCII, otherwise RFC 2047
/// encoded words, folded. Line breaks can't get through either way.
fn encode_header(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if value.is_ascii() && value.len() <= 70 {
        return value;
    }
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in value.chars() {
        if chunk.len() + c.len_utf8() > 45 {
            words.push(std::mem::take(&mut chunk));
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        words.push(chunk);
    }
    words
        .iter()
        .map(|w| format!("=?UTF-8?B?{}?=", STANDARD.encode(w)))
        .collect::<Vec<_>>()
        .join("\r\n ")
}

fn collapse_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decodes character references: numeric ones and the named ones pages use.
fn decode_entities(s: &str) -> String {
    static REF: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"&(#[xX][0-9a-fA-F]{1,6}|#[0-9]{1,7}|[a-zA-Z]{2,8});")
            .expect("a valid pattern")
    });
    REF.replace_all(s, |c: &regex::Captures| {
        let body = &c[1];
        let ch = if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        } else if let Some(dec) = body.strip_prefix('#') {
            dec.parse().ok().and_then(char::from_u32)
        } else {
            match body {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                "copy" => Some('©'),
                "reg" => Some('®'),
                "euro" => Some('€'),
                "pound" => Some('£'),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                "hellip" => Some('…'),
                "lsquo" => Some('‘'),
                "rsquo" => Some('’'),
                "ldquo" => Some('“'),
                "rdquo" => Some('”'),
                _ => None,
            }
        };
        ch.map_or_else(|| c[0].to_string(), |ch| ch.to_string())
    })
    .into_owned()
}

/// The text of a page, for the plain-text part of an email: paragraphs and
/// rows on their own lines, cells apart by a tab, and links followed by
/// where they go.
pub fn to_text(html: &str) -> String {
    static DROP: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?is)<!--.*?-->|<(head|style|script|title)\b.*?</(head|style|script|title)\s*>",
        )
        .expect("a valid pattern")
    });
    static LINK: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r#"(?is)<a\b[^>]*?\bhref\s*=\s*(?:"([^"]*)"|'([^']*)')[^>]*>(.*?)</a\s*>"#,
        )
        .expect("a valid pattern")
    });
    static BREAK: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)<br\b[^>]*>|<tr\b[^>]*>|</?(?:p|div|h[1-6]|table|ul|ol|blockquote|pre|section|article|header|footer)\b[^>]*>",
        )
        .expect("a valid pattern")
    });
    static CELL_PARA_OPEN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)(<t[dh]\b[^>]*>)\s*(?:<p\b[^>]*>\s*)?").expect("a valid pattern")
    });
    static CELL_PARA_CLOSE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)</p\s*>\s*(</t[dh]\s*>)").expect("a valid pattern")
    });
    static ITEM: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)<li\b[^>]*>").expect("a valid pattern"));
    static CELL: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)\s*</t[dh]\s*>\s*").expect("a valid pattern"));
    static TAG: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?s)<[^>]*>").expect("a valid pattern"));
    static GAPS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\n{3,}").expect("a valid pattern"));

    let s = DROP.replace_all(html, "");
    let s = collapse_space(&s);
    let s = LINK.replace_all(&s, |c: &regex::Captures| {
        let url = c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str());
        let label = &c[3];
        let shown = decode_entities(TAG.replace_all(label, "").trim());
        let url = decode_entities(url.trim());
        let lower = url.to_ascii_lowercase();
        let linkable = ["http://", "https://", "mailto:"]
            .iter()
            .any(|p| lower.starts_with(p));
        if linkable && shown != url && !url.ends_with(&shown) {
            format!("{label} ({url})")
        } else {
            label.to_string()
        }
    });
    // LibreOffice puts each cell's text in a paragraph; the cell is the unit.
    let s = CELL_PARA_OPEN.replace_all(&s, "$1");
    let s = CELL_PARA_CLOSE.replace_all(&s, "$1");
    let s = BREAK.replace_all(&s, "\n");
    let s = ITEM.replace_all(&s, "\n- ");
    let s = CELL.replace_all(&s, "\t");
    let s = TAG.replace_all(&s, "");
    let s = decode_entities(&s);
    let lines: Vec<&str> = s.lines().map(|l| l.trim_matches([' ', '\t'])).collect();
    let joined = lines.join("\n");
    let s = GAPS.replace_all(&joined, "\n\n");
    let mut out = s.trim().replace('\n', "\r\n");
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(html: &[u8], files: &Files, sheets: &[Bytes], to: Packed) -> Vec<u8> {
        pack(html, files, sheets, to, usize::MAX).unwrap()
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

    fn files(entries: &[(&str, &[u8])]) -> Files {
        entries
            .iter()
            .map(|(n, b)| (n.to_string(), Bytes::copy_from_slice(b)))
            .collect()
    }

    fn html(packed: Vec<u8>) -> String {
        String::from_utf8(packed).unwrap()
    }

    #[test]
    fn images_beside_the_page_go_inside_it() {
        let page =
            br#"<p><img src="out_html_1.png" alt="Logo"/><img src='x.gif'><img src=y.png></p>"#;
        let f = files(&[
            ("out_html_1.png", PNG),
            ("x.gif", b"GIF89a"),
            ("y.png", PNG),
        ]);
        let out = html(packed(page, &f, &[], Packed::Html));
        let png = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
        assert!(
            out.contains(&format!(r#"<img src="{png}" alt="Logo"/>"#)),
            "{out}"
        );
        assert!(out.contains("data:image/gif;base64,"), "{out}");
        assert!(
            !out.contains("out_html_1.png") && !out.contains("y.png"),
            "{out}"
        );
    }

    #[test]
    fn names_it_does_not_hold_stay_as_written() {
        let page = br#"<img src="https://example.com/a.png"><img src="../secret.png"><img src="file:///etc/passwd">"#;
        let f = files(&[("secret.png", PNG), ("passwd", b"root")]);
        assert_eq!(html(packed(page, &f, &[], Packed::Html)).as_bytes(), page);
    }

    #[test]
    fn backgrounds_and_css_urls_are_inlined() {
        let page = br#"<body background="bg.png"><div style="background: url( 'bg.png' )"></div><style>a{background:url(bg.png)}</style>"#;
        let out = html(packed(page, &files(&[("bg.png", PNG)]), &[], Packed::Html));
        assert_eq!(out.matches("data:image/png;base64,").count(), 3, "{out}");
    }

    #[test]
    fn a_url_in_a_style_attribute_keeps_the_attribute_whole() {
        let page = br#"<div style="background: url('bg.png')"></div><div style='background:url("bg.png")'></div><div style="background:url(bg.png)"></div>"#;
        let out = html(packed(page, &files(&[("bg.png", PNG)]), &[], Packed::Html));
        let data = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
        assert!(
            out.contains(&format!(r#"<div style="background: url('{data}')"></div>"#)),
            "{out}"
        );
        assert!(
            out.contains(&format!(r#"<div style='background:url("{data}")'></div>"#)),
            "{out}"
        );
        assert!(
            out.contains(&format!(r#"<div style="background:url({data})"></div>"#)),
            "{out}"
        );
    }

    #[test]
    fn escaped_slashes_and_dot_slash_find_the_file() {
        let page = br#"<img src="brand&#x2f;logo.png"><img src="./brand/logo.png">"#;
        let out = html(packed(
            page,
            &files(&[("brand/logo.png", PNG)]),
            &[],
            Packed::Html,
        ));
        assert_eq!(out.matches("data:image/png").count(), 2, "{out}");
    }

    #[test]
    fn linked_and_sent_stylesheets_become_style_elements() {
        let page = br#"<html><head><link rel="stylesheet" href="site.css"><link rel="icon" href="site.css"></head><body></body></html>"#;
        let f = files(&[
            ("site.css", b"h1{color:red;background:url(dot.png)}"),
            ("dot.png", PNG),
        ]);
        let extra = [Bytes::from_static(b"p{margin:0}</STYLE><script>")];
        let out = html(packed(page, &f, &extra, Packed::Html));
        assert!(
            out.contains("<style>\nh1{color:red;background:url(data:image/png;base64,"),
            "{out}"
        );
        assert!(
            out.contains(r#"<link rel="icon" href="site.css">"#),
            "{out}"
        );
        assert!(
            out.contains("p{margin:0}<\\/style><script>\n</style></head>"),
            "{out}"
        );
        assert!(!out.contains("</STYLE>"), "{out}");
    }

    #[test]
    fn an_email_has_text_html_and_its_images_by_cid() {
        let page = "<html><head><title>Invoice &amp; receipt</title><style>p{}</style></head>\
            <body><p>Dear Ada,</p><p>Total:\n 1&nbsp;500</p><img src=\"logo.png\">\
            <img src=\"logo.png\"><p><a href=\"https://pay.example.com/x\">Pay now</a></p></body></html>";
        let out = html(packed(
            page.as_bytes(),
            &files(&[("logo.png", PNG), ("unused.png", PNG)]),
            &[],
            Packed::Eml,
        ));
        assert!(
            out.starts_with("MIME-Version: 1.0\r\nSubject: Invoice & receipt\r\nX-Unsent: 1\r\n"),
            "{out}"
        );
        assert!(
            out.contains("Content-Type: multipart/alternative;"),
            "{out}"
        );
        assert!(
            out.contains("Content-Type: multipart/related; type=\"text/html\";"),
            "{out}"
        );
        assert_eq!(out.matches("Content-ID: <").count(), 1, "{out}");
        assert!(out.contains("Content-ID: <1.logo.png@rusticdoc>"), "{out}");
        assert!(out.contains("filename=\"logo.png\""), "{out}");
        assert!(out.lines().all(|l| l.len() <= 998), "{out}");
        assert!(
            !out.replace("\r\n", "").contains('\n'),
            "bare LF in {out:?}"
        );

        let bodies = decoded_parts(&out);
        assert_eq!(
            String::from_utf8_lossy(&bodies[0]),
            "Dear Ada,\r\n\r\nTotal: 1 500\r\n\r\nPay now (https://pay.example.com/x)\r\n"
        );
        let page = String::from_utf8_lossy(&bodies[1]);
        assert!(
            page.contains(
                "<img src=\"cid:1.logo.png@rusticdoc\"><img src=\"cid:1.logo.png@rusticdoc\">"
            ),
            "{page}"
        );
        assert_eq!(bodies[2], PNG);
        assert_eq!(bodies.len(), 3);
    }

    #[test]
    fn an_email_without_images_has_no_related_part() {
        let out = html(packed(b"<p>Hi</p>", &Files::new(), &[], Packed::Eml));
        assert!(!out.contains("multipart/related"), "{out}");
        assert!(!out.contains("Subject:"), "{out}");
        assert_eq!(decoded_parts(&out)[0], b"Hi\r\n");
    }

    #[test]
    fn a_subject_cannot_inject_headers_and_non_ascii_is_encoded() {
        let out = html(packed(
            b"<title>Hi\r\nBcc: x@y</title>",
            &Files::new(),
            &[],
            Packed::Eml,
        ));
        assert!(out.contains("Subject: Hi Bcc: x@y\r\n"), "{out}");
        let out = html(packed(
            "<title>Ankara ya Ünal — oda №5</title>".as_bytes(),
            &Files::new(),
            &[],
            Packed::Eml,
        ));
        let line = out.lines().find(|l| l.starts_with("Subject:")).unwrap();
        assert!(line.starts_with("Subject: =?UTF-8?B?"), "{out}");
    }

    #[test]
    fn text_keeps_rows_lists_and_cells_apart() {
        let text = to_text(
            "<table><tr><td>Item</td><td>Qty</td></tr><tr><td>Pen</td><td>2</td></tr></table><ul><li>one</li><li>two</li></ul><!-- x -->",
        );
        assert_eq!(text, "Item\tQty\r\nPen\t2\r\n\r\n- one\r\n- two\r\n");
    }

    #[test]
    fn text_of_libreoffice_cells_keeps_a_row_on_one_line() {
        // As LibreOffice writes them: each cell's text in a paragraph, after
        // a line break and indentation.
        let cell = |text: &str| {
            format!("\n\t\t<td width=\"25%\"><p class=\"western\">\n\t\t\t{text}</p>\n\t\t</td>")
        };
        let html = format!(
            "<table>\n\t<tr valign=\"top\">{}{}{}\n\t</tr>\n\t<tr>\n\t\t<td> Beans </td><td>2</td><td></td>\n\t</tr>\n</table>",
            cell("Item"),
            cell("Qty"),
            cell("Amount")
        );
        assert_eq!(to_text(&html), "Item\tQty\tAmount\r\nBeans\t2\r\n");
    }

    /// Each base64 body in the message, decoded, in order.
    fn decoded_parts(message: &str) -> Vec<Vec<u8>> {
        message
            .split("\r\n--")
            .filter_map(|p| p.split_once("\r\n\r\n"))
            .filter(|(h, _)| h.contains("Content-Transfer-Encoding: base64"))
            .map(|(_, body)| {
                let b64: String = body.lines().take_while(|l| !l.is_empty()).collect();
                STANDARD.decode(b64.trim()).unwrap()
            })
            .collect()
    }

    #[test]
    fn a_page_naming_one_file_many_times_is_refused_before_it_is_made() {
        let big = Bytes::from(vec![0u8; 30_000]);
        let f: Files = [
            ("big.png".to_string(), big.clone()),
            ("big.css".to_string(), big),
        ]
        .into();
        let images = "<img src=\"big.png\">".repeat(100);
        let links = "<link rel=\"stylesheet\" href=\"big.css\">".repeat(100);
        for (page, to) in [
            (&images, Packed::Html),
            (&links, Packed::Html),
            (&links, Packed::Eml),
        ] {
            let err = pack(page.as_bytes(), &f, &[], to, 1_000_000).unwrap_err();
            assert!(err.to_string().contains("1000000 bytes"), "{err}");
        }
        // An email attaches the image once, however often it is named.
        let eml = pack(images.as_bytes(), &f, &[], Packed::Eml, 1_000_000).unwrap();
        assert_eq!(html(eml).matches("Content-ID:").count(), 1);
        // A few copies fit.
        let few = "<img src=\"big.png\">".repeat(3);
        assert!(pack(few.as_bytes(), &f, &[], Packed::Html, 1_000_000).is_ok());
    }
}
