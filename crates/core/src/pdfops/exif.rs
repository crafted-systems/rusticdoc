//! Metadata under ExifTool's names and value shapes, which is what Gotenberg's
//! clients read and write: `CreateDate` as `2006:09:18 16:27:50-04:00`,
//! `Marked` as a boolean, `Keywords` as a list, `PDFVersion` as a number.
//!
//! A write goes to the Info dictionary and to the XMP packet together, so a
//! PDF/A document stays consistent. Keys and values are checked as ExifTool's
//! wrapper checks them, so clients see the same refusals, even though nothing
//! here is a command line.

use std::collections::BTreeMap;

use ::lopdf::{Dictionary, Document as Pdf, Object};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde_json::{Value, json};

use super::lopdf::{engine_error, load, set_xmp_packet, text, text_object, xmp_packet};
use super::{Metadata, xmp};
use crate::error::{Error, Result};
use crate::job::Document;

/// A decoded XMP packet larger than this is ignored, since compression lets a
/// small file inflate to anything.
const MAX_PACKET: usize = 16 << 20;
/// XMP deeper than this is not metadata anyone wrote by hand.
const MAX_DEPTH: usize = 128;
/// Most elements read from one packet.
const MAX_NODES: usize = 200_000;

const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
const NS_PDF: &str = "http://ns.adobe.com/pdf/1.3/";
const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";
const NS_RIGHTS: &str = "http://ns.adobe.com/xap/1.0/rights/";
const NS_META: &str = "adobe:ns:meta/";
const NS_XML: &str = "http://www.w3.org/XML/1998/namespace";

/// Tags that would rename or move a file in ExifTool: dropped without error.
const DROPPED: &[&str] = &[
    "filename",
    "directory",
    "hardlink",
    "symlink",
    "filepermissions",
];
/// Bare names that are options in ExifTool: refused unless given a group.
const REJECTED: &[&str] = &[
    "addtagsfromfile",
    "alltagsfromfile",
    "api",
    "argfile",
    "charset",
    "common_args",
    "config",
    "csv",
    "diff",
    "echo",
    "efile",
    "execute",
    "ext",
    "fileorder",
    "geotag",
    "geosync",
    "htmldump",
    "if",
    "j",
    "json",
    "lang",
    "listitem",
    "o",
    "out",
    "p",
    "php",
    "require",
    "srcfile",
    "stay_open",
    "tagsfromfile",
    "textout",
    "use",
    "w",
    "wm",
    "xmlformat",
];

/// The text a client sees for any metadata it may not write.
pub const NOT_WRITABLE: &str = "The requested metadata could not be written; ensure values are valid and free of control characters.";

fn refused() -> Error {
    Error::Status(400, NOT_WRITABLE.to_string())
}

// ------------------------------------------------------------------ reading

/// Reads a document's metadata. An encrypted file only shows its header.
pub fn read(doc: &Document) -> Result<Metadata> {
    let pdf = Pdf::load_mem(&doc.bytes)
        .map_err(|e| Error::InvalidRequest(format!("'{}' cannot be parsed: {e}", doc.name)))?;
    let mut out = base();
    if pdf.is_encrypted() {
        out.insert("Encryption".into(), json!("Standard"));
        if let Some(version) = header_version(&doc.bytes) {
            out.insert("PDFVersion".into(), number_or(&version));
        }
        return Ok(out);
    }
    out.insert("PDFVersion".into(), number_or(&pdf.version));
    if doc.bytes.windows(11).any(|w| w == b"/Linearized") {
        out.insert("Linearized".into(), json!("Yes"));
    }
    if let Ok(catalog) = pdf.catalog() {
        if let Some(count) = page_count(&pdf, catalog) {
            out.insert("PageCount".into(), json!(count));
        }
        if let Some(lang) = catalog.get(b"Lang").ok().and_then(|l| deref_text(&pdf, l)) {
            out.insert("Language".into(), json!(lang));
        }
        let marked = catalog
            .get(b"MarkInfo")
            .ok()
            .and_then(|m| pdf.dereference(m).ok())
            .and_then(|(_, m)| m.as_dict().ok())
            .and_then(|m| m.get(b"Marked").ok())
            .and_then(|m| m.as_bool().ok());
        if marked == Some(true) {
            out.insert("TaggedPDF".into(), json!("Yes"));
        }
    }
    if let Some(info) = info(&pdf) {
        for (key, value) in info.iter() {
            let Some(value) = deref_text(&pdf, value) else {
                continue;
            };
            let key = String::from_utf8_lossy(key).to_string();
            match key.as_str() {
                "CreationDate" => {
                    out.insert("CreateDate".into(), json!(format_pdf_date(&value)));
                }
                "ModDate" => {
                    let date = json!(format_pdf_date(&value));
                    out.insert("ModifyDate".into(), date.clone());
                    out.insert("ModDate".into(), date);
                }
                "Keywords" => {
                    out.insert(key, split_keywords(&value));
                }
                _ => {
                    out.insert(key, json!(value));
                }
            }
        }
    }
    if let Some(packet) = xmp_packet(&pdf).filter(|p| p.len() <= MAX_PACKET) {
        match parse_packet(&packet) {
            Ok(props) => overlay(&mut out, props),
            Err(e) => tracing::warn!(error = %e, "ignoring invalid XMP metadata"),
        }
    }
    Ok(out)
}

