//! Filling: values from data or templates, regenerated appearances,
//! annotation tags and flattening.

use std::fmt::Write as _;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use serde_json::Value;

use super::fields::{self, FF_COMBO, FF_EDIT, FF_MULTILINE, Field, Kind};
use super::shaped::{self, Align, FontChoice, TextBox};
use super::text::{self, num};
use super::{
    FillLimits, FillOptions, Rect, acroform, add_helvetica, add_page_resource, deref, get,
    get_dict, get_i64, get_name, malformed, object_text, page_annots, set_page_annots,
};
use crate::error::{Error, Result};
use crate::limits::MAX_FILL_TAGS;
use crate::pdfops::Deadline;
use crate::template;
use rusticdoc_template::limit::{self, RenderFailure};

/// Fills every field, applies annotation tags, marks the form for
/// regeneration and flattens it when asked.
pub(super) fn fill(
    doc: Document,
    data: &Value,
    env: &minijinja::Environment<'static>,
    options: &FillOptions,
    limits: FillLimits,
) -> Result<Document> {
    let fields = fields::collect(&doc)?;
    let mut filler = Filler {
        doc,
        data,
        env: env.clone(),
        helvetica: None,
        shaped: shaped::Pending::default(),
        fuel: template::TEMPLATE_FUEL,
        bytes_left: limits.max_render_bytes,
        tags: 0,
        limits,
    };
    for field in &fields {
        limits.deadline.check()?;
        let value = match lookup(data, &field.name) {
            Some(value) => Some(value.clone()),
            None => filler.template_value(field)?.map(Value::String),
        };
        if let Some(value) = value {
            filler.apply(field, &value)?;
        }
    }
    filler.annotation_tags(&fields, options.strip_tag_annotations)?;
    // Viewers that redraw fields on request (Poppler, Acrobat) cannot shape
    // or find a face for text outside Latin, so a form that has some keeps
    // the appearances drawn here.
    let shaped = std::mem::take(&mut filler.shaped);
    let need_appearances = shaped.is_empty();
    shaped.draw(&mut filler.doc, limits.deadline)?;
    filler.set_need_appearances(need_appearances)?;
    let mut doc = filler.doc;
    if options.flatten {
        flatten(&mut doc, limits.deadline)?;
    }
    Ok(doc)
}

/// Resolves a dotted path (`customer.name`, `items.0.qty`) into the data.
fn lookup<'a>(data: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = data;
    for segment in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// A JSON value as the text a field stores.
fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Whether a value switches a checkbox with on-state `on` on.
fn truthy(value: &Value, on: &str) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => {
            let s = s.trim();
            s == on || matches!(s.to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1")
        }
        _ => false,
    }
}

/// The on-state of a checkbox or radio widget: the `/AP /N` key that is not
/// `Off`.
fn on_state(doc: &Document, widget: ObjectId) -> Option<String> {
    let dict = doc.get_dictionary(widget).ok()?;
    let states = get_dict(doc, get_dict(doc, dict, b"AP")?, b"N")?;
    states
        .iter()
        .map(|(key, _)| String::from_utf8_lossy(key).into_owned())
        .find(|key| key != "Off")
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Fill,
    Check,
}

/// A tag found in an annotation: the template to render and the shorthand
/// it ended with.
#[derive(Debug, PartialEq, Eq)]
struct Tag {
    source: String,
    mode: Mode,
}

fn parse_tag(contents: &str) -> Option<Tag> {
    let trimmed = contents.trim();
    if !text::is_template(trimmed) {
        return None;
    }
    let single = trimmed
        .strip_prefix("{{")
        .and_then(|s| s.strip_suffix("}}"))
        .filter(|inner| !inner.contains("{{"));
    if let Some((expr, filter)) = single.and_then(|inner| inner.rsplit_once('|')) {
        let mode = match filter.trim() {
            "fill" => Some(Mode::Fill),
            "check" => Some(Mode::Check),
            _ => None,
        };
        if let Some(mode) = mode {
            return Some(Tag {
                source: format!("{{{{ {} }}}}", expr.trim()),
                mode,
            });
        }
    }
    Some(Tag {
        source: trimmed.to_string(),
        mode: Mode::Fill,
    })
}

