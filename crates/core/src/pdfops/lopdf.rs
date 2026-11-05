//! The pure-Rust engine, built on `lopdf`. It is first in the fallback
//! order, so it serves every operation it implements; encryption and
//! optimisation stay with the process engines.
//!
//! Text stamps use the base-14 Helvetica with WinAnsi encoding: Latin text
//! needs no embedded font, and anything outside Latin-1 is written as `?`.
//! Non-Latin stamps need the pdfcpu engine, which embeds a font.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use ::lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, StringFormat, dictionary};
use async_trait::async_trait;

use super::{
    Attachment, Bookmark, Deadline, FacturxProfile, Metadata, Overlay, OverlayKind, PdfEngine,
    Rotation, SplitMode, draw, pages, require_pdf, xmp,
};
use crate::error::{Error, Result};
use crate::job::Document;

pub(super) const ENGINE: &str = "lopdf";

/// Largest document the engine parses unless configured otherwise.
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

/// Most parts one split request may produce.
pub const MAX_SPLIT_PARTS: usize = 1_000;

/// Most name-tree nodes or entries one walk visits.
const TREE_LIMIT: usize = 10_000;

/// Blocking PDF jobs that may run at once, so a burst cannot take every
/// blocking thread of the runtime.
static PERMITS: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));

/// Most pages a split may touch in all (parts times document pages), since
/// every part starts from a copy of the whole document.
const MAX_SPLIT_PAGES: usize = 200_000;

/// The pure-Rust engine.
#[derive(Debug, Clone, Copy)]
pub struct LopdfEngine {
    max_bytes: usize,
}

impl Default for LopdfEngine {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

impl LopdfEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refuses documents (and overlay files) larger than `bytes`.
    pub fn with_max_bytes(mut self, bytes: usize) -> Self {
        self.max_bytes = bytes;
        self
    }

    fn accept(&self, doc: &Document) -> Result<()> {
        require_pdf(doc)?;
        self.accept_size(doc.bytes.len(), &doc.name)
    }

    fn accept_size(&self, len: usize, name: &str) -> Result<()> {
        if len > self.max_bytes {
            return Err(Error::InvalidRequest(format!(
                "'{name}' is {len} bytes, over the {} byte limit",
                self.max_bytes
            )));
        }
        Ok(())
    }
}

/// Runs CPU-bound PDF work off the async threads, under the step timeout.
/// The work checks the deadline itself; a task that still overruns is
/// abandoned, not killed, and ends on its own.
async fn blocking<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce(Deadline) -> Result<T> + Send + 'static,
) -> Result<T> {
    let deadline = Deadline::new(timeout);
    let work = async {
        // The permit travels into the blocking closure: a job that outlives
        // its timeout keeps it until the work really ends.
        let permit = PERMITS
            .clone()
            .acquire_owned()
            .await
            .map_err(engine_error)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            f(deadline)
        })
        .await
        .map_err(engine_error)?
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(result) => result,
        Err(_) => Err(Error::Timeout(timeout)),
    }
}

