//! Content-stream drawing for the lopdf engine: flattening form widgets,
//! text and image stamps, and laying one PDF over another.
//!
//! Every page keeps its original content untouched: it is wrapped in
//! `q`/`Q` so the drawing before it (a watermark) or after it (a stamp)
//! starts from a clean graphics state.

use std::fmt::Write as _;

use ::lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, dictionary};

use super::lopdf::{DEFAULT_MAX_BYTES, engine_error, inherited, load, page_content_capped};
use super::{Deadline, Overlay, OverlayKind, Placement, Position, pages};
use crate::error::{Error, Result};
use crate::job::Document;
use crate::limits::{MAX_OUTPUT_BYTES, charge_output};

/// Distance from the page edge for anchored stamps, in points.
const MARGIN: f32 = 20.0;

/// Advance widths of Helvetica for the printable ASCII range, in 1/1000 em.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

fn width(c: char) -> f32 {
    match c as u32 {
        32..=126 => HELVETICA[c as usize - 32] as f32,
        _ => 556.0,
    }
}

/// A character in WinAnsiEncoding; `?` for anything outside it.
fn winansi(c: char) -> u8 {
    match c {
        '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => c as u8,
        '€' => 0x80,
        '…' => 0x85,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        _ => b'?',
    }
}

pub(super) fn num(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// The page's media box, as `[x1, y1, x2, y2]`; US Letter when absent.
fn page_box(pdf: &Pdf, page_id: ObjectId) -> [f32; 4] {
    inherited(pdf, page_id, b"MediaBox")
        .and_then(|m| floats(&m, 4))
        .map(|b| {
            [
                b[0].min(b[2]),
                b[1].min(b[3]),
                b[0].max(b[2]),
                b[1].max(b[3]),
            ]
        })
        .unwrap_or([0.0, 0.0, 612.0, 792.0])
}

fn floats(object: &Object, n: usize) -> Option<Vec<f32>> {
    let items = object.as_array().ok()?;
    if items.len() < n {
        return None;
    }
    items.iter().take(n).map(|o| o.as_float().ok()).collect()
}

/// Registers `id` under `name` in the page's `/Resources/<category>`,
/// whether that sub-dictionary is direct or referenced.
fn add_resource(
    pdf: &mut Pdf,
    page_id: ObjectId,
    category: &str,
    name: &str,
    id: ObjectId,
) -> Result<()> {
    let sub = {
        let resources = pdf
            .get_or_create_resources(page_id)
            .and_then(Object::as_dict_mut)?;
        if !resources.has(category.as_bytes()) {
            resources.set(category, Dictionary::new());
        }
        match resources.get(category.as_bytes())? {
            Object::Reference(id) => Some(*id),
            _ => None,
        }
    };
    let dict = match sub {
        Some(id) => pdf.get_object_mut(id).and_then(Object::as_dict_mut)?,
        None => pdf
            .get_or_create_resources(page_id)
            .and_then(Object::as_dict_mut)?
            .get_mut(category.as_bytes())
            .and_then(Object::as_dict_mut)?,
    };
    dict.set(name, id);
    Ok(())
}

/// Puts `before` ahead of the page's content streams and `after` behind
/// them.
fn wrap_content(pdf: &mut Pdf, page_id: ObjectId, before: Vec<u8>, after: Vec<u8>) -> Result<()> {
    let page = pdf.get_dictionary(page_id)?;
    let existing: Vec<Object> = match page.get(b"Contents") {
        Ok(Object::Reference(id)) => match pdf.get_object(*id) {
            Ok(Object::Array(list)) => list.clone(),
            _ => vec![Object::Reference(*id)],
        },
        Ok(Object::Array(list)) => list.clone(),
        _ => Vec::new(),
    };
    let head = pdf.add_object(Stream::new(Dictionary::new(), before));
    let tail = pdf.add_object(Stream::new(Dictionary::new(), after));
    let mut list = vec![Object::Reference(head)];
    list.extend(existing);
    list.push(Object::Reference(tail));
    pdf.get_object_mut(page_id)
        .and_then(Object::as_dict_mut)?
        .set("Contents", list);
    Ok(())
}

// ---------------------------------------------------------------- flatten

/// Draws every widget's normal appearance into its page and removes the
/// widgets and the AcroForm. Widgets without an appearance stream stay,
/// and so does the AcroForm while any remain.
pub(super) fn flatten(pdf: &mut Pdf, deadline: Deadline) -> Result<()> {
    let mut widgets_left = false;
    for page_id in pdf.page_iter().collect::<Vec<_>>() {
        deadline.check()?;
        let annots: Vec<Object> = match pdf.get_dictionary(page_id)?.get(b"Annots") {
            Ok(a) => match pdf.dereference(a) {
                Ok((_, Object::Array(list))) => list.clone(),
                _ => Vec::new(),
            },
            Err(_) => continue,
        };
        let mut kept = Vec::new();
        let mut drawing = String::new();
        for annot in annots {
            let dict = match pdf.dereference(&annot) {
                Ok((_, Object::Dictionary(dict))) => dict.clone(),
                _ => {
                    kept.push(annot);
                    continue;
                }
            };
            let is_widget = dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Widget");
            let Some((stream_id, bbox, matrix)) =
                (is_widget).then(|| appearance(pdf, &dict)).flatten()
            else {
                widgets_left |= is_widget;
                kept.push(annot);
                continue;
            };
            let flags = dict.get(b"F").and_then(Object::as_i64).unwrap_or(0);
            let rect: Option<[f32; 4]> = dict
                .get(b"Rect")
                .ok()
                .and_then(|r| floats(r, 4))
                .and_then(|r| r.try_into().ok());
            // Hidden and NoView widgets are dropped without drawing.
            let Some(rect) = rect.filter(|_| flags & 0b10_0010 == 0) else {
                continue;
            };
            let name = format!("FxAP{}", stream_id.0);
            add_resource(pdf, page_id, "XObject", &name, stream_id)?;
            let m = placement(bbox, matrix, rect);
            let _ = writeln!(
                drawing,
                "q {} cm /{name} Do Q",
                m.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" ")
            );
        }
        if !drawing.is_empty() {
            let mut after = b"Q\n".to_vec();
            after.extend_from_slice(drawing.as_bytes());
            wrap_content(pdf, page_id, b"q\n".to_vec(), after)?;
        }
        let page = pdf.get_object_mut(page_id).and_then(Object::as_dict_mut)?;
        if kept.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", kept);
        }
    }
    if !widgets_left {
        pdf.catalog_mut()?.remove(b"AcroForm");
    }
    Ok(())
}

