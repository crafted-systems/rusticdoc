//! Walking the AcroForm field tree into terminal fields with their widgets.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use lopdf::{Dictionary, Document, Object, ObjectId};

use super::{Rect, acroform, deref, get, get_i64, get_name, object_text};
use crate::error::{Error, Result};

/// The six kinds of field the API reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Text,
    Checkbox,
    Radio,
    Choice,
    Signature,
    Button,
}

impl Kind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Checkbox => "checkbox",
            Kind::Radio => "radio",
            Kind::Choice => "choice",
            Kind::Signature => "signature",
            Kind::Button => "button",
        }
    }
}

/// A widget annotation of a field, with the page it was found on.
#[derive(Debug, Clone)]
pub(super) struct Widget {
    pub id: ObjectId,
    pub page: Option<ObjectId>,
    pub rect: Option<Rect>,
}

/// A terminal field: one with a value of its own, and the widgets that show
/// it. Inheritable entries (`/FT`, `/Ff`, `/V`, `/DV`, `/Opt`, `/DA`,
/// `/Q`) are already resolved through the parents.
#[derive(Debug, Clone)]
pub(super) struct Field {
    pub id: ObjectId,
    pub name: String,
    pub kind: Kind,
    pub flags: i64,
    // Inherited entries are shared with every field under the node that set
    // them, not copied: a form can hang thousands of fields under one `/Opt`.
    pub value: Option<Arc<Object>>,
    pub default: Option<Arc<Object>>,
    pub options: Arc<Vec<String>>,
    /// The raw `/Opt` array of a choice field.
    pub opt: Option<Arc<Object>>,
    pub da: Option<String>,
    pub quadding: i64,
    pub widgets: Vec<Widget>,
}

impl Field {
    /// 1-based number of the page the first widget sits on; 0 when none.
    /// `numbers` is every page's number by its id, built once for the form.
    pub(super) fn page(&self, numbers: &HashMap<ObjectId, u32>) -> u32 {
        self.widgets
            .iter()
            .find_map(|w| w.page)
            .and_then(|pid| numbers.get(&pid).copied())
            .unwrap_or(0)
    }

    pub(super) fn has_flag(&self, bit: u32) -> bool {
        self.flags & (1i64 << (bit - 1)) != 0
    }
}

/// `/Ff` bits by their number in the PDF specification.
pub(super) const FF_MULTILINE: u32 = 13;
pub(super) const FF_RADIO: u32 = 16;
pub(super) const FF_PUSHBUTTON: u32 = 17;
pub(super) const FF_COMBO: u32 = 18;
pub(super) const FF_EDIT: u32 = 19;

#[derive(Clone, Default)]
struct Inherited {
    ft: Option<String>,
    ff: Option<i64>,
    v: Option<Arc<Object>>,
    dv: Option<Arc<Object>>,
    opt: Option<Arc<Object>>,
    /// The export values of `opt`, read once where it is set.
    options: Arc<Vec<String>>,
    da: Option<String>,
    q: Option<i64>,
}

impl Inherited {
    fn absorb(&mut self, doc: &Document, dict: &Dictionary) {
        if let Some(ft) = get_name(doc, dict, b"FT") {
            self.ft = Some(ft);
        }
        if let Some(ff) = get_i64(doc, dict, b"Ff") {
            self.ff = Some(ff);
        }
        if let Some(v) = get(doc, dict, b"V") {
            self.v = Some(Arc::new(v.clone()));
        }
        if let Some(dv) = get(doc, dict, b"DV") {
            self.dv = Some(Arc::new(dv.clone()));
        }
        if let Some(opt) = get(doc, dict, b"Opt") {
            self.options = Arc::new(options(doc, opt));
            self.opt = Some(Arc::new(opt.clone()));
        }
        if let Some(Object::String(da, _)) = get(doc, dict, b"DA") {
            self.da = Some(super::text::decode_text_string(da));
        }
        if let Some(q) = get_i64(doc, dict, b"Q") {
            self.q = Some(q);
        }
    }
}

struct Walker<'a> {
    doc: &'a Document,
    /// Page of every annotation reachable from a page's `/Annots`.
    annot_pages: HashMap<ObjectId, ObjectId>,
    /// Nodes already walked, so a cycle in `/Kids` ends where it started.
    visited: HashSet<ObjectId>,
    out: Vec<Field>,
}

/// How many field-tree nodes a form may have before it is refused.
const MAX_FIELD_NODES: usize = 10_000;