#[async_trait]
impl PdfEngine for LopdfEngine {
    fn name(&self) -> &'static str {
        ENGINE
    }

    async fn merge(&self, docs: &[Document], timeout: Duration) -> Result<Document> {
        if docs.is_empty() {
            return Err(Error::InvalidRequest("merge needs at least one PDF".into()));
        }
        docs.iter().try_for_each(|d| self.accept(d))?;
        let docs = docs.to_vec();
        blocking(timeout, move |deadline| merge(&docs, deadline)).await
    }

    async fn split(
        &self,
        doc: &Document,
        mode: &SplitMode,
        timeout: Duration,
    ) -> Result<Vec<Document>> {
        self.accept(doc)?;
        let (doc, mode) = (doc.clone(), mode.clone());
        blocking(timeout, move |deadline| split(&doc, &mode, deadline)).await
    }

    async fn flatten(&self, doc: &Document, timeout: Duration) -> Result<Document> {
        self.accept(doc)?;
        let doc = doc.clone();
        blocking(timeout, move |deadline| {
            let mut pdf = load(&doc)?;
            draw::flatten(&mut pdf, deadline)?;
            save(pdf, doc.name.clone())
        })
        .await
    }

    async fn rotate(
        &self,
        doc: &Document,
        rotation: Rotation,
        pages: &str,
        timeout: Duration,
    ) -> Result<Document> {
        self.accept(doc)?;
        let (doc, spec) = (doc.clone(), pages.to_string());
        blocking(timeout, move |deadline| {
            rotate(&doc, rotation, &spec, deadline)
        })
        .await
    }

    async fn read_metadata(&self, doc: &Document, timeout: Duration) -> Result<Metadata> {
        self.accept(doc)?;
        let doc = doc.clone();
        blocking(timeout, move |deadline| {
            let pdf = load(&doc)?;
            deadline.check()?;
            read_metadata(&pdf)
        })
        .await
    }

    async fn write_metadata(
        &self,
        doc: &Document,
        metadata: &Metadata,
        timeout: Duration,
    ) -> Result<Document> {
        self.accept(doc)?;
        let (doc, metadata) = (doc.clone(), metadata.clone());
        blocking(timeout, move |deadline| {
            let mut pdf = load(&doc)?;
            deadline.check()?;
            write_metadata(&mut pdf, &metadata, deadline)?;
            deadline.check()?;
            save(pdf, doc.name.clone())
        })
        .await
    }

    async fn read_bookmarks(&self, doc: &Document, timeout: Duration) -> Result<Vec<Bookmark>> {
        self.accept(doc)?;
        let doc = doc.clone();
        blocking(timeout, move |deadline| {
            read_outline(&load(&doc)?, deadline)
        })
        .await
    }

    async fn write_bookmarks(
        &self,
        doc: &Document,
        bookmarks: &[Bookmark],
        timeout: Duration,
    ) -> Result<Document> {
        self.accept(doc)?;
        let (doc, bookmarks) = (doc.clone(), bookmarks.to_vec());
        blocking(timeout, move |deadline| {
            let mut pdf = load(&doc)?;
            write_outline(&mut pdf, &bookmarks, deadline)?;
            save(pdf, doc.name.clone())
        })
        .await
    }

    async fn embed(
        &self,
        doc: &Document,
        files: &[Attachment],
        timeout: Duration,
    ) -> Result<Document> {
        self.accept(doc)?;
        for file in files {
            self.accept_size(file.bytes.len(), &file.name)?;
        }
        let (doc, files) = (doc.clone(), files.to_vec());
        blocking(timeout, move |deadline| {
            let mut pdf = load(&doc)?;
            for file in &files {
                embed_file(&mut pdf, file, "Data", deadline)?;
            }
            save(pdf, doc.name.clone())
        })
        .await
    }

    async fn watermark(
        &self,
        doc: &Document,
        overlay: &Overlay,
        timeout: Duration,
    ) -> Result<Document> {
        self.accept_overlay(doc, overlay)?;
        let (doc, overlay) = (doc.clone(), overlay.clone());
        blocking(timeout, move |deadline| {
            let mut pdf = load(&doc)?;
            draw::overlay(&mut pdf, &overlay, false, deadline)?;
            save(pdf, doc.name.clone())
        })
        .await
    }

    async fn stamp(
        &self,
        doc: &Document,
        overlay: &Overlay,
        timeout: Duration,
    ) -> Result<Document> {
        self.accept_overlay(doc, overlay)?;
        let (doc, overlay) = (doc.clone(), overlay.clone());
        blocking(timeout, move |deadline| {
            let mut pdf = load(&doc)?;
            draw::overlay(&mut pdf, &overlay, true, deadline)?;
            save(pdf, doc.name.clone())
        })
        .await
    }

    async fn facturx(
        &self,
        doc: &Document,
        xml: &[u8],
        profile: FacturxProfile,
        timeout: Duration,
    ) -> Result<Document> {
        self.accept(doc)?;
        self.accept_size(xml.len(), "factur-x.xml")?;
        let (doc, xml) = (doc.clone(), xml.to_vec());
        blocking(timeout, move |deadline| {
            let mut pdf = load(&doc)?;
            facturx(&mut pdf, xml, profile, deadline)?;
            save(pdf, doc.name.clone())
        })
        .await
    }
}

impl LopdfEngine {
    fn accept_overlay(&self, doc: &Document, overlay: &Overlay) -> Result<()> {
        self.accept(doc)?;
        match &overlay.kind {
            OverlayKind::Image { bytes, name } | OverlayKind::Pdf { bytes, name } => {
                self.accept_size(bytes.len(), name)
            }
            OverlayKind::Text { .. } => Ok(()),
        }
    }
}

// ---------------------------------------------------------------- loading

pub(super) fn engine_error(e: impl std::fmt::Display) -> Error {
    Error::Engine {
        engine: ENGINE,
        message: e.to_string(),
    }
}

/// Parses a document. Encrypted files are refused: the engines work on
/// open documents only.
pub(super) fn load(doc: &Document) -> Result<Pdf> {
    let pdf = Pdf::load_mem(&doc.bytes)
        .map_err(|e| Error::InvalidRequest(format!("'{}' cannot be parsed: {e}", doc.name)))?;
    if pdf.is_encrypted() {
        return Err(Error::InvalidRequest(format!(
            "'{}' is encrypted; decrypt it first",
            doc.name
        )));
    }
    if pdf.catalog().is_err() {
        return Err(Error::InvalidRequest(format!(
            "'{}' has no document catalog",
            doc.name
        )));
    }
    Ok(pdf)
}

pub(super) fn save(mut pdf: Pdf, name: String) -> Result<Document> {
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).map_err(engine_error)?;
    Ok(Document::new(name, bytes))
}

/// Decodes a PDF text string: UTF-16BE or UTF-8 with a byte order mark,
/// otherwise PDFDocEncoding read as Latin-1.
pub(super) fn text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .chunks(2)
            .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    bytes.iter().map(|&b| b as char).collect()
}