/// The widget's normal appearance stream (by `/AS` for multi-state
/// widgets), its `BBox` and `Matrix`.
fn appearance(pdf: &mut Pdf, annot: &Dictionary) -> Option<(ObjectId, [f32; 4], [f32; 6])> {
    let ap = pdf
        .dereference(annot.get(b"AP").ok()?)
        .ok()?
        .1
        .as_dict()
        .ok()?;
    let normal = ap.get(b"N").ok()?;
    let stream_id = match normal {
        Object::Reference(id) => match pdf.get_object(*id).ok()? {
            Object::Stream(_) => *id,
            Object::Dictionary(states) => state_stream(annot, states)?,
            _ => return None,
        },
        Object::Dictionary(states) => state_stream(annot, states)?,
        _ => return None,
    };
    let stream = pdf.get_object_mut(stream_id).ok()?.as_stream_mut().ok()?;
    let bbox: [f32; 4] = floats(stream.dict.get(b"BBox").ok()?, 4)?.try_into().ok()?;
    let matrix: [f32; 6] = stream
        .dict
        .get(b"Matrix")
        .ok()
        .and_then(|m| floats(m, 6))
        .and_then(|m| m.try_into().ok())
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    stream.dict.set("Type", "XObject");
    stream.dict.set("Subtype", "Form");
    Some((stream_id, bbox, matrix))
}

fn state_stream(annot: &Dictionary, states: &Dictionary) -> Option<ObjectId> {
    let state = annot.get(b"AS").and_then(Object::as_name).ok();
    match state {
        Some(state) => states.get(state).ok()?.as_reference().ok(),
        None if states.len() == 1 => states.iter().next()?.1.as_reference().ok(),
        None => None,
    }
}

