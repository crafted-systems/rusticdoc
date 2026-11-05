//! PDF templates: filling form fields, tags in annotations, and laying a
//! rendered template over a base PDF.
//!
//! Three mechanisms turn a PDF into a template:
//!
//! 1. **Form fields.** [`fields`] walks the AcroForm tree and [`fill`] sets
//!    every field whose fully qualified name is a path into the data
//!    (`customer.name`, `items.0.qty`), or whose value or default value is a
//!    `{{ }}` / `{% %}` template, which is rendered with the caller's
//!    environment. Text fields get a fresh appearance stream so Chrome and
//!    Preview, which do not regenerate appearances, show the value; the form
//!    is also marked `NeedAppearances` so Acrobat redraws it.
//! 2. **Annotation tags.** A `Text`, `FreeText` or `Square` annotation whose
//!    contents are a tag (`{{ customer.name }}`, optionally ending in the
//!    `| fill` or `| check` shorthand) fills the widget it sits over, or, when
//!    no widget is under it, has its rendered value drawn into its rectangle.
//!    The annotations can then be stripped from the output.
//! 3. **Overlay.** [`overlay`] draws every page of one PDF over the matching
//!    page of another by importing it as a form XObject, so a rendered
//!    template can be laid on letterhead or a letterhead on a rendered
//!    template.
//!
//! Text that this module draws itself (regenerated text-field appearances
//! and free-standing annotation values) uses the base-14 Helvetica font with
//! `WinAnsiEncoding`, which every viewer has built in. That covers Latin
//! text; characters outside WinAnsi are drawn as `?`. Field values
//! themselves are stored in full (UTF-16BE when they leave Latin), so a
//! viewer that regenerates appearances still shows them. Rich text, comb
//! fields, date formats and JavaScript actions are not interpreted.

mod fields;
mod fill;
mod overlay;
mod text;

use lopdf::{Dictionary, Document, Object, ObjectId, dictionary};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// One AcroForm field as the API lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldInfo {
    /// Fully qualified name, such as `customer.name`.
    pub name: String,
    /// `text`, `checkbox`, `radio`, `choice`, `signature` or `button`.
    pub kind: String,
    /// 1-based page the first widget sits on.
    pub page: u32,
    /// Current value, when any.
    pub value: Option<String>,
    /// Options for choice fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// How [`fill`] treats the form once the values are in.
#[derive(Debug, Clone, Default)]
pub struct FillOptions {
    /// Flatten the filled fields into page content so they cannot change.
    pub flatten: bool,
    /// Remove annotations that carried tags after applying them.
    pub strip_tag_annotations: bool,
}

/// Lists the fields of a form.
///
/// A field without widgets on any page reports page `0`. A PDF without an
/// AcroForm lists nothing.
pub fn fields(pdf: &[u8]) -> Result<Vec<FieldInfo>> {
    let doc = load(pdf)?;
    let fields = fields::collect(&doc)?;
    Ok(fields
        .iter()
        .map(|field| FieldInfo {
            name: field.name.clone(),
            kind: field.kind.as_str().to_string(),
            page: field.page(&doc),
            value: field.value.as_ref().and_then(|v| object_text(&doc, v)),
            options: field.options.clone(),
        })
        .collect())
}

/// Fills a form from data. A field named `customer.name` takes that path
/// from the data; a field whose default value is a `{{ }}` tag is rendered
/// with the environment. Annotations (notes, free text) whose contents are
/// a tag fill the field under them, or draw the value in their rectangle.
pub fn fill(
    pdf: &[u8],
    data: &serde_json::Value,
    env: &minijinja::Environment<'static>,
    options: &FillOptions,
) -> Result<Vec<u8>> {
    let doc = load(pdf)?;
    let doc = fill::fill(doc, data, env, options)?;
    save(doc)
}