/// Encodes text for a PDF string: ASCII stays literal, the rest becomes
/// UTF-16BE with a byte order mark.
pub(super) fn text_object(s: &str) -> Object {
    if s.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
        return Object::string_literal(s);
    }
    let mut bytes = vec![0xFE, 0xFF];
    for unit in s.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(bytes, StringFormat::Hexadecimal)
}

/// A page attribute, following `/Parent` when the page inherits it.
pub(super) fn inherited(pdf: &Pdf, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut node = pdf.get_dictionary(page_id).ok()?;
    for _ in 0..64 {
        if let Ok(value) = node.get(key) {
            return pdf.dereference(value).ok().map(|(_, o)| o.clone());
        }
        let parent = node.get(b"Parent").ok()?.as_reference().ok()?;
        node = pdf.get_dictionary(parent).ok()?;
    }
    None
}

/// The dictionary behind `catalog[key]`, created when missing and moved
/// out of any reference so it can be edited in place.
fn catalog_dict_mut<'a>(pdf: &'a mut Pdf, key: &[u8]) -> Result<&'a mut Dictionary> {
    let existing = pdf.catalog()?.get(key).ok().cloned();
    let value = match existing {
        Some(Object::Reference(id)) => pdf.get_object(id).map_err(engine_error)?.clone(),
        Some(Object::Dictionary(d)) => Object::Dictionary(d),
        _ => Object::Dictionary(Dictionary::new()),
    };
    let catalog = pdf.catalog_mut()?;
    catalog.set(key.to_vec(), value);
    catalog
        .get_mut(key)
        .and_then(Object::as_dict_mut)
        .map_err(engine_error)
}

// ------------------------------------------------------------------ merge

pub(super) fn merge(docs: &[Document], deadline: Deadline) -> Result<Document> {
    let mut out = Pdf::with_version("1.7");
    let mut page_ids: Vec<ObjectId> = Vec::new();
    let mut bookmarks: Vec<Bookmark> = Vec::new();
    let mut fields: Vec<Object> = Vec::new();
    let mut acroform: Option<Dictionary> = None;
    let mut next_id = 1;
    for doc in docs {
        deadline.check()?;
        let mut pdf = load(doc)?;
        let offset = page_ids.len() as u32;
        let mut outline = read_outline(&pdf, deadline)?;
        shift_pages(&mut outline, offset);
        bookmarks.extend(outline);
        // Pages lose their old page tree, so inherited attributes move in.
        for id in pdf.page_iter().collect::<Vec<_>>() {
            deadline.check()?;
            for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
                if pdf.get_dictionary(id).map(|d| d.has(key)).unwrap_or(true) {
                    continue;
                }
                if let Some(value) = inherited(&pdf, id, key) {
                    pdf.get_object_mut(id)
                        .and_then(Object::as_dict_mut)
                        .map_err(engine_error)?
                        .set(key.to_vec(), value);
                }
            }
        }
        pdf.renumber_objects_with(next_id);
        next_id = pdf.max_id + 1;
        if let Ok(form) = pdf.catalog()?.get(b"AcroForm")
            && let Ok((_, Object::Dictionary(form))) = pdf.dereference(form)
        {
            if let Ok(Object::Array(list)) = form
                .get(b"Fields")
                .and_then(|f| pdf.dereference(f).map(|(_, o)| o))
            {
                fields.extend(list.iter().cloned());
            }
            acroform.get_or_insert_with(|| form.clone());
        }
        page_ids.extend(pdf.page_iter());
        for (id, object) in pdf.objects {
            let skip = object
                .as_dict()
                .ok()
                .and_then(|d| d.get_type().ok())
                .is_some_and(|t| matches!(t, b"Catalog" | b"Pages" | b"Outlines"));
            if !skip {
                out.objects.insert(id, object);
            }
        }
    }
    out.max_id = next_id - 1;
    let pages_id = out.new_object_id();
    for id in &page_ids {
        out.get_object_mut(*id)
            .and_then(Object::as_dict_mut)
            .map_err(engine_error)?
            .set("Parent", pages_id);
    }
    out.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids.iter().map(|id| Object::Reference(*id)).collect::<Vec<_>>(),
            "Count" => page_ids.len() as i64,
        }
        .into(),
    );
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
    if let Some(mut form) = acroform
        && !fields.is_empty()
    {
        form.set("Fields", fields);
        catalog.set("AcroForm", form);
    }
    let catalog_id = out.add_object(catalog);
    out.trailer.set("Root", catalog_id);
    write_outline(&mut out, &bookmarks, deadline)?;
    out.prune_objects();
    out.renumber_objects();
    save(out, docs[0].with_extension("pdf"))
}

fn shift_pages(bookmarks: &mut [Bookmark], offset: u32) {
    for b in bookmarks {
        b.page += offset;
        shift_pages(&mut b.children, offset);
    }
}

// ------------------------------------------------------------------ split