fn base() -> Metadata {
    BTreeMap::from([
        ("FileType".to_string(), json!("PDF")),
        ("FileTypeExtension".to_string(), json!("pdf")),
        ("MIMEType".to_string(), json!("application/pdf")),
        ("Linearized".to_string(), json!("No")),
    ])
}

fn header_version(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(1024)];
    let at = head.windows(5).position(|w| w == b"%PDF-")? + 5;
    let rest = &head[at..];
    let end = rest
        .iter()
        .position(|b| !(b.is_ascii_digit() || *b == b'.'))
        .unwrap_or(rest.len());
    Some(String::from_utf8_lossy(&rest[..end]).to_string())
}

fn page_count(pdf: &Pdf, catalog: &Dictionary) -> Option<i64> {
    let pages = catalog.get(b"Pages").ok()?;
    let pages = pdf.dereference(pages).ok()?.1.as_dict().ok()?;
    let count = pages.get(b"Count").ok()?;
    pdf.dereference(count).ok()?.1.as_i64().ok()
}

fn info(pdf: &Pdf) -> Option<&Dictionary> {
    let info = pdf.trailer.get(b"Info").ok()?;
    pdf.dereference(info).ok()?.1.as_dict().ok()
}

/// A string or name, following a reference if needed.
fn deref_text(pdf: &Pdf, object: &Object) -> Option<String> {
    match pdf.dereference(object).ok()?.1 {
        Object::String(bytes, _) => Some(text(bytes)),
        Object::Name(name) => Some(String::from_utf8_lossy(name).to_string()),
        _ => None,
    }
}

fn number_or(s: &str) -> Value {
    match s.parse::<f64>() {
        Ok(n) => json!(n),
        Err(_) => json!(s),
    }
}

/// `/Keywords` is a comma or semicolon separated list; one entry stays a bare
/// string.
fn split_keywords(text: &str) -> Value {
    let parts: Vec<&str> = text.trim().split([',', ';']).map(str::trim).collect();
    if parts.len() == 1 {
        return json!(parts[0]);
    }
    json!(parts)
}

/// `D:20060918162750-04'00'` as `2006:09:18 16:27:50-04:00`; anything that is
/// not a PDF date stays as it is.
fn format_pdf_date(value: &str) -> String {
    let Some(rest) = value.strip_prefix("D:") else {
        return value.to_string();
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits < 4 {
        return value.to_string();
    }
    let (date, zone) = rest.split_at(digits);
    let part = |from: usize, to: usize, fallback: &'static str| -> &str {
        date.get(from..to).unwrap_or(fallback)
    };
    let mut out = format!(
        "{}:{}:{} {}:{}:{}",
        part(0, 4, "0000"),
        part(4, 6, "01"),
        part(6, 8, "01"),
        part(8, 10, "00"),
        part(10, 12, "00"),
        part(12, 14, "00"),
    );
    let mut chars = zone.chars();
    match chars.next() {
        Some('Z' | 'z') => out.push('Z'),
        Some(sign @ ('+' | '-')) => {
            let digits: String = chars.filter(char::is_ascii_digit).collect();
            out.push(sign);
            out.push_str(digits.get(..2).unwrap_or("00"));
            out.push(':');
            out.push_str(digits.get(2..4).unwrap_or("00"));
        }
        _ => {}
    }
    out
}