struct Filler<'a> {
    doc: Document,
    data: &'a Value,
    env: minijinja::Environment<'static>,
    helvetica: Option<ObjectId>,
    /// Text outside WinAnsi, drawn once every value is in.
    shaped: shaped::Pending,
    /// Instructions left for every tag together: a budget per tag would let a
    /// form with many tags spend without end.
    fuel: u64,
    /// Bytes left for every tag together.
    bytes_left: usize,
    tags: usize,
    limits: FillLimits,
}

impl Filler<'_> {
    fn render(&mut self, source: &str, what: &str) -> Result<String> {
        self.limits.deadline.check()?;
        self.tags += 1;
        if self.tags > MAX_FILL_TAGS {
            return Err(Error::InvalidRequest(format!(
                "the PDF has more than {MAX_FILL_TAGS} template tags"
            )));
        }
        if self.fuel == 0 {
            return Err(Error::Template(format!(
                "{what}: the tags of this PDF ran out of fuel"
            )));
        }
        self.env.set_fuel(Some(self.fuel));
        let template = self
            .env
            .template_from_str(source)
            .map_err(|e| Error::Template(format!("{what}: {e}")))?;
        let ctx = minijinja::Value::from(minijinja::value::Serde(self.data));
        match limit::render(&template, ctx, self.bytes_left) {
            Ok((output, captured)) => {
                let used = captured.state().fuel_levels().map_or(0, |(used, _)| used);
                drop(captured);
                self.fuel = self.fuel.saturating_sub(used);
                self.bytes_left = self.bytes_left.saturating_sub(output.len());
                Ok(output)
            }
            Err(RenderFailure::TooLarge(_)) => {
                Err(template::too_large(self.limits.max_render_bytes))
            }
            Err(RenderFailure::Template(e)) => Err(Error::Template(format!("{what}: {e}"))),
        }
    }

    /// The rendered value of a field whose `/V` or `/DV` is a template.
    fn template_value(&mut self, field: &Field) -> Result<Option<String>> {
        for candidate in [&field.value, &field.default] {
            if let Some(Object::String(bytes, _)) = candidate {
                let source = text::decode_text_string(bytes);
                if text::is_template(&source) {
                    let what = format!("field `{}`", field.name);
                    return self.render(&source, &what).map(Some);
                }
            }
        }
        Ok(None)
    }

    fn helvetica(&mut self) -> ObjectId {
        match self.helvetica {
            Some(id) => id,
            None => {
                let id = add_helvetica(&mut self.doc);
                self.helvetica = Some(id);
                id
            }
        }
    }

    fn set_entry(&mut self, id: ObjectId, key: &str, value: Object) -> Result<()> {
        self.doc
            .get_dictionary_mut(id)
            .map_err(malformed)?
            .set(key, value);
        Ok(())
    }

    /// Stores `value` in `field` and refreshes what the widgets show.
    fn apply(&mut self, field: &Field, value: &Value) -> Result<()> {
        match field.kind {
            Kind::Text => {
                let shown = value_text(value);
                self.set_entry(field.id, "V", text::encode_text_string(&shown))?;
                self.text_appearances(field, &shown)
            }
            Kind::Checkbox => {
                let mut state = "Off".to_string();
                for widget in &field.widgets {
                    let on = on_state(&self.doc, widget.id).unwrap_or_else(|| "Yes".into());
                    let checked = truthy(value, &on);
                    let name = if checked { on.as_str() } else { "Off" };
                    self.set_entry(widget.id, "AS", Object::Name(name.into()))?;
                    if checked {
                        state = on;
                    }
                }
                self.set_entry(field.id, "V", Object::Name(state.into_bytes()))
            }
            Kind::Radio => {
                let wanted = value_text(value);
                let mut chosen = None;
                let mut states = Vec::new();
                for widget in &field.widgets {
                    let on = on_state(&self.doc, widget.id).unwrap_or_else(|| "Off".into());
                    let hit = !wanted.is_empty() && on == wanted;
                    let name = if hit { on.as_str() } else { "Off" };
                    self.set_entry(widget.id, "AS", Object::Name(name.into()))?;
                    if hit {
                        chosen = Some(on.clone());
                    }
                    states.push(on);
                }
                match chosen {
                    Some(on) => self.set_entry(field.id, "V", Object::Name(on.into_bytes())),
                    None if wanted.is_empty() || wanted == "Off" => {
                        self.set_entry(field.id, "V", Object::Name(b"Off".to_vec()))
                    }
                    None => Err(Error::InvalidRequest(format!(
                        "field `{}`: `{wanted}` is not one of {}",
                        field.name,
                        states.join(", ")
                    ))),
                }
            }
            Kind::Choice => {
                let values: Vec<String> = match value {
                    Value::Array(items) => items.iter().map(value_text).collect(),
                    other => vec![value_text(other)],
                };
                if !field.has_flag(FF_EDIT)
                    && let Some(bad) = values.iter().find(|v| !field.options.contains(v))
                {
                    return Err(Error::InvalidRequest(format!(
                        "field `{}`: `{bad}` is not one of {}",
                        field.name,
                        field.options.join(", ")
                    )));
                }
                let stored = match values.as_slice() {
                    [single] => text::encode_text_string(single),
                    many => {
                        Object::Array(many.iter().map(|v| text::encode_text_string(v)).collect())
                    }
                };
                self.set_entry(field.id, "V", stored)?;
                if field.has_flag(FF_COMBO) {
                    let shown = values
                        .iter()
                        .map(|v| fields::display_text(&self.doc, field, v))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.text_appearances(field, &shown)?;
                }
                Ok(())
            }
            Kind::Signature | Kind::Button => Ok(()),
        }
    }

    /// The font the field's `/DA` names, when the form's `/DR` has it.
    fn form_font(&mut self, name: &str) -> Option<ObjectId> {
        let fonts = get_dict(&self.doc, acroform(&self.doc)?, b"DR")
            .and_then(|dr| get_dict(&self.doc, dr, b"Font"))?;
        match fonts.get(name.as_bytes()).ok()? {
            Object::Reference(id) => Some(*id),
            Object::Dictionary(dict) => {
                let dict = dict.clone();
                Some(self.doc.add_object(dict))
            }
            _ => None,
        }
    }

    /// The `/BaseFont` of a font in the form's `/DR`, read without changing
    /// anything.
    fn form_base_font(&self, name: &str) -> Option<String> {
        let fonts = get_dict(&self.doc, acroform(&self.doc)?, b"DR")
            .and_then(|dr| get_dict(&self.doc, dr, b"Font"))?;
        let font = get(&self.doc, fonts, name.as_bytes())?.as_dict().ok()?;
        get_name(&self.doc, font, b"BaseFont")
    }

    /// Queues the appearance of every widget of a text-like field whose
    /// value has characters Helvetica cannot show.
    fn shaped_appearances(&mut self, field: &Field, shown: &str) -> Result<()> {
        let (font_name, size, colour) = text::parse_da(field.da.as_deref().unwrap_or(""));
        let font = FontChoice::from_names(&font_name, self.form_base_font(&font_name).as_deref());
        let multiline = field.kind == Kind::Text && field.has_flag(FF_MULTILINE);
        for widget in &field.widgets {
            let Some(rect) = widget.rect else {
                continue;
            };
            let (width, height) = (rect.width(), rect.height());
            let background = self.widget_background(widget.id, width, height);
            self.shaped.widget(
                widget.id,
                background,
                TextBox {
                    text: shown.to_string(),
                    width,
                    height,
                    size,
                    align: Align::from_quadding(field.quadding),
                    multiline,
                    font,
                    colour: shaped::da_colour(&colour),
                },
            )?;
        }
        Ok(())
    }

    /// Writes a `/Tx BMC ... EMC` normal appearance showing `shown` into
    /// every widget of a text-like field. Text with characters outside
    /// WinAnsi is queued for [`shaped`] instead.
    fn text_appearances(&mut self, field: &Field, shown: &str) -> Result<()> {
        if shaped::needs_shaping(shown) {
            return self.shaped_appearances(field, shown);
        }
        let (font_name, size, colour) = text::parse_da(field.da.as_deref().unwrap_or(""));
        let font_id = match self.form_font(&font_name) {
            Some(id) => id,
            None => self.helvetica(),
        };
        let multiline = field.kind == Kind::Text && field.has_flag(FF_MULTILINE);
        for widget in &field.widgets {
            let Some(rect) = widget.rect else {
                continue;
            };
            self.shaped.forget(widget.id);
            let (w, h) = (rect.width(), rect.height());
            let mut content = self.widget_background(widget.id, w, h);
            let (size, lines) = text::layout(shown, w, h, size, field.quadding, multiline);
            let _ = writeln!(
                content,
                "/Tx BMC q 1 1 {} {} re W n",
                num((w - 2.0).max(0.0)),
                num((h - 2.0).max(0.0))
            );
            content.push_str(&text::text_block(&font_name, size, &colour, &lines));
            content.push_str("Q EMC\n");
            let stream = Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Form",
                    "BBox" => vec![0.into(), 0.into(), w.into(), h.into()],
                    "Resources" => dictionary! {
                        "Font" => dictionary! { font_name.as_str() => Object::Reference(font_id) },
                    },
                },
                content.into_bytes(),
            );
            let stream_id = self.doc.add_object(stream);
            self.set_entry(
                widget.id,
                "AP",
                dictionary! { "N" => Object::Reference(stream_id) }.into(),
            )?;
        }
        Ok(())
    }

    /// Background fill and border from the widget's `/MK`, so a regenerated
    /// appearance keeps the look the form designer gave the box.
    fn widget_background(&self, widget: ObjectId, w: f32, h: f32) -> String {
        let colour_op = |key: &[u8], fill: bool| -> Option<String> {
            let mk = get_dict(&self.doc, self.doc.get_dictionary(widget).ok()?, b"MK")?;
            let parts: Vec<String> = get(&self.doc, mk, key)?
                .as_array()
                .ok()?
                .iter()
                .filter_map(|n| deref(&self.doc, n).as_float().ok())
                .map(num)
                .collect();
            let op = match (parts.len(), fill) {
                (1, true) => "g",
                (1, false) => "G",
                (3, true) => "rg",
                (3, false) => "RG",
                (4, true) => "k",
                (4, false) => "K",
                _ => return None,
            };
            Some(format!("{} {op}", parts.join(" ")))
        };
        let mut out = String::new();
        if let Some(bg) = colour_op(b"BG", true) {
            let _ = writeln!(out, "q {bg} 0 0 {} {} re f Q", num(w), num(h));
        }
        if let Some(bc) = colour_op(b"BC", false) {
            let _ = writeln!(
                out,
                "q {bc} 1 w 0.5 0.5 {} {} re S Q",
                num((w - 1.0).max(0.0)),
                num((h - 1.0).max(0.0))
            );
        }
        out
    }

    /// Draws `shown` on the page inside `rect`, in Helvetica at `size`, or
    /// queues it for [`shaped`] when Helvetica cannot show it.
    fn draw_text(&mut self, page_id: ObjectId, rect: Rect, shown: &str, size: f32) -> Result<()> {
        if shown.trim().is_empty() {
            return Ok(());
        }
        if shaped::needs_shaping(shown) {
            return self.shaped.page(
                page_id,
                rect,
                TextBox {
                    text: shown.to_string(),
                    width: rect.width(),
                    height: rect.height(),
                    size: if size > 0.0 { size } else { 10.0 },
                    align: Align::Start,
                    multiline: true,
                    font: FontChoice::SANS,
                    colour: [0.0; 3],
                },
            );
        }
        let font_id = self.helvetica();
        add_page_resource(&mut self.doc, page_id, b"Font", "RdHelv", font_id)?;
        let size = if size > 0.0 { size } else { 10.0 };
        let (size, lines) = text::layout(shown, rect.width(), rect.height(), size, 0, true);
        let mut content = format!("\nq 1 0 0 1 {} {} cm\n", num(rect.x1), num(rect.y1));
        content.push_str(&text::text_block("RdHelv", size, "0 g", &lines));
        content.push_str("Q\n");
        self.doc
            .add_page_contents(page_id, content.into_bytes())
            .map_err(malformed)
    }

    fn annotation_tag(&self, dict: &Dictionary) -> Option<Tag> {
        let subtype = get_name(&self.doc, dict, b"Subtype")?;
        if !matches!(subtype.as_str(), "Text" | "FreeText" | "Square") {
            return None;
        }
        let contents = get(&self.doc, dict, b"Contents").and_then(|c| object_text(&self.doc, c))?;
        parse_tag(&contents)
    }

    /// Applies every tag annotation: into the widget under it, or onto the
    /// page when there is none.
    fn annotation_tags(&mut self, fields: &[Field], strip: bool) -> Result<()> {
        let pages: Vec<(u32, ObjectId)> = self.doc.get_pages().into_iter().collect();
        for (page_no, page_id) in pages {
            let entries = page_annots(&self.doc, page_id);
            let mut kept = Vec::with_capacity(entries.len());
            let mut stripped = false;
            for entry in entries {
                self.limits.deadline.check()?;
                let dict = match &entry {
                    Object::Reference(id) => self.doc.get_dictionary(*id).ok().cloned(),
                    Object::Dictionary(dict) => Some(dict.clone()),
                    _ => None,
                };
                let tag = dict.as_ref().and_then(|d| self.annotation_tag(d));
                let Some((dict, tag)) = dict.zip(tag) else {
                    kept.push(entry);
                    continue;
                };
                let what = format!("annotation on page {page_no}");
                let rendered = self.render(&tag.source, &what)?;
                let rect = dict
                    .get(b"Rect")
                    .ok()
                    .and_then(|r| Rect::from_object(&self.doc, r));
                let target = rect.and_then(|rect| widget_under(fields, page_id, &rect));
                match (target, rect) {
                    (Some(field), _) => self.apply(field, &Value::String(rendered))?,
                    (None, Some(rect)) => {
                        let shown = match tag.mode {
                            Mode::Check if truthy(&Value::String(rendered.clone()), "") => {
                                "X".into()
                            }
                            Mode::Check => String::new(),
                            Mode::Fill => rendered,
                        };
                        let da = get(&self.doc, &dict, b"DA")
                            .and_then(|da| object_text(&self.doc, da))
                            .unwrap_or_default();
                        let (_, size, _) = text::parse_da(&da);
                        self.draw_text(page_id, rect, &shown, size)?;
                    }
                    (None, None) => {}
                }
                if strip {
                    stripped = true;
                } else {
                    kept.push(entry);
                }
            }
            if stripped {
                set_page_annots(&mut self.doc, page_id, kept)?;
            }
        }
        Ok(())
    }

    /// Sets the form's `NeedAppearances`, or removes it.
    fn set_need_appearances(&mut self, on: bool) -> Result<()> {
        let apply = |form: &mut Dictionary| {
            if on {
                form.set("NeedAppearances", true);
            } else {
                form.remove(b"NeedAppearances");
            }
        };
        let catalog = self.doc.catalog().map_err(malformed)?;
        match catalog.get(b"AcroForm") {
            Ok(Object::Reference(id)) => {
                let id = *id;
                if let Ok(form) = self.doc.get_dictionary_mut(id) {
                    apply(form);
                }
            }
            Ok(Object::Dictionary(_)) => {
                apply(
                    self.doc
                        .catalog_mut()
                        .map_err(malformed)?
                        .get_mut(b"AcroForm")
                        .and_then(Object::as_dict_mut)
                        .map_err(malformed)?,
                );
            }
            _ => {}
        }
        Ok(())
    }
}