pub(super) fn split(doc: &Document, mode: &SplitMode, deadline: Deadline) -> Result<Vec<Document>> {
    let pdf = load(doc)?;
    let count = pdf.get_pages().len() as u32;
    let too_many = |parts: usize| {
        Error::InvalidRequest(format!(
            "split would produce {parts} parts, over the limit of {MAX_SPLIT_PARTS}"
        ))
    };
    // Parts are counted before any range is expanded, and stay ranges.
    let parts: Vec<(String, Vec<pages::PageRange>)> = match mode {
        SplitMode::Pages { ranges, unify } => {
            let ranges = pages::parse(ranges, count)?;
            if *unify {
                vec![(pages::join(&ranges).replace(',', "_"), ranges)]
            } else {
                if ranges.len() > MAX_SPLIT_PARTS {
                    return Err(too_many(ranges.len()));
                }
                ranges.into_iter().map(|r| (r.label(), vec![r])).collect()
            }
        }
        SplitMode::Intervals { span } => {
            if *span == 0 {
                return Err(Error::InvalidRequest(
                    "split span must be at least 1".into(),
                ));
            }
            let n = count.div_ceil(*span) as usize;
            if n > MAX_SPLIT_PARTS {
                return Err(too_many(n));
            }
            pages::intervals(*span, count)?
                .into_iter()
                .map(|r| (r.label(), vec![r]))
                .collect()
        }
    };
    if parts.len().saturating_mul(count as usize) > MAX_SPLIT_PAGES {
        return Err(Error::InvalidRequest(format!(
            "split of {count} pages into {} parts is over the limit of {MAX_SPLIT_PAGES} part-pages",
            parts.len()
        )));
    }
    let outline = read_outline(&pdf, deadline)?;
    let stem = doc.with_extension("");
    let stem = stem.trim_end_matches('.');
    let mut outputs = Vec::with_capacity(parts.len());
    for (label, ranges) in parts {
        deadline.check()?;
        // `index[old page]` is the page's number in the part, 0 when dropped.
        let mut index = vec![0u32; count as usize + 1];
        let mut next = 0;
        for n in 1..=count {
            if ranges.iter().any(|r| r.first <= n && n <= r.last) {
                next += 1;
                index[n as usize] = next;
            }
        }
        let delete: Vec<u32> = (1..=count).filter(|n| index[*n as usize] == 0).collect();
        let mut part = pdf.clone();
        part.delete_pages(&delete);
        deadline.check()?;
        let bookmarks = keep_bookmarks(&outline, &index);
        write_outline(&mut part, &bookmarks, deadline)?;
        part.prune_objects();
        part.renumber_objects();
        deadline.check()?;
        outputs.push(save(part, format!("{stem}-{label}.pdf"))?);
    }
    Ok(outputs)
}

/// Bookmarks whose page survives, renumbered through `index` (old page
/// number to new, 0 for dropped pages).
fn keep_bookmarks(bookmarks: &[Bookmark], index: &[u32]) -> Vec<Bookmark> {
    bookmarks
        .iter()
        .filter_map(|b| {
            let page = *index.get(b.page as usize).filter(|p| **p > 0)?;
            Some(Bookmark {
                title: b.title.clone(),
                page,
                children: keep_bookmarks(&b.children, index),
            })
        })
        .collect()
}

// ----------------------------------------------------------------- rotate

pub(super) fn rotate(
    doc: &Document,
    rotation: Rotation,
    spec: &str,
    deadline: Deadline,
) -> Result<Document> {
    let mut pdf = load(doc)?;
    let pages = pdf.get_pages();
    for n in pages::select(spec, pages.len() as u32)? {
        deadline.check()?;
        let id = pages[&n];
        let current = inherited(&pdf, id, b"Rotate")
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        pdf.get_object_mut(id)
            .and_then(Object::as_dict_mut)
            .map_err(engine_error)?
            .set("Rotate", (current + rotation.degrees()).rem_euclid(360));
    }
    save(pdf, doc.name.clone())
}

// --------------------------------------------------------------- metadata

/// Info keys and their XMP property; the third column is the XMP shape.
const INFO_XMP: &[(&str, &str, char)] = &[
    ("Title", "dc:title", 'a'),
    ("Author", "dc:creator", 's'),
    ("Subject", "dc:description", 'a'),
    ("Keywords", "pdf:Keywords", 't'),
    ("Creator", "xmp:CreatorTool", 't'),
    ("Producer", "pdf:Producer", 't'),
    ("CreationDate", "xmp:CreateDate", 't'),
    ("ModDate", "xmp:ModifyDate", 't'),
];

fn info_dict(pdf: &Pdf) -> Option<&Dictionary> {
    let info = pdf.trailer.get(b"Info").ok()?;
    pdf.dereference(info).ok()?.1.as_dict().ok()
}