/// `2006-09-18T16:27:50-04:00` as `2006:09:18 16:27:50-04:00`, or `None` when
/// the text is not a date.
fn format_xmp_date(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() < 16 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' {
        return None;
    }
    let digit = |r: std::ops::Range<usize>| bytes[r].iter().all(u8::is_ascii_digit);
    if !(digit(0..4) && digit(5..7) && digit(8..10) && digit(11..13) && digit(14..16)) {
        return None;
    }
    let mut at = 16;
    let mut seconds = "00";
    if bytes.get(at) == Some(&b':') {
        seconds = value.get(at + 1..at + 3)?;
        if !seconds.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        at += 3;
    }
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
    }
    let zone = &value[at..];
    let valid_zone = zone.is_empty()
        || zone == "Z"
        || (zone.len() == 6
            && (zone.starts_with('+') || zone.starts_with('-'))
            && zone.as_bytes()[3] == b':');
    if !valid_zone {
        return None;
    }
    Some(format!(
        "{}:{}:{} {}:{}:{seconds}{zone}",
        &value[0..4],
        &value[5..7],
        &value[8..10],
        &value[11..13],
        &value[14..16],
    ))
}

// ------------------------------------------------------------------ XMP parse

#[derive(Default)]
struct Node {
    ns: String,
    local: String,
    attrs: Vec<(String, String, String)>,
    children: Vec<Node>,
    text: String,
}

/// One simple property of a packet.
struct Prop {
    ns: String,
    local: String,
    values: Vec<String>,
}

type Scopes = Vec<Vec<(String, String)>>;

fn resolve(scopes: &Scopes, prefix: &str) -> String {
    for scope in scopes.iter().rev() {
        if let Some((_, uri)) = scope.iter().find(|(p, _)| p == prefix) {
            return uri.clone();
        }
    }
    if prefix == "xml" {
        return NS_XML.to_string();
    }
    String::new()
}

fn split_name(name: &str) -> (&str, &str) {
    name.split_once(':').unwrap_or(("", name))
}

/// Opens a node from a start tag: its namespace declarations first, since they
/// apply to the tag itself.
fn open(start: &BytesStart, scopes: &mut Scopes) -> Result<Node> {
    let bad = |e: &dyn std::fmt::Display| engine_error(format!("parse XMP: {e}"));
    let mut scope = Vec::new();
    let mut plain = Vec::new();
    for attr in start.attributes() {
        let attr = attr.map_err(|e| bad(&e))?;
        let name = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let value = xmp::unescape(&String::from_utf8_lossy(&attr.value));
        if name == "xmlns" {
            scope.push((String::new(), value));
        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
            scope.push((prefix.to_string(), value));
        } else {
            plain.push((name, value));
        }
    }
    scopes.push(scope);
    let tag = String::from_utf8_lossy(start.name().as_ref()).to_string();
    let (prefix, local) = split_name(&tag);
    let mut node = Node {
        ns: resolve(scopes, prefix),
        local: local.to_string(),
        ..Node::default()
    };
    for (name, value) in plain {
        let (prefix, local) = split_name(&name);
        // An attribute without a prefix is in no namespace.
        let ns = if prefix.is_empty() {
            String::new()
        } else {
            resolve(scopes, prefix)
        };
        node.attrs.push((ns, local.to_string(), value));
    }
    Ok(node)
}