/// Every terminal field of the document's AcroForm, in tree order.
pub(super) fn collect(doc: &Document) -> Result<Vec<Field>> {
    let Some(form) = acroform(doc) else {
        return Ok(Vec::new());
    };
    let mut inherited = Inherited::default();
    inherited.absorb(doc, form);
    inherited.v = None;
    inherited.dv = None;

    let mut annot_pages = HashMap::new();
    for page_id in doc.page_iter() {
        for entry in super::page_annots(doc, page_id) {
            if let Object::Reference(id) = entry {
                annot_pages.entry(id).or_insert(page_id);
            }
        }
    }
    let mut walker = Walker {
        doc,
        annot_pages,
        visited: HashSet::new(),
        out: Vec::new(),
    };
    let roots = get(doc, form, b"Fields")
        .and_then(|f| f.as_array().ok())
        .cloned()
        .unwrap_or_default();
    for root in roots {
        if let Object::Reference(id) = root {
            walker.walk(id, "", &inherited, 0)?;
        }
    }
    Ok(walker.out)
}

impl Walker<'_> {
    fn walk(
        &mut self,
        id: ObjectId,
        parent: &str,
        inherited: &Inherited,
        depth: u32,
    ) -> Result<()> {
        if depth > 32 || !self.visited.insert(id) {
            return Ok(());
        }
        if self.visited.len() > MAX_FIELD_NODES {
            return Err(Error::InvalidRequest(format!(
                "the form has more than {MAX_FIELD_NODES} field nodes"
            )));
        }
        let Ok(dict) = self.doc.get_dictionary(id) else {
            return Ok(());
        };
        let mut inherited = inherited.clone();
        inherited.absorb(self.doc, dict);
        let name = match get(self.doc, dict, b"T").and_then(|t| object_text(self.doc, t)) {
            Some(t) if parent.is_empty() => t,
            Some(t) => format!("{parent}.{t}"),
            None => parent.to_string(),
        };
        let kids: Vec<ObjectId> = get(self.doc, dict, b"Kids")
            .and_then(|k| k.as_array().ok())
            .map(|kids| kids.iter().filter_map(|k| k.as_reference().ok()).collect())
            .unwrap_or_default();
        let is_field = |kid: &ObjectId| {
            self.doc
                .get_dictionary(*kid)
                .map(|d| d.has(b"T"))
                .unwrap_or(false)
        };
        if kids.iter().any(is_field) {
            for kid in kids {
                self.walk(kid, &name, &inherited, depth + 1)?;
            }
            return Ok(());
        }
        let Some(ft) = inherited.ft.as_deref() else {
            return Ok(());
        };
        let flags = inherited.ff.unwrap_or(0);
        let kind = match ft {
            "Tx" => Kind::Text,
            "Ch" => Kind::Choice,
            "Sig" => Kind::Signature,
            "Btn" if flags & (1 << (FF_PUSHBUTTON - 1)) != 0 => Kind::Button,
            "Btn" if flags & (1 << (FF_RADIO - 1)) != 0 => Kind::Radio,
            "Btn" => Kind::Checkbox,
            _ => return Ok(()),
        };
        let widget_ids = if kids.is_empty() { vec![id] } else { kids };
        let widgets = widget_ids.into_iter().map(|wid| self.widget(wid)).collect();
        self.out.push(Field {
            id,
            name,
            kind,
            flags,
            value: inherited.v.clone(),
            default: inherited.dv.clone(),
            options: inherited.options.clone(),
            opt: inherited.opt.clone(),
            da: inherited.da.clone(),
            quadding: inherited.q.unwrap_or(0),
            widgets,
        });
        Ok(())
    }

    fn widget(&self, id: ObjectId) -> Widget {
        let dict = self.doc.get_dictionary(id).ok();
        let declared = dict
            .and_then(|d| d.get(b"P").ok())
            .and_then(|p| p.as_reference().ok())
            .filter(|p| {
                self.doc
                    .get_dictionary(*p)
                    .is_ok_and(|d| d.has_type(b"Page"))
            });
        let page = declared.or_else(|| self.annot_pages.get(&id).copied());
        let rect = dict
            .and_then(|d| d.get(b"Rect").ok())
            .and_then(|r| Rect::from_object(self.doc, r));
        Widget { id, page, rect }
    }
}

/// The export values of a choice field's `/Opt`: each entry is a string or
/// an `[export display]` pair.
fn options(doc: &Document, opt: &Object) -> Vec<String> {
    let Ok(items) = deref(doc, opt).as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match deref(doc, item) {
            Object::Array(pair) => pair.first().and_then(|export| object_text(doc, export)),
            other => object_text(doc, other),
        })
        .collect()
}

/// The display text of the option whose export value is `value`, or
/// `value` itself when the field has no pairs.
pub(super) fn display_text(doc: &Document, field: &Field, value: &str) -> String {
    let Some(Ok(items)) = field.opt.as_ref().map(|opt| deref(doc, opt).as_array()) else {
        return value.to_string();
    };
    items
        .iter()
        .find_map(|item| match deref(doc, item) {
            Object::Array(pair) if pair.len() == 2 => {
                let export = object_text(doc, &pair[0])?;
                (export == value)
                    .then(|| object_text(doc, &pair[1]))
                    .flatten()
            }
            _ => None,
        })
        .unwrap_or_else(|| value.to_string())
}