/// Lays every page of `overlay` over the matching page of `base`. Extra
/// overlay pages are appended; extra base pages are kept as they are.
pub fn overlay(base: &[u8], overlay: &[u8]) -> Result<Vec<u8>> {
    let base_doc = load(base)?;
    let overlay_doc = load(overlay)?;
    let doc = overlay::overlay(base_doc, overlay_doc)?;
    save(doc)
}

fn load(pdf: &[u8]) -> Result<Document> {
    let doc = Document::load_mem(pdf)
        .map_err(|e| Error::InvalidRequest(format!("cannot parse the PDF: {e}")))?;
    if doc.is_encrypted() {
        return Err(Error::Unsupported("encrypted PDF forms".into()));
    }
    Ok(doc)
}

fn save(mut doc: Document) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| Error::Engine {
        engine: "lopdf",
        message: e.to_string(),
    })?;
    Ok(out)
}

fn malformed(e: lopdf::Error) -> Error {
    Error::InvalidRequest(format!("malformed PDF: {e}"))
}

const NULL: Object = Object::Null;

/// Follows references until the object itself; a dangling reference or a
/// reference loop resolves to null.
fn deref<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    let mut current = object;
    for _ in 0..32 {
        match current {
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(next) => current = next,
                Err(_) => return &NULL,
            },
            other => return other,
        }
    }
    &NULL
}

/// Reads `key` from `dict`, following references; a missing or null entry
/// is `None`.
fn get<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    dict.get(key)
        .ok()
        .map(|object| deref(doc, object))
        .filter(|object| !object.is_null())
}

fn get_dict<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Dictionary> {
    get(doc, dict, key).and_then(|object| object.as_dict().ok())
}