fn read_metadata(pdf: &Pdf) -> Result<Metadata> {
    let mut meta = Metadata::new();
    if let Some(info) = info_dict(pdf) {
        for (key, value) in info.iter() {
            let value = match pdf.dereference(value).map(|(_, o)| o) {
                Ok(Object::String(bytes, _)) => serde_json::Value::String(text(bytes)),
                Ok(Object::Name(name)) => String::from_utf8_lossy(name).to_string().into(),
                Ok(Object::Integer(n)) => (*n).into(),
                Ok(Object::Real(r)) => serde_json::json!(*r),
                Ok(Object::Boolean(b)) => (*b).into(),
                _ => continue,
            };
            meta.insert(String::from_utf8_lossy(key).to_string(), value);
        }
    }
    if let Some(xml) = xmp_packet(pdf) {
        let mut props = serde_json::Map::new();
        let extra = [("pdfaid:part", ""), ("pdfaid:conformance", "")];
        for (info_key, qname) in INFO_XMP
            .iter()
            .map(|(k, q, _)| (*k, *q))
            .chain(extra.iter().map(|(q, k)| (*k, *q)))
        {
            if let Some(value) = xmp::get(&xml, qname) {
                if !info_key.is_empty() {
                    meta.entry(info_key.to_string())
                        .or_insert_with(|| value.clone().into());
                }
                props.insert(qname.to_string(), value.into());
            }
        }
        meta.insert("xmp".into(), props.into());
    }
    Ok(meta)
}

fn write_metadata(pdf: &mut Pdf, metadata: &Metadata, deadline: Deadline) -> Result<()> {
    let mut xml = xmp_packet(pdf);
    let info_id = match pdf.trailer.get(b"Info").ok().cloned() {
        Some(Object::Reference(id)) if pdf.get_dictionary(id).is_ok() => id,
        Some(Object::Dictionary(d)) => pdf.add_object(d),
        _ => pdf.add_object(Dictionary::new()),
    };
    pdf.trailer.set("Info", info_id);
    for (key, value) in metadata {
        deadline.check()?;
        if key == "xmp" {
            continue;
        }
        let value: Option<String> = match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        };
        let is_date = key == "CreationDate" || key == "ModDate";
        let (info_value, xmp_value) = match &value {
            Some(v) if is_date => {
                let (pdf_date, iso) = xmp::date_pair(v);
                (Some(pdf_date), Some(iso))
            }
            Some(v) => (Some(v.clone()), Some(v.clone())),
            None => (None, None),
        };
        let info = pdf.get_dictionary_mut(info_id).map_err(engine_error)?;
        match &info_value {
            Some(v) if key == "Trapped" => {
                info.set(key.as_bytes().to_vec(), Object::Name(v.as_bytes().to_vec()))
            }
            Some(v) => info.set(key.as_bytes().to_vec(), text_object(v)),
            None => {
                info.remove(key.as_bytes());
            }
        }
        if let Some(xml) = xml.as_mut()
            && let Some((_, qname, shape)) = INFO_XMP.iter().find(|(k, _, _)| k == key)
        {
            match shape {
                'a' => xmp::set_alt(xml, qname, xmp_value.as_deref()),
                's' => xmp::set_seq(xml, qname, xmp_value.as_deref()),
                _ => xmp::set_text(xml, qname, xmp_value.as_deref()),
            }
        }
    }
    if let Some(xml) = xml {
        set_xmp_packet(pdf, xml)?;
    }
    Ok(())
}