/// The field whose widget on `page_id` shares the most area with `rect`.
fn widget_under<'f>(fields: &'f [Field], page_id: ObjectId, rect: &Rect) -> Option<&'f Field> {
    fields
        .iter()
        .filter_map(|field| {
            let overlap = field
                .widgets
                .iter()
                .filter(|w| w.page == Some(page_id))
                .filter_map(|w| w.rect.map(|r| r.overlap(rect)))
                .fold(0.0f32, f32::max);
            (overlap > 0.0).then_some((field, overlap))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(field, _)| field)
}

/// The stream of a widget's normal appearance, picking the `/AS` state of a
/// state dictionary.
fn normal_appearance(doc: &Document, widget: &Dictionary) -> Option<ObjectId> {
    let pick = |states: &Dictionary| -> Option<ObjectId> {
        match get_name(doc, widget, b"AS") {
            Some(state) => states.get(state.as_bytes()).ok()?.as_reference().ok(),
            None if states.len() == 1 => states.iter().next()?.1.as_reference().ok(),
            None => None,
        }
    };
    let ap = get_dict(doc, widget, b"AP")?;
    match ap.get(b"N").ok()? {
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Stream(_) => Some(*id),
            Object::Dictionary(states) => pick(states),
            _ => None,
        },
        Object::Dictionary(states) => pick(states),
        _ => None,
    }
}