/// Parses the packet into a tree, under the limits above.
fn parse_tree(packet: &str) -> Result<Node> {
    let bad = |e: &dyn std::fmt::Display| engine_error(format!("parse XMP: {e}"));
    let mut reader = Reader::from_str(packet);
    let mut scopes: Scopes = Vec::new();
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut count = 0usize;
    loop {
        let event = reader.read_event().map_err(|e| bad(&e))?;
        match event {
            Event::Start(start) => {
                count += 1;
                if count > MAX_NODES || stack.len() > MAX_DEPTH {
                    return Err(bad(&"too many elements"));
                }
                stack.push(open(&start, &mut scopes)?);
            }
            Event::Empty(start) => {
                count += 1;
                if count > MAX_NODES {
                    return Err(bad(&"too many elements"));
                }
                let node = open(&start, &mut scopes)?;
                scopes.pop();
                stack
                    .last_mut()
                    .ok_or_else(|| bad(&"unbalanced elements"))?
                    .children
                    .push(node);
            }
            Event::End(_) => {
                scopes.pop();
                let node = stack.pop().ok_or_else(|| bad(&"unbalanced elements"))?;
                stack
                    .last_mut()
                    .ok_or_else(|| bad(&"unbalanced elements"))?
                    .children
                    .push(node);
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    let raw = String::from_utf8_lossy(t.as_ref()).to_string();
                    top.text.push_str(&xmp::unescape(&raw));
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(t.as_ref()));
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    let name = String::from_utf8_lossy(r.as_ref()).to_string();
                    top.text.push_str(&xmp::unescape(&format!("&{name};")));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 {
        return Err(bad(&"unbalanced elements"));
    }
    stack.pop().ok_or_else(|| bad(&"unbalanced elements"))
}

/// The simple properties of a packet. Nested structures such as PDF/A
/// extension schemas are skipped.
fn parse_packet(packet: &str) -> Result<Vec<Prop>> {
    let tree = parse_tree(packet)?;
    let mut props = Vec::new();
    collect(&tree, false, &mut props);
    Ok(props)
}

fn collect(node: &Node, in_rdf: bool, props: &mut Vec<Prop>) {
    for child in &node.children {
        if child.ns == NS_META && child.local == "xmpmeta" {
            for (ns, local, value) in &child.attrs {
                if ns == NS_META && local == "xmptk" {
                    props.push(Prop {
                        ns: NS_META.into(),
                        local: "xmptk".into(),
                        values: vec![value.clone()],
                    });
                }
            }
            collect(child, false, props);
        } else if child.ns == NS_RDF && child.local == "RDF" {
            collect(child, true, props);
        } else if in_rdf && child.ns == NS_RDF && child.local == "Description" {
            for (ns, local, value) in &child.attrs {
                if ns.is_empty() || ns == NS_RDF || ns == NS_XML {
                    continue;
                }
                props.push(Prop {
                    ns: ns.clone(),
                    local: local.clone(),
                    values: vec![value.clone()],
                });
            }
            for prop in &child.children {
                if let Some(values) = values_of(prop) {
                    props.push(Prop {
                        ns: prop.ns.clone(),
                        local: prop.local.clone(),
                        values,
                    });
                }
            }
        } else if !in_rdf {
            collect(child, false, props);
        }
    }
}

/// The values of a property, or `None` for a structure.
fn values_of(prop: &Node) -> Option<Vec<String>> {
    if prop.children.is_empty() {
        let text = prop.text.trim();
        return (!text.is_empty()).then(|| vec![text.to_string()]);
    }
    let mut values = Vec::new();
    for child in &prop.children {
        let container = child.ns == NS_RDF && matches!(child.local.as_str(), "Alt" | "Seq" | "Bag");
        if !container {
            return None;
        }
        let mut items = Vec::new();
        for item in &child.children {
            if item.ns != NS_RDF || item.local != "li" {
                return None;
            }
            items.extend(values_of(item)?);
        }
        // An rdf:Alt holds translations of one value; the first is the default.
        if child.local == "Alt" {
            items.truncate(1);
        }
        values.extend(items);
    }
    Some(values)
}

fn value_of(values: &[String]) -> Value {
    if let [one] = values {
        return scalar(one);
    }
    Value::Array(values.iter().map(|v| scalar(v)).collect())
}

fn scalar(value: &str) -> Value {
    if let Some(date) = format_xmp_date(value) {
        return json!(date);
    }
    let plain = {
        let body = value.strip_prefix('-').unwrap_or(value);
        let mut parts = body.splitn(2, '.');
        let whole = parts.next().unwrap_or("");
        let fraction = parts.next();
        !whole.is_empty()
            && whole.bytes().all(|b| b.is_ascii_digit())
            && fraction.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
    };
    if plain && let Ok(n) = value.parse::<f64>() {
        return json!(n);
    }
    json!(value)
}

/// Merges a packet into the metadata read from Info. XMP wins, as in ExifTool,
/// except that `dc:creator` only fills `Author` when Info has none and the
/// creating application stays in `Creator`, so a write reads back as written.
fn overlay(out: &mut Metadata, props: Vec<Prop>) {
    for prop in props {
        let value = value_of(&prop.values);
        match (prop.ns.as_str(), prop.local.as_str()) {
            (NS_META, "xmptk") => {
                out.insert("XMPToolkit".into(), value);
            }
            (NS_DC, "title") => {
                out.insert("Title".into(), value);
            }
            (NS_DC, "creator") | (NS_PDF, "Author") => {
                out.entry("Author".into()).or_insert(value);
            }
            (NS_PDF, "ModDate") => {
                out.entry("ModDate".into()).or_insert(value);
            }
            (NS_DC, "description") => {
                out.insert("Description".into(), value);
            }
            (NS_DC, "subject") => {
                out.entry("Subject".into()).or_insert(value);
            }
            (NS_DC, "language") => {
                out.insert("Language".into(), value);
            }
            (NS_DC, "format") => {
                out.insert("Format".into(), value);
            }
            (NS_PDF, "Keywords") => {
                let keywords = match prop.values.as_slice() {
                    [one] => split_keywords(one),
                    _ => value,
                };
                out.insert("Keywords".into(), keywords);
            }
            (NS_PDF, "Producer") => {
                out.insert("Producer".into(), value);
            }
            (NS_XMP, "CreatorTool") => {
                out.insert("CreatorTool".into(), value.clone());
                out.entry("Creator".into()).or_insert(value);
            }
            (NS_XMP, "CreateDate") => {
                out.entry("CreateDate".into()).or_insert(value);
            }
            (NS_XMP, "ModifyDate") => {
                out.entry("ModifyDate".into()).or_insert(value);
            }
            (NS_RIGHTS, "Marked") => {
                let marked = match &value {
                    Value::String(text) => json!(text.eq_ignore_ascii_case("true")),
                    other => other.clone(),
                };
                out.insert("Marked".into(), marked);
            }
            (_, local) => {
                let mut chars = local.chars();
                if let Some(first) = chars.next() {
                    let key = first.to_uppercase().chain(chars).collect::<String>();
                    out.insert(key, value);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ writing

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Alt,
    Seq,
    Bag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InfoKind {
    Text,
    Date,
    Name,
}

/// Where a key is stored.
#[derive(Debug, Clone, Copy)]
struct Field {
    info: Option<&'static str>,
    info_kind: InfoKind,
    qname: Option<&'static str>,
    kind: Kind,
    date: bool,
    /// Stored in Info, with any XMP copy removed.
    info_only: bool,
}

const fn field(info: &'static str, qname: &'static str, kind: Kind) -> Field {
    Field {
        info: Some(info),
        info_kind: InfoKind::Text,
        qname: Some(qname),
        kind,
        date: false,
        info_only: false,
    }
}

const fn date_field(info: &'static str, qname: &'static str) -> Field {
    Field {
        info: Some(info),
        info_kind: InfoKind::Date,
        qname: Some(qname),
        kind: Kind::Text,
        date: true,
        info_only: false,
    }
}

const fn xmp_field(qname: &'static str, kind: Kind) -> Field {
    Field {
        info: None,
        info_kind: InfoKind::Text,
        qname: Some(qname),
        kind,
        date: false,
        info_only: false,
    }
}

fn known_field(lower: &str) -> Option<Field> {
    Some(match lower {
        "title" => field("Title", "dc:title", Kind::Alt),
        "author" => field("Author", "dc:creator", Kind::Seq),
        "subject" => field("Subject", "dc:description", Kind::Alt),
        "keywords" => field("Keywords", "pdf:Keywords", Kind::Text),
        "creator" | "creatortool" => field("Creator", "xmp:CreatorTool", Kind::Text),
        "producer" => field("Producer", "pdf:Producer", Kind::Text),
        "createdate" | "creationdate" => date_field("CreationDate", "xmp:CreateDate"),
        "moddate" | "modifydate" => date_field("ModDate", "xmp:ModifyDate"),
        "trapped" => Field {
            info: Some("Trapped"),
            info_kind: InfoKind::Name,
            qname: Some("pdf:Trapped"),
            kind: Kind::Text,
            date: false,
            info_only: true,
        },
        "copyright" => xmp_field("tiff:Copyright", Kind::Alt),
        "rights" => xmp_field("dc:rights", Kind::Alt),
        "description" => xmp_field("dc:description", Kind::Alt),
        "language" => xmp_field("dc:language", Kind::Bag),
        "marked" => xmp_field("xmpRights:Marked", Kind::Text),
        "pdfversion" => xmp_field("pdf:PDFVersion", Kind::Text),
        "metadatadate" => Field {
            date: true,
            ..xmp_field("xmp:MetadataDate", Kind::Text)
        },
        _ => return None,
    })
}

/// One Info entry to set, or to delete when `None`.
#[derive(Debug, Clone)]
enum InfoValue {
    Text(String),
    Name(String),
}

/// A validated metadata write.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    info: BTreeMap<String, Option<InfoValue>>,
    props: Vec<(String, Kind, Vec<String>)>,
    /// At least one value only XMP can hold.
    creates_xmp: bool,
}

impl Plan {
    /// Checks the keys and values and decides what goes where.
    pub fn build(metadata: &Metadata) -> Result<Self> {
        let mut plan = Plan::default();
        for (key, value) in metadata {
            let bare = key.rsplit(':').next().unwrap_or(key);
            let lower = bare.to_ascii_lowercase();
            if DROPPED.contains(&lower.as_str()) {
                continue;
            }
            if !safe_key(key) || bare.is_empty() {
                return Err(refused());
            }
            if bare == key && REJECTED.contains(&lower.as_str()) {
                return Err(refused());
            }
            let values = values(value)?;
            let field = known_field(&lower).unwrap_or(Field {
                info: None,
                info_kind: InfoKind::Text,
                qname: None,
                kind: Kind::Text,
                date: false,
                info_only: false,
            });
            let info_key = field.info.map(str::to_string).or_else(|| {
                // Unknown tags are kept as custom Info entries, which any PDF
                // reader can read back, rather than being lost.
                field.qname.is_none().then(|| bare.to_string())
            });
            plan.add(field, info_key, values)?;
        }
        Ok(plan)
    }

    fn add(&mut self, field: Field, info_key: Option<String>, values: Vec<String>) -> Result<()> {
        let deleting = values.is_empty();
        let joined = values.join(", ");
        let (info_value, xmp_values) = if deleting {
            (None, Vec::new())
        } else if field.info_kind == InfoKind::Name {
            let name = match joined.to_ascii_lowercase().as_str() {
                "true" => "True",
                "false" => "False",
                "unknown" => "Unknown",
                _ => return Err(refused()),
            };
            (Some(InfoValue::Name(name.to_string())), values)
        } else if field.date || field.info_kind == InfoKind::Date {
            let (pdf, iso) = parse_date(&values[0]).ok_or_else(refused)?;
            (Some(InfoValue::Text(pdf)), vec![iso])
        } else if matches!(field.kind, Kind::Text | Kind::Alt) {
            (Some(InfoValue::Text(joined.clone())), vec![joined])
        } else {
            (Some(InfoValue::Text(joined)), values)
        };
        if let Some(key) = &info_key {
            self.info.insert(key.clone(), info_value);
        }
        if let Some(qname) = field.qname {
            let xmp_values = if field.info_only {
                Vec::new()
            } else {
                xmp_values
            };
            self.props.push((qname.to_string(), field.kind, xmp_values));
            if field.info.is_none() && !deleting {
                self.creates_xmp = true;
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.info.is_empty() && self.props.is_empty()
    }

    /// Writes the plan into a loaded document.
    pub fn apply(&self, pdf: &mut Pdf) -> Result<()> {
        if !self.info.is_empty() {
            let info_id = match pdf.trailer.get(b"Info").ok().cloned() {
                Some(Object::Reference(id)) if pdf.get_dictionary(id).is_ok() => id,
                Some(Object::Dictionary(d)) => pdf.add_object(d),
                _ => pdf.add_object(Dictionary::new()),
            };
            pdf.trailer.set("Info", info_id);
            let info = pdf.get_dictionary_mut(info_id).map_err(engine_error)?;
            for (key, value) in &self.info {
                match value {
                    Some(InfoValue::Text(text)) => {
                        info.set(key.as_bytes().to_vec(), text_object(text));
                    }
                    Some(InfoValue::Name(name)) => {
                        info.set(
                            key.as_bytes().to_vec(),
                            Object::Name(name.as_bytes().to_vec()),
                        );
                    }
                    None => {
                        info.remove(key.as_bytes());
                    }
                }
            }
        }
        if self.props.is_empty() {
            return Ok(());
        }
        // A document without XMP only gets a packet for what Info cannot hold.
        let mut packet = match xmp_packet(pdf) {
            Some(packet) => packet,
            None if self.creates_xmp => xmp::empty_packet(),
            None => return Ok(()),
        };
        for (qname, kind, values) in &self.props {
            let items = |container: &str| -> String {
                let items: String = values
                    .iter()
                    .map(|v| format!("<rdf:li>{}</rdf:li>", xmp::escape(v)))
                    .collect();
                format!("<rdf:{container}>{items}</rdf:{container}>")
            };
            let inner = if values.is_empty() {
                None
            } else {
                Some(match kind {
                    Kind::Text => xmp::escape(&values[0]),
                    Kind::Alt => format!(
                        "<rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt>",
                        xmp::escape(&values[0])
                    ),
                    Kind::Seq => items("Seq"),
                    Kind::Bag => items("Bag"),
                })
            };
            xmp::set_raw(&mut packet, qname, inner.as_deref());
        }
        set_xmp_packet(pdf, packet)
    }
}

/// Letters, digits, `_`, `.` and `:`, and `-` after the first character.
fn safe_key(key: &str) -> bool {
    let mut chars = key.chars();
    let first = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':'));
    first && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// A JSON value as strings: strings, lists of strings, booleans and numbers.
/// An empty value deletes the tag, as it does in ExifTool.
fn values(value: &Value) -> Result<Vec<String>> {
    let values = match value {
        Value::String(s) => vec![s.clone()],
        Value::Array(list) => list
            .iter()
            .map(|entry| entry.as_str().map(str::to_string).ok_or_else(refused))
            .collect::<Result<Vec<_>>>()?,
        Value::Bool(true) => vec!["True".to_string()],
        Value::Bool(false) => vec!["False".to_string()],
        Value::Number(n) => vec![n.to_string()],
        Value::Null | Value::Object(_) => return Err(refused()),
    };
    if values.iter().any(|s| s.contains(['\n', '\r', '\0'])) {
        return Err(refused());
    }
    Ok(values.into_iter().filter(|s| !s.is_empty()).collect())
}

/// A client's date in the two forms a PDF stores it in: for Info and for XMP.
/// Takes ISO and ExifTool separators, with or without a time and a zone.
fn parse_date(value: &str) -> Option<(String, String)> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() < 10 || !matches!(bytes[4], b'-' | b':') || !matches!(bytes[7], b'-' | b':') {
        return None;
    }
    let number = |r: std::ops::Range<usize>| -> Option<&str> {
        let part = value.get(r)?;
        part.bytes().all(|b| b.is_ascii_digit()).then_some(part)
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (m, d): (u32, u32) = (month.parse().ok()?, day.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    if bytes.len() == 10 {
        return Some((
            format!("D:{year}{month}{day}000000"),
            format!("{year}-{month}-{day}T00:00:00"),
        ));
    }
    if !matches!(bytes[10], b'T' | b't' | b' ') || bytes.len() < 19 {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let mut at = 19;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
    }
    let zone = value.get(at..)?;
    let stamp = format!("{year}{month}{day}{hour}{minute}{second}");
    let iso = format!("{year}-{month}-{day}T{hour}:{minute}:{second}");
    if zone.is_empty() {
        return Some((format!("D:{stamp}"), iso));
    }
    if zone.eq_ignore_ascii_case("z") {
        return Some((format!("D:{stamp}Z"), format!("{iso}Z")));
    }
    let z = zone.as_bytes();
    if z.len() != 6 || !matches!(z[0], b'+' | b'-') || z[3] != b':' {
        return None;
    }
    let (zh, zm) = (zone.get(1..3)?, zone.get(4..6)?);
    if !zh.bytes().chain(zm.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((
        format!("D:{stamp}{}{zh}'{zm}'", &zone[..1]),
        format!("{iso}{zone}"),
    ))
}

/// A write followed by a read of the same document, for tests and the CLI.
pub fn write_bytes(doc: &Document, plan: &Plan) -> Result<Document> {
    let mut pdf = load(doc)?;
    plan.apply(&mut pdf)?;
    super::lopdf::save(pdf, doc.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdfops::fixtures;

    fn metadata(raw: &str) -> Metadata {
        serde_json::from_str(raw).unwrap()
    }

    const SAMPLE: &str = r#"{"Author":"Ada","Copyright":"Ada","CreateDate":"2006-09-18T16:27:50-04:00",
        "Creator":"Maker","Keywords":["first","second"],"Marked":true,
        "ModDate":"2006-09-18T16:27:50-04:00","PDFVersion":1.7,"Producer":"Press",
        "Subject":"Sample","Title":"Sample","Trapped":"Unknown"}"#;

    #[test]
    fn writes_read_back_in_exiftools_shapes() {
        let doc = Document::new("a.pdf", fixtures::pages(1));
        let plan = Plan::build(&metadata(SAMPLE)).unwrap();
        let out = read(&write_bytes(&doc, &plan).unwrap()).unwrap();
        assert_eq!(out["Author"], "Ada");
        assert_eq!(out["Copyright"], "Ada");
        assert_eq!(out["CreateDate"], "2006:09:18 16:27:50-04:00");
        assert_eq!(out["ModDate"], "2006:09:18 16:27:50-04:00");
        assert_eq!(out["Creator"], "Maker");
        assert_eq!(out["Keywords"], json!(["first", "second"]));
        assert_eq!(out["Marked"], true);
        assert_eq!(out["PDFVersion"], 1.7);
        assert_eq!(out["Producer"], "Press");
        assert_eq!(out["Subject"], "Sample");
        assert_eq!(out["Title"], "Sample");
        assert_eq!(out["Trapped"], "Unknown");
        assert_eq!(out["FileType"], "PDF");
    }

    #[test]
    fn unsafe_requests_are_refused_or_dropped() {
        for raw in [
            r#"{"Title":"a\nb"}"#,
            r#"{"j":"/tmp/db.json"}"#,
            r#"{"bad key":"x"}"#,
            r#"{"Trapped":"Maybe"}"#,
            r#"{"CreateDate":"yesterday"}"#,
            r#"{"Keywords":[1,2]}"#,
        ] {
            let err = Plan::build(&metadata(raw)).unwrap_err();
            assert!(matches!(err, Error::Status(400, _)), "{raw}: {err}");
        }
        let plan = Plan::build(&metadata(
            r#"{"System:FileName":"stolen.pdf","System:Directory":"/tmp"}"#,
        ))
        .unwrap();
        assert!(plan.is_empty());
        assert!(Plan::build(&metadata(r#"{"XMP:j":"fine"}"#)).is_ok());
    }

    #[test]
    fn dates_take_both_separators() {
        assert_eq!(
            parse_date("2006:09:18 16:27:50+01:00").unwrap().0,
            "D:20060918162750+01'00'"
        );
        assert_eq!(
            parse_date("2006-09-18T16:27:50Z").unwrap().1,
            "2006-09-18T16:27:50Z"
        );
        assert_eq!(parse_date("2006-09-18").unwrap().0, "D:20060918000000");
        assert_eq!(parse_date("nope"), None);
        assert_eq!(parse_date("2006-13-18"), None);
        assert_eq!(
            format_pdf_date("D:20060918162750-04'00'"),
            "2006:09:18 16:27:50-04:00"
        );
        assert_eq!(format_pdf_date("D:2006"), "2006:01:01 00:00:00");
        assert_eq!(format_pdf_date("whenever"), "whenever");
        assert_eq!(
            format_xmp_date("2006-09-18T16:27:50.5+01:00").unwrap(),
            "2006:09:18 16:27:50+01:00"
        );
        assert_eq!(format_xmp_date("2006"), None);
    }

    #[test]
    fn packets_with_any_prefix_are_understood() {
        let packet = r#"<?xpacket begin="" id="x"?><x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="kit">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:d="http://purl.org/dc/elements/1.1/" xmlns:pa="http://ns.adobe.com/pdf/1.3/" pa:Producer="A &amp; B">
<d:title><rdf:Alt><rdf:li xml:lang="x-default">T</rdf:li><rdf:li xml:lang="de">U</rdf:li></rdf:Alt></d:title>
<d:creator><rdf:Seq><rdf:li>One</rdf:li><rdf:li>Two</rdf:li></rdf:Seq></d:creator>
</rdf:Description>
<rdf:Description rdf:about="" xmlns:e="http://www.aiim.org/pdfa/ns/extension/"><e:schemas><rdf:Bag><rdf:li rdf:parseType="Resource"><e:x>skip</e:x></rdf:li></rdf:Bag></e:schemas></rdf:Description>
</rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;
        let mut out = base();
        overlay(&mut out, parse_packet(packet).unwrap());
        assert_eq!(out["XMPToolkit"], "kit");
        assert_eq!(out["Producer"], "A & B");
        assert_eq!(out["Title"], "T");
        assert_eq!(out["Author"], json!(["One", "Two"]));
        assert!(!out.contains_key("Schemas"));
        assert!(parse_packet("<a><b></a>").is_err());
    }
}