/// The XMP packet of the catalog's `/Metadata` stream, when there is one.
pub(super) fn xmp_packet(pdf: &Pdf) -> Option<String> {
    let id = pdf
        .catalog()
        .ok()?
        .get(b"Metadata")
        .ok()?
        .as_reference()
        .ok()?;
    let stream = pdf.get_object(id).ok()?.as_stream().ok()?;
    let bytes = stream
        .decompressed_content()
        .unwrap_or_else(|_| stream.content.clone());
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Replaces (or creates) the `/Metadata` stream. It stays unfiltered, as
/// PDF/A requires.
pub(super) fn set_xmp_packet(pdf: &mut Pdf, xml: String) -> Result<()> {
    let stream = Stream::new(
        dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
        xml.into_bytes(),
    )
    .with_compression(false);
    let existing = pdf
        .catalog()?
        .get(b"Metadata")
        .ok()
        .and_then(|m| m.as_reference().ok());
    match existing {
        Some(id) => pdf.set_object(id, stream),
        None => {
            let id = pdf.add_object(stream);
            pdf.catalog_mut()?.set("Metadata", id);
        }
    }
    Ok(())
}

// -------------------------------------------------------------- bookmarks

/// The outline tree, with destinations resolved to page numbers. Items
/// whose destination cannot be resolved point at page 1.
pub(super) fn read_outline(pdf: &Pdf, deadline: Deadline) -> Result<Vec<Bookmark>> {
    let index: HashMap<ObjectId, u32> =
        pdf.get_pages().into_iter().map(|(n, id)| (id, n)).collect();
    let first = pdf
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Outlines").ok())
        .and_then(|o| pdf.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok())
        .and_then(|o| o.get(b"First").ok())
        .and_then(|f| f.as_reference().ok());
    walk_outline(pdf, first, &index, &mut HashSet::new(), 0, deadline)
}

fn walk_outline(
    pdf: &Pdf,
    mut current: Option<ObjectId>,
    index: &HashMap<ObjectId, u32>,
    seen: &mut HashSet<ObjectId>,
    depth: usize,
    deadline: Deadline,
) -> Result<Vec<Bookmark>> {
    let mut out = Vec::new();
    while let Some(id) = current {
        deadline.check()?;
        if depth > 64 || !seen.insert(id) {
            break;
        }
        let Ok(item) = pdf.get_dictionary(id) else {
            break;
        };
        let title = item
            .get(b"Title")
            .ok()
            .and_then(|t| pdf.dereference(t).ok())
            .and_then(|(_, o)| o.as_str().ok())
            .map(text)
            .unwrap_or_default();
        let page = destination_page(pdf, item, index, deadline)?.unwrap_or(1);
        let first = item.get(b"First").ok().and_then(|f| f.as_reference().ok());
        let children = walk_outline(pdf, first, index, seen, depth + 1, deadline)?;
        out.push(Bookmark {
            title,
            page,
            children,
        });
        current = item.get(b"Next").ok().and_then(|n| n.as_reference().ok());
    }
    Ok(out)
}

/// The page an outline item points at; `None` when it cannot be resolved.
/// Only a deadline is an error.
fn destination_page(
    pdf: &Pdf,
    item: &Dictionary,
    index: &HashMap<ObjectId, u32>,
    deadline: Deadline,
) -> Result<Option<u32>> {
    let dest = match item.get(b"Dest") {
        Ok(dest) => dest.clone(),
        Err(_) => {
            let action = item
                .get(b"A")
                .ok()
                .and_then(|a| pdf.dereference(a).ok())
                .and_then(|(_, a)| a.as_dict().ok())
                .filter(|a| a.get(b"S").and_then(Object::as_name).ok() == Some(b"GoTo"));
            match action.and_then(|a| a.get(b"D").ok()) {
                Some(d) => d.clone(),
                None => return Ok(None),
            }
        }
    };
    resolve_destination(pdf, &dest, index, 0, deadline)
}

fn resolve_destination(
    pdf: &Pdf,
    dest: &Object,
    index: &HashMap<ObjectId, u32>,
    depth: usize,
    deadline: Deadline,
) -> Result<Option<u32>> {
    if depth > 8 {
        return Ok(None);
    }
    Ok(match dest {
        Object::Reference(id) => match pdf.get_object(*id) {
            Ok(target) => resolve_destination(pdf, target, index, depth + 1, deadline)?,
            Err(_) => None,
        },
        Object::Array(items) => match items.first() {
            Some(Object::Reference(page)) => index.get(page).copied(),
            Some(Object::Integer(n)) => Some(*n as u32 + 1),
            _ => None,
        },
        Object::Dictionary(d) => match d.get(b"D") {
            Ok(inner) => resolve_destination(pdf, inner, index, depth + 1, deadline)?,
            Err(_) => None,
        },
        Object::String(name, _) | Object::Name(name) => {
            match named_destination(pdf, name, deadline)? {
                Some(target) => resolve_destination(pdf, &target, index, depth + 1, deadline)?,
                None => None,
            }
        }
        _ => None,
    })
}

/// Looks `name` up in the catalog's `Dests` dictionary or the `Names/Dests`
/// name tree, following `/Limits` rather than reading every entry.
fn named_destination(pdf: &Pdf, name: &[u8], deadline: Deadline) -> Result<Option<Object>> {
    let Ok(catalog) = pdf.catalog() else {
        return Ok(None);
    };
    if let Ok(dests) = catalog
        .get(b"Dests")
        .and_then(|d| pdf.dereference(d).map(|(_, o)| o))
        && let Ok(dests) = dests.as_dict()
    {
        return Ok(dests.get(name).ok().cloned());
    }
    let tree = catalog
        .get(b"Names")
        .ok()
        .and_then(|n| pdf.dereference(n).ok())
        .and_then(|(_, n)| n.as_dict().ok())
        .and_then(|n| n.get(b"Dests").ok())
        .and_then(|t| pdf.dereference(t).ok())
        .and_then(|(_, t)| t.as_dict().ok());
    match tree {
        Some(tree) => {
            let mut found = Vec::new();
            let mut walk = TreeWalk::new(deadline);
            walk.visit(pdf, tree, Some(name), &mut found, 0)?;
            Ok(found.pop().map(|(_, v)| v))
        }
        None => Ok(None),
    }
}

/// A bounded walk over a name tree: nodes are visited once, at most
/// [`TREE_LIMIT`] of them, and the deadline is checked at each.
struct TreeWalk {
    seen: HashSet<ObjectId>,
    nodes: usize,
    deadline: Deadline,
}

impl TreeWalk {
    fn new(deadline: Deadline) -> Self {
        Self {
            seen: HashSet::new(),
            nodes: 0,
            deadline,
        }
    }

    /// Collects `(key, value)` pairs into `out`, leaves first to last. With
    /// `key`, only that entry is collected and subtrees whose `/Limits`
    /// exclude it are skipped; returns `true` once it is found.
    fn visit(
        &mut self,
        pdf: &Pdf,
        node: &Dictionary,
        key: Option<&[u8]>,
        out: &mut Vec<(Vec<u8>, Object)>,
        depth: usize,
    ) -> Result<bool> {
        self.deadline.check()?;
        self.nodes += 1;
        if depth > 32 || self.nodes > TREE_LIMIT || out.len() >= TREE_LIMIT {
            return Ok(false);
        }
        let deref = |k: &[u8]| node.get(k).and_then(|o| pdf.dereference(o).map(|(_, o)| o));
        if let (Some(key), Ok(Object::Array(limits))) = (key, deref(b"Limits"))
            && let [Object::String(low, _), Object::String(high, _)] = limits.as_slice()
            && (key < low.as_slice() || key > high.as_slice())
        {
            return Ok(false);
        }
        if let Ok(Object::Array(names)) = deref(b"Names") {
            for pair in names.chunks(2) {
                if let [Object::String(k, _), value] = pair {
                    if key.is_some_and(|key| key != k.as_slice()) {
                        continue;
                    }
                    out.push((k.clone(), value.clone()));
                    if key.is_some() || out.len() >= TREE_LIMIT {
                        return Ok(key.is_some());
                    }
                }
            }
        }
        if let Ok(Object::Array(kids)) = deref(b"Kids") {
            for kid in kids {
                if let Ok(id) = kid.as_reference()
                    && !self.seen.insert(id)
                {
                    continue;
                }
                if let Ok((_, Object::Dictionary(kid))) = pdf.dereference(kid)
                    && self.visit(pdf, kid, key, out, depth + 1)?
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// Every `(key, value)` of a name tree, leaves first to last, up to
/// [`TREE_LIMIT`] entries.
fn name_tree_entries(
    pdf: &Pdf,
    node: &Dictionary,
    out: &mut Vec<(Vec<u8>, Object)>,
    deadline: Deadline,
) -> Result<()> {
    TreeWalk::new(deadline)
        .visit(pdf, node, None, out, 0)
        .map(|_| ())
}

/// Replaces the outline tree. Pages outside the document are clamped to
/// its last page.
pub(super) fn write_outline(
    pdf: &mut Pdf,
    bookmarks: &[Bookmark],
    deadline: Deadline,
) -> Result<()> {
    let _ = pdf.delete_outlines();
    pdf.catalog_mut()?.remove(b"Outlines");
    if bookmarks.is_empty() {
        return Ok(());
    }
    let pages = pdf.get_pages();
    let count = pages.len() as u32;
    if count == 0 {
        return Err(Error::InvalidRequest("the PDF has no pages".into()));
    }
    let outlines_id = pdf.new_object_id();
    let (first, last, total) = outline_items(pdf, bookmarks, outlines_id, &pages, count, deadline)?;
    pdf.objects.insert(
        outlines_id,
        dictionary! {
            "Type" => "Outlines",
            "First" => first.unwrap_or(outlines_id),
            "Last" => last.unwrap_or(outlines_id),
            "Count" => total,
        }
        .into(),
    );
    pdf.catalog_mut()?.set("Outlines", outlines_id);
    Ok(())
}

fn outline_items(
    pdf: &mut Pdf,
    items: &[Bookmark],
    parent: ObjectId,
    pages: &std::collections::BTreeMap<u32, ObjectId>,
    count: u32,
    deadline: Deadline,
) -> Result<(Option<ObjectId>, Option<ObjectId>, i64)> {
    let ids: Vec<ObjectId> = items.iter().map(|_| pdf.new_object_id()).collect();
    let mut total = 0;
    for (i, item) in items.iter().enumerate() {
        deadline.check()?;
        let page_id = pages[&item.page.clamp(1, count)];
        let mut dict = dictionary! {
            "Title" => text_object(&item.title),
            "Parent" => parent,
            "Dest" => vec![Object::Reference(page_id), "Fit".into()],
        };
        if i > 0 {
            dict.set("Prev", ids[i - 1]);
        }
        if i + 1 < ids.len() {
            dict.set("Next", ids[i + 1]);
        }
        let (first, last, n) = outline_items(pdf, &item.children, ids[i], pages, count, deadline)?;
        if let (Some(first), Some(last)) = (first, last) {
            dict.set("First", first);
            dict.set("Last", last);
            dict.set("Count", n);
        }
        pdf.objects.insert(ids[i], dict.into());
        total += 1 + n;
    }
    Ok((ids.first().copied(), ids.last().copied(), total))
}

// ---------------------------------------------------------- embedded files

/// Embeds a file into the `EmbeddedFiles` name tree and lists it in the
/// catalog's `/AF`, with the given `/AFRelationship`. A file of the same
/// name is replaced.
pub(super) fn embed_file(
    pdf: &mut Pdf,
    file: &Attachment,
    relationship: &str,
    deadline: Deadline,
) -> Result<()> {
    if file.name.is_empty() {
        return Err(Error::InvalidRequest("an attachment needs a name".into()));
    }
    let mut stream = Stream::new(
        dictionary! {
            "Type" => "EmbeddedFile",
            "Subtype" => Object::Name(file.mime.as_bytes().to_vec()),
            "Params" => dictionary! { "Size" => file.bytes.len() as i64 },
        },
        file.bytes.clone(),
    );
    let _ = stream.compress();
    let stream_id = pdf.add_object(stream);
    let spec_id = pdf.add_object(dictionary! {
        "Type" => "Filespec",
        "F" => text_object(&file.name),
        "UF" => text_object(&file.name),
        "Desc" => text_object(&file.name),
        "AFRelationship" => Object::Name(relationship.as_bytes().to_vec()),
        "EF" => dictionary! { "F" => stream_id, "UF" => stream_id },
    });
    let mut entries = Vec::new();
    if let Some(tree) = embedded_files_tree(pdf) {
        name_tree_entries(pdf, &tree, &mut entries, deadline)?;
    }
    entries.retain(|(k, _)| k != file.name.as_bytes());
    entries.push((file.name.as_bytes().to_vec(), Object::Reference(spec_id)));
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let flat: Vec<Object> = entries
        .into_iter()
        .flat_map(|(k, v)| [Object::String(k, StringFormat::Literal), v])
        .collect();
    catalog_dict_mut(pdf, b"Names")?.set("EmbeddedFiles", dictionary! { "Names" => flat });
    let af = match pdf
        .catalog()?
        .get(b"AF")
        .and_then(|a| pdf.dereference(a).map(|(_, o)| o))
    {
        Ok(Object::Array(list)) => list.clone(),
        _ => Vec::new(),
    };
    let mut af: Vec<Object> = af
        .into_iter()
        .filter(|o| !is_filespec_named(pdf, o, &file.name))
        .collect();
    af.push(Object::Reference(spec_id));
    pdf.catalog_mut()?.set("AF", af);
    Ok(())
}

fn is_filespec_named(pdf: &Pdf, spec: &Object, name: &str) -> bool {
    pdf.dereference(spec)
        .ok()
        .and_then(|(_, o)| o.as_dict().ok())
        .and_then(|d| d.get(b"F").ok())
        .and_then(|f| f.as_str().ok())
        .is_some_and(|f| text(f) == name)
}

/// The `EmbeddedFiles` name tree root, when the catalog has one.
fn embedded_files_tree(pdf: &Pdf) -> Option<Dictionary> {
    let names = pdf
        .dereference(pdf.catalog().ok()?.get(b"Names").ok()?)
        .ok()?
        .1;
    let tree = pdf
        .dereference(names.as_dict().ok()?.get(b"EmbeddedFiles").ok()?)
        .ok()?
        .1;
    tree.as_dict().ok().cloned()
}

/// The embedded file names, in name-tree order (at most 10,000).
pub fn embedded_file_names(pdf: &Pdf) -> Vec<String> {
    let mut entries = Vec::new();
    if let Some(tree) = embedded_files_tree(pdf) {
        let _ = name_tree_entries(
            pdf,
            &tree,
            &mut entries,
            Deadline::new(Duration::from_secs(30)),
        );
    }
    entries.into_iter().map(|(k, _)| text(&k)).collect()
}

impl From<::lopdf::Error> for Error {
    fn from(e: ::lopdf::Error) -> Self {
        engine_error(e)
    }
}

// ---------------------------------------------------------------- Factur-X

/// Embeds `factur-x.xml` and writes the Factur-X XMP. The PDF/A identification
/// is only touched when the input already declares PDF/A-3 (its conformance
/// becomes `B`); a plain PDF gets the attachment and the `fx:` properties but
/// no PDF/A claim, since true PDF/A-3 needs the input exported as PDF/A-3.
fn facturx(pdf: &mut Pdf, xml: Vec<u8>, profile: FacturxProfile, deadline: Deadline) -> Result<()> {
    let (level, relationship) = match profile {
        FacturxProfile::Minimum => ("MINIMUM", "Data"),
        FacturxProfile::BasicWl => ("BASIC WL", "Data"),
        FacturxProfile::Basic => ("BASIC", "Alternative"),
        FacturxProfile::En16931 => ("EN 16931", "Alternative"),
        FacturxProfile::Extended => ("EXTENDED", "Alternative"),
    };
    embed_file(
        pdf,
        &Attachment {
            name: "factur-x.xml".into(),
            bytes: xml,
            mime: "text/xml".into(),
        },
        relationship,
        deadline,
    )?;
    let mut packet = xmp_packet(pdf).unwrap_or_else(xmp::empty_packet);
    if !packet.contains(xmp::FACTURX_NS) {
        xmp::insert_description(&mut packet, xmp::FACTURX_SCHEMA);
    }
    xmp::set_text(&mut packet, "fx:DocumentType", Some("INVOICE"));
    xmp::set_text(&mut packet, "fx:DocumentFileName", Some("factur-x.xml"));
    xmp::set_text(&mut packet, "fx:Version", Some("1.0"));
    xmp::set_text(&mut packet, "fx:ConformanceLevel", Some(level));
    if xmp::get(&packet, "pdfaid:part").as_deref() == Some("3") {
        xmp::set_text(&mut packet, "pdfaid:conformance", Some("B"));
    }
    set_xmp_packet(pdf, packet)
}