/// The `/BBox` and `/Matrix` of a form XObject.
fn form_geometry(doc: &Document, stream_id: ObjectId) -> Option<(Rect, [f32; 6])> {
    let stream = doc.get_object(stream_id).ok()?.as_stream().ok()?;
    let bbox = Rect::from_object(doc, stream.dict.get(b"BBox").ok()?)?;
    let mut matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    if let Some(Ok(items)) = get(doc, &stream.dict, b"Matrix").map(|m| m.as_array())
        && items.len() == 6
    {
        for (slot, item) in matrix.iter_mut().zip(items) {
            *slot = deref(doc, item).as_float().ok()?;
        }
    }
    Some((bbox, matrix))
}

/// Draws every widget's normal appearance into its page and removes the
/// widgets and the AcroForm, so the values can no longer change.
fn flatten(doc: &mut Document, deadline: Deadline) -> Result<()> {
    let pages: Vec<ObjectId> = doc.page_iter().collect();
    let mut counter = 0u32;
    for page_id in pages {
        deadline.check()?;
        let entries = page_annots(doc, page_id);
        if entries.is_empty() {
            continue;
        }
        let mut kept = Vec::with_capacity(entries.len());
        let mut ops = String::new();
        let mut removed = false;
        for entry in entries {
            let dict = match &entry {
                Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
                Object::Dictionary(dict) => Some(dict.clone()),
                _ => None,
            };
            let Some(dict) =
                dict.filter(|d| get_name(doc, d, b"Subtype").as_deref() == Some("Widget"))
            else {
                kept.push(entry);
                continue;
            };
            removed = true;
            let hidden = get_i64(doc, &dict, b"F").unwrap_or(0) & (2 | 32) != 0;
            let rect = dict
                .get(b"Rect")
                .ok()
                .and_then(|r| Rect::from_object(doc, r));
            let Some((rect, stream_id)) = rect.zip(normal_appearance(doc, &dict)) else {
                continue;
            };
            let Some((bbox, m)) = form_geometry(doc, stream_id).filter(|_| !hidden) else {
                continue;
            };
            let corners = [
                (bbox.x1, bbox.y1),
                (bbox.x2, bbox.y1),
                (bbox.x2, bbox.y2),
                (bbox.x1, bbox.y2),
            ]
            .map(|(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]));
            let (tx1, tx2) = corners.iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| {
                (lo.min(c.0), hi.max(c.0))
            });
            let (ty1, ty2) = corners.iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| {
                (lo.min(c.1), hi.max(c.1))
            });
            let sx = if tx2 - tx1 > 0.001 {
                rect.width() / (tx2 - tx1)
            } else {
                1.0
            };
            let sy = if ty2 - ty1 > 0.001 {
                rect.height() / (ty2 - ty1)
            } else {
                1.0
            };
            let (ex, ey) = (rect.x1 - tx1 * sx, rect.y1 - ty1 * sy);
            if let Ok(Object::Stream(stream)) = doc.get_object_mut(stream_id) {
                stream.dict.set("Type", "XObject");
                stream.dict.set("Subtype", "Form");
            }
            counter += 1;
            let name = format!("RdFm{counter}");
            add_page_resource(doc, page_id, b"XObject", &name, stream_id)?;
            let _ = writeln!(
                ops,
                "q {} 0 0 {} {} {} cm /{name} Do Q",
                num(sx),
                num(sy),
                num(ex),
                num(ey)
            );
        }
        if removed {
            set_page_annots(doc, page_id, kept)?;
        }
        if !ops.is_empty() {
            super::isolate_page_content(doc, page_id)?;
            doc.add_page_contents(page_id, format!("\n{ops}").into_bytes())
                .map_err(malformed)?;
        }
    }
    doc.catalog_mut().map_err(malformed)?.remove(b"AcroForm");
    doc.prune_objects();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with `count` annotations that each hold `tag`.
    fn tagged(count: usize, tag: &str) -> Vec<u8> {
        let mut pdf = Document::load_mem(&crate::pdfops::fixtures::pages(1)).unwrap();
        let page = *pdf.get_pages().get(&1).unwrap();
        let annots: Vec<Object> = (0..count)
            .map(|_| {
                dictionary! {
                    "Type" => "Annot", "Subtype" => "Text",
                    "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
                    "Contents" => Object::string_literal(tag),
                }
                .into()
            })
            .collect();
        pdf.get_dictionary_mut(page).unwrap().set("Annots", annots);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).unwrap();
        bytes
    }

    fn fill_it(pdf: &[u8], limits: FillLimits) -> Result<Vec<u8>> {
        super::super::fill_within(
            pdf,
            &serde_json::json!({}),
            &template::environment(),
            &FillOptions {
                flatten: false,
                strip_tag_annotations: true,
            },
            limits,
        )
    }

    #[test]
    fn every_tag_of_a_form_shares_one_fuel_budget() {
        // Each tag alone fits the budget; with one budget per tag, these two
        // hundred would run for many seconds.
        let tag = "{% for i in range(100000) %}{% for j in range(10) %}{% endfor %}{% endfor %}x";
        let started = std::time::Instant::now();
        let err = fill_it(&tagged(200, tag), FillLimits::default()).unwrap_err();
        assert!(matches!(err, Error::Template(_)), "{err}");
        let single = fill_it(&tagged(1, tag), FillLimits::default());
        assert!(single.is_ok(), "{:?}", single.err());
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
    }

    #[test]
    fn a_form_may_hold_at_most_so_many_tags() {
        let err =
            fill_it(&tagged(MAX_FILL_TAGS + 1, "{{ 1 }}"), FillLimits::default()).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
        assert!(fill_it(&tagged(MAX_FILL_TAGS, "{{ 1 }}"), FillLimits::default()).is_ok());
    }

    #[test]
    fn the_output_of_every_tag_counts_against_one_limit() {
        let tag = r#"{{ "a" * 1000 }}"#;
        let limits = FillLimits {
            max_render_bytes: 5_000,
            ..FillLimits::default()
        };
        let err = fill_it(&tagged(10, tag), limits).unwrap_err();
        assert!(err.to_string().contains("byte limit"), "{err}");
    }

    #[test]
    fn a_form_stops_at_its_deadline() {
        let limits = FillLimits {
            deadline: Deadline::new(std::time::Duration::ZERO),
            ..FillLimits::default()
        };
        let err = fill_it(&tagged(1, "{{ 1 }}"), limits).unwrap_err();
        assert!(matches!(err, Error::Timeout(_)), "{err}");
    }

    #[test]
    fn lookup_walks_objects_and_arrays() {
        let data = serde_json::json!({"customer": {"name": "Ada"}, "items": [{"qty": 3}]});
        assert_eq!(lookup(&data, "customer.name"), Some(&Value::from("Ada")));
        assert_eq!(lookup(&data, "items.0.qty"), Some(&Value::from(3)));
        assert_eq!(lookup(&data, "items.1.qty"), None);
        assert_eq!(lookup(&data, "customer.phone"), None);
        assert_eq!(lookup(&data, ""), None);
    }

    #[test]
    fn tags_and_shorthands() {
        assert_eq!(parse_tag("hello"), None);
        assert_eq!(
            parse_tag(" {{ paid | check }} "),
            Some(Tag {
                source: "{{ paid }}".into(),
                mode: Mode::Check
            })
        );
        assert_eq!(
            parse_tag("{{ total | round(2) }}"),
            Some(Tag {
                source: "{{ total | round(2) }}".into(),
                mode: Mode::Fill
            })
        );
        assert_eq!(
            parse_tag("{{ a }} and {{ b | check }}"),
            Some(Tag {
                source: "{{ a }} and {{ b | check }}".into(),
                mode: Mode::Fill
            })
        );
    }

    #[test]
    fn truthiness() {
        assert!(truthy(&Value::Bool(true), "Yes"));
        assert!(truthy(&Value::from("yes"), "Yes"));
        assert!(truthy(&Value::from("Ja"), "Ja"));
        assert!(truthy(&Value::from(1), "Yes"));
        assert!(!truthy(&Value::from("no"), "Yes"));
        assert!(!truthy(&Value::Null, "Yes"));
    }
}