/// The matrix that maps a form's transformed `BBox` onto `rect`
/// (ISO 32000-1, 12.5.5).
fn placement(bbox: [f32; 4], m: [f32; 6], rect: [f32; 4]) -> [f32; 6] {
    let corners = [
        (bbox[0], bbox[1]),
        (bbox[2], bbox[1]),
        (bbox[2], bbox[3]),
        (bbox[0], bbox[3]),
    ]
    .map(|(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]));
    let (bx1, by1) = corners
        .iter()
        .fold((f32::MAX, f32::MAX), |a, c| (a.0.min(c.0), a.1.min(c.1)));
    let (bx2, by2) = corners
        .iter()
        .fold((f32::MIN, f32::MIN), |a, c| (a.0.max(c.0), a.1.max(c.1)));
    let (rx1, ry1, rx2, ry2) = (
        rect[0].min(rect[2]),
        rect[1].min(rect[3]),
        rect[0].max(rect[2]),
        rect[1].max(rect[3]),
    );
    let sx = if bx2 - bx1 > 0.0 {
        (rx2 - rx1) / (bx2 - bx1)
    } else {
        1.0
    };
    let sy = if by2 - by1 > 0.0 {
        (ry2 - ry1) / (by2 - by1)
    } else {
        1.0
    };
    [sx, 0.0, 0.0, sy, rx1 - bx1 * sx, ry1 - by1 * sy]
}

// --------------------------------------------------------- stamps, overlays

enum Art {
    Text {
        font: ObjectId,
        text: String,
        size: f32,
        rgb: [f32; 3],
    },
    Image {
        id: ObjectId,
        width: f32,
        height: f32,
    },
    /// One form XObject per overlay page, in page order.
    Forms(Vec<ObjectId>),
}

/// Draws `overlay` on the selected pages: over the content when `over`
/// (a stamp), under it otherwise (a watermark). PDF overlays are drawn at
/// their own size over the page of the same number; position and scale do
/// not apply to them.
pub(super) fn overlay(
    pdf: &mut Pdf,
    overlay: &Overlay,
    over: bool,
    deadline: Deadline,
) -> Result<()> {
    overlay_within(pdf, overlay, over, deadline, MAX_OUTPUT_BYTES)
}