fn get_i64(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<i64> {
    get(doc, dict, key).and_then(|object| object.as_i64().ok())
}

fn get_name(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<String> {
    get(doc, dict, key)
        .and_then(|object| object.as_name().ok())
        .map(|name| String::from_utf8_lossy(name).into_owned())
}

/// How deep `object_text` follows nested arrays before giving up, so a
/// self-referencing object cannot recurse without end.
const MAX_TEXT_DEPTH: u32 = 16;

/// The textual value of an object: strings decoded, names as they are,
/// numbers printed, arrays joined with `, `.
fn object_text(doc: &Document, object: &Object) -> Option<String> {
    object_text_at(doc, object, 0)
}

fn object_text_at(doc: &Document, object: &Object, depth: u32) -> Option<String> {
    if depth >= MAX_TEXT_DEPTH {
        return None;
    }
    match deref(doc, object) {
        Object::String(bytes, _) => Some(text::decode_text_string(bytes)),
        Object::Name(name) => Some(String::from_utf8_lossy(name).into_owned()),
        Object::Integer(n) => Some(n.to_string()),
        Object::Real(n) => Some(n.to_string()),
        Object::Boolean(b) => Some(b.to_string()),
        Object::Array(items) => Some(
            items
                .iter()
                .filter_map(|item| object_text_at(doc, item, depth + 1))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        _ => None,
    }
}

/// The AcroForm dictionary, wherever the catalog keeps it.
fn acroform(doc: &Document) -> Option<&Dictionary> {
    let catalog = doc.catalog().ok()?;
    get_dict(doc, catalog, b"AcroForm")
}

/// A rectangle in page space with `x1 <= x2` and `y1 <= y2`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

impl Rect {
    fn from_object(doc: &Document, object: &Object) -> Option<Rect> {
        let items = deref(doc, object).as_array().ok()?;
        if items.len() != 4 {
            return None;
        }
        let mut n = [0f32; 4];
        for (slot, item) in n.iter_mut().zip(items) {
            *slot = deref(doc, item).as_float().ok()?;
        }
        Some(Rect {
            x1: n[0].min(n[2]),
            y1: n[1].min(n[3]),
            x2: n[0].max(n[2]),
            y2: n[1].max(n[3]),
        })
    }

    fn width(&self) -> f32 {
        self.x2 - self.x1
    }

    fn height(&self) -> f32 {
        self.y2 - self.y1
    }

    /// Area shared with `other`; zero when they do not overlap.
    fn overlap(&self, other: &Rect) -> f32 {
        let w = self.x2.min(other.x2) - self.x1.max(other.x1);
        let h = self.y2.min(other.y2) - self.y1.max(other.y1);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }
}

/// Adds `name => id` to the `category` dictionary (`Font`, `XObject`) of a
/// page's resources, creating what is missing and following references.
fn add_page_resource(
    doc: &mut Document,
    page_id: ObjectId,
    category: &[u8],
    name: &str,
    id: ObjectId,
) -> Result<()> {
    doc.get_or_create_resources(page_id).map_err(malformed)?;
    let page = doc.get_dictionary(page_id).map_err(malformed)?;
    let resources_ref = page
        .get(b"Resources")
        .ok()
        .and_then(|r| r.as_reference().ok());
    let category_ref = match resources_ref {
        Some(rid) => doc.get_dictionary(rid).ok(),
        None => page.get(b"Resources").ok().and_then(|r| r.as_dict().ok()),
    }
    .and_then(|res| res.get(category).ok())
    .and_then(|c| c.as_reference().ok());

    if let Some(cid) = category_ref {
        let dict = doc.get_dictionary_mut(cid).map_err(malformed)?;
        dict.set(name, Object::Reference(id));
        return Ok(());
    }
    let resources = match resources_ref {
        Some(rid) => doc.get_dictionary_mut(rid).map_err(malformed)?,
        None => doc
            .get_dictionary_mut(page_id)
            .map_err(malformed)?
            .get_mut(b"Resources")
            .and_then(Object::as_dict_mut)
            .map_err(malformed)?,
    };
    if !matches!(resources.get(category), Ok(Object::Dictionary(_))) {
        resources.set(category, Dictionary::new());
    }
    resources
        .get_mut(category)
        .and_then(Object::as_dict_mut)
        .map_err(malformed)?
        .set(name, Object::Reference(id));
    Ok(())
}

/// The entries of a page's `/Annots` array, whether it is direct or
/// indirect, as they are written.
fn page_annots(doc: &Document, page_id: ObjectId) -> Vec<Object> {
    doc.get_dictionary(page_id)
        .ok()
        .and_then(|page| get(doc, page, b"Annots"))
        .and_then(|a| a.as_array().ok())
        .cloned()
        .unwrap_or_default()
}

/// Replaces a page's `/Annots` with `entries`, dropping the key when empty.
fn set_page_annots(doc: &mut Document, page_id: ObjectId, entries: Vec<Object>) -> Result<()> {
    let page = doc.get_dictionary_mut(page_id).map_err(malformed)?;
    if entries.is_empty() {
        page.remove(b"Annots");
    } else {
        page.set("Annots", entries);
    }
    Ok(())
}

/// Wraps the page's existing content in `q ... Q`, so what is appended
/// after it starts from a clean graphics state whatever the page left
/// behind.
fn isolate_page_content(doc: &mut Document, page_id: ObjectId) -> Result<()> {
    let page = doc.get_dictionary(page_id).map_err(malformed)?;
    let mut list: Vec<Object> = match page.get(b"Contents") {
        Ok(Object::Reference(id)) => match doc.get_object(*id) {
            Ok(Object::Array(items)) => items.clone(),
            Ok(_) => vec![Object::Reference(*id)],
            Err(_) => Vec::new(),
        },
        Ok(Object::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    if list.is_empty() {
        return Ok(());
    }
    let open = doc.add_object(lopdf::Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let close = doc.add_object(lopdf::Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));
    list.insert(0, Object::Reference(open));
    list.push(Object::Reference(close));
    doc.get_dictionary_mut(page_id)
        .map_err(malformed)?
        .set("Contents", list);
    Ok(())
}

/// A base-14 Helvetica font dictionary, added to the document.
fn add_helvetica(doc: &mut Document) -> ObjectId {
    doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    })
}