/// As [`overlay`], drawing at most `budget` bytes of content in all: the text is
/// written once per page.
pub(super) fn overlay_within(
    pdf: &mut Pdf,
    overlay: &Overlay,
    over: bool,
    deadline: Deadline,
    budget: usize,
) -> Result<()> {
    overlay.validate()?;
    let mut drawn = 0;
    let page_ids = pdf.get_pages();
    let selected = pages::select(&overlay.pages, page_ids.len() as u32)?;
    let opacity = overlay.opacity.clamp(0.0, 1.0);
    let gs_id = pdf.add_object(dictionary! {
        "Type" => "ExtGState", "CA" => opacity, "ca" => opacity,
    });
    let gs_name = format!("FxGS{}", gs_id.0);
    let art = match &overlay.kind {
        OverlayKind::Text {
            text,
            font_size,
            colour,
        } => Art::Text {
            font: pdf.add_object(dictionary! {
                "Type" => "Font", "Subtype" => "Type1",
                "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
            }),
            text: text.clone(),
            size: *font_size,
            rgb: parse_colour(colour)?,
        },
        OverlayKind::Image { bytes, name } => {
            let stream = ::lopdf::xobject::image_from(bytes.clone()).map_err(|e| {
                Error::InvalidRequest(format!("'{name}' is not a usable PNG or JPEG: {e}"))
            })?;
            let width = stream.dict.get(b"Width").and_then(Object::as_i64)? as f32;
            let height = stream.dict.get(b"Height").and_then(Object::as_i64)? as f32;
            Art::Image {
                id: pdf.add_object(stream),
                width,
                height,
            }
        }
        OverlayKind::Pdf { bytes, name } => Art::Forms(import_pages(
            pdf,
            &Document::new(name.clone(), bytes.clone()),
            deadline,
        )?),
    };
    for n in selected {
        deadline.check()?;
        let page_id = page_ids[&n];
        add_resource(pdf, page_id, "ExtGState", &gs_name, gs_id)?;
        let bx = page_box(pdf, page_id);
        let pw = bx[2] - bx[0];
        let (sin, cos) = overlay.rotation.to_radians().sin_cos();
        let mut content = format!("q /{gs_name} gs ");
        match &art {
            Art::Text {
                font,
                text,
                size,
                rgb,
            } => {
                let name = format!("FxF{}", font.0);
                add_resource(pdf, page_id, "Font", &name, *font)?;
                let unit: f32 = text.chars().map(width).sum::<f32>() / 1000.0;
                let size = match overlay.placement {
                    Some(Placement { width: Some(w), .. }) if unit > 0.0 => w / unit,
                    Some(_) => *size,
                    None if overlay.scale > 0.0 && unit > 0.0 => overlay.scale * pw / unit,
                    None => *size,
                };
                let (w, h) = (unit * size, size);
                let (cx, cy) = centre(overlay, bx, w, h);
                let _ = write!(
                    content,
                    "1 0 0 1 {} {} cm {} {} {} {} 0 0 cm BT /{name} {} Tf {} {} {} rg {} {} Td (",
                    num(cx),
                    num(cy),
                    num(cos),
                    num(sin),
                    num(-sin),
                    num(cos),
                    num(size),
                    num(rgb[0]),
                    num(rgb[1]),
                    num(rgb[2]),
                    num(-w / 2.0),
                    num(-h * 0.35)
                );
                let mut bytes = content.into_bytes();
                for c in text.chars() {
                    let b = winansi(c);
                    if matches!(b, b'(' | b')' | b'\\') {
                        bytes.push(b'\\');
                    }
                    bytes.push(b);
                }
                bytes.extend_from_slice(b") Tj ET Q\n");
                charge_output(&mut drawn, bytes.len(), budget)?;
                place(pdf, page_id, bytes, over)?;
                continue;
            }
            Art::Image { id, width, height } => {
                let name = format!("FxI{}", id.0);
                add_resource(pdf, page_id, "XObject", &name, *id)?;
                let w = match overlay.placement {
                    Some(Placement { width: Some(w), .. }) => w,
                    Some(_) => *width,
                    None if overlay.scale > 0.0 => overlay.scale * pw,
                    None => *width,
                };
                let h = w * height / width;
                let (cx, cy) = centre(overlay, bx, w, h);
                let _ = writeln!(
                    content,
                    "1 0 0 1 {} {} cm {} {} {} {} 0 0 cm {} 0 0 {} {} {} cm /{name} Do Q",
                    num(cx),
                    num(cy),
                    num(cos),
                    num(sin),
                    num(-sin),
                    num(cos),
                    num(w),
                    num(h),
                    num(-w / 2.0),
                    num(-h / 2.0)
                );
            }
            Art::Forms(forms) => {
                let Some(form) = forms.get(n as usize - 1) else {
                    continue;
                };
                let name = format!("FxX{}", form.0);
                add_resource(pdf, page_id, "XObject", &name, *form)?;
                let _ = writeln!(
                    content,
                    "1 0 0 1 {} {} cm /{name} Do Q",
                    num(bx[0]),
                    num(bx[1])
                );
            }
        }
        place(pdf, page_id, content.into_bytes(), over)?;
    }
    if matches!(art, Art::Forms(_)) {
        pdf.prune_objects();
    }
    Ok(())
}

fn place(pdf: &mut Pdf, page_id: ObjectId, drawing: Vec<u8>, over: bool) -> Result<()> {
    if over {
        let mut after = b"Q\n".to_vec();
        after.extend(drawing);
        wrap_content(pdf, page_id, b"q\n".to_vec(), after)
    } else {
        let mut before = drawing;
        before.extend_from_slice(b"q\n");
        wrap_content(pdf, page_id, before, b"Q\n".to_vec())
    }
}

/// The centre of a `w` by `h` mark: at its exact placement, or at its
/// position inside the page box.
fn centre(overlay: &Overlay, bx: [f32; 4], w: f32, h: f32) -> (f32, f32) {
    match overlay.placement {
        Some(p) => (bx[0] + p.x + w / 2.0, bx[1] + p.y + h / 2.0),
        None => anchor(overlay.position, bx, w, h),
    }
}

/// The centre of a `w` by `h` box at `position` inside the page box.
fn anchor(position: Position, bx: [f32; 4], w: f32, h: f32) -> (f32, f32) {
    use Position::*;
    let x = match position {
        TopLeft | BottomLeft => bx[0] + MARGIN + w / 2.0,
        TopRight | BottomRight => bx[2] - MARGIN - w / 2.0,
        _ => (bx[0] + bx[2]) / 2.0,
    };
    let y = match position {
        TopLeft | TopCenter | TopRight => bx[3] - MARGIN - h / 2.0,
        BottomLeft | BottomCenter | BottomRight => bx[1] + MARGIN + h / 2.0,
        _ => (bx[1] + bx[3]) / 2.0,
    };
    (x, y)
}

pub(super) fn parse_colour(hex: &str) -> Result<[f32; 3]> {
    let trimmed = hex.trim();
    let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
    let bad = || Error::InvalidRequest(format!("colour '{hex}' is not #rrggbb"));
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let mut rgb = [0.0; 3];
    for (i, slot) in rgb.iter_mut().enumerate() {
        *slot =
            u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).map_err(|_| bad())? as f32 / 255.0;
    }
    Ok(rgb)
}

/// Imports every page of `source` into `pdf` as a form XObject and returns
/// their ids in page order.
fn import_pages(pdf: &mut Pdf, source: &Document, deadline: Deadline) -> Result<Vec<ObjectId>> {
    let mut src = load(source)?;
    src.renumber_objects_with(pdf.max_id + 1);
    let mut forms = Vec::new();
    let mut budget = DEFAULT_MAX_BYTES;
    for page_id in src.page_iter().collect::<Vec<_>>() {
        deadline.check()?;
        let bbox = page_box(&src, page_id);
        let resources = match src.get_page_resources(page_id).map_err(engine_error)? {
            (Some(direct), _) => Object::Dictionary(direct.clone()),
            (None, ids) if !ids.is_empty() => Object::Reference(ids[0]),
            _ => Object::Dictionary(Dictionary::new()),
        };
        let mut form = Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "BBox" => bbox.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
                "Resources" => resources,
            },
            page_content_capped(&src, page_id, &mut budget)?,
        );
        let _ = form.compress();
        forms.push(form);
    }
    pdf.objects.extend(src.objects);
    pdf.max_id = pdf.max_id.max(src.max_id);
    Ok(forms.into_iter().map(|f| pdf.add_object(f)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_maps_bbox_onto_rect() {
        let m = placement(
            [0.0, 0.0, 100.0, 20.0],
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            [50.0, 700.0, 250.0, 740.0],
        );
        assert_eq!(m, [2.0, 0.0, 0.0, 2.0, 50.0, 700.0]);
        assert_eq!(parse_colour("#ff0000").unwrap(), [1.0, 0.0, 0.0]);
        assert!(parse_colour("red").is_err());
        assert!(parse_colour("aé123").is_err(), "a split character");
        assert!(parse_colour("##ff0000").is_err());
        assert!(parse_colour("#ff0000, url:example.com").is_err());
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.5), "-0.5");
    }

    #[test]
    fn stamped_text_stops_at_the_output_budget() {
        let mut pdf = Pdf::load_mem(&crate::pdfops::fixtures::pages(100)).unwrap();
        let mark = Overlay {
            placement: None,
            kind: OverlayKind::Text {
                text: "x".repeat(64 * 1024),
                font_size: 24.0,
                colour: "#000000".into(),
            },
            position: Position::Center,
            opacity: 1.0,
            rotation: 0.0,
            scale: 0.5,
            pages: String::new(),
        };
        let deadline = crate::pdfops::fixtures::far();
        let err = overlay_within(&mut pdf, &mark, true, deadline, 1 << 20).unwrap_err();
        assert!(err.to_string().contains("limit"), "{err}");
    }
}
