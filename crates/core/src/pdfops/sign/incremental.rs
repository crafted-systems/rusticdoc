//! The incremental update that carries a signature. The original bytes are
//! kept as they are and the update is appended after them: the signature
//! field, its widget on a page, the AcroForm that lists it, the `/Sig`
//! dictionary with its `/ByteRange` and `/Contents` placeholder, and a
//! cross-reference section of the same kind as the original's last one.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::ops::Range;
use std::time::SystemTime;

use ::lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, Writer, dictionary};
use sha2::{Digest, Sha256};

use super::SignOptions;
use crate::error::{Error, Result};
use crate::pdfops::draw::num;

/// Room for `0 a b c` with each offset up to ten digits.
const BYTE_RANGE_WIDTH: usize = 1 + 3 * 11;

/// `/F 132`: Print and Locked, as other signers set on signature widgets.
const WIDGET_FLAGS: i64 = 132;

/// The document with the update appended, before the signature is in it.
pub(super) struct Update {
    pub bytes: Vec<u8>,
    /// The hex digits of `/Contents`, between its `<` and `>`.
    pub contents: Range<usize>,
}

impl Update {
    /// The signed bytes: everything but the `/Contents` value and its brackets.
    pub fn signed_ranges(&self) -> [&[u8]; 2] {
        [
            &self.bytes[..self.contents.start - 1],
            &self.bytes[self.contents.end + 1..],
        ]
    }
}

fn invalid(name: &str, what: impl std::fmt::Display) -> Error {
    Error::InvalidRequest(format!("'{name}' cannot be signed: {what}"))
}

fn writing(e: impl std::fmt::Display) -> Error {
    Error::Engine {
        engine: "signer",
        message: format!("cannot write the signature update: {e}"),
    }
}

/// The offset the file's last `startxref` points at.
fn last_startxref(bytes: &[u8]) -> Option<usize> {
    let at = bytes.windows(9).rposition(|w| w == b"startxref")?;
    let digits: Vec<u8> = bytes[at + 9..]
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .take_while(u8::is_ascii_digit)
        .collect();
    std::str::from_utf8(&digits).ok()?.parse().ok()
}

/// An array in `dict` under `key`, followed through one reference. Returns the
/// array and, when it is an object of its own, its id.
fn array_in(pdf: &Pdf, dict: &Dictionary, key: &[u8]) -> (Vec<Object>, Option<ObjectId>) {
    match dict.get(key) {
        Ok(Object::Reference(id)) => match pdf.get_object(*id).and_then(Object::as_array) {
            Ok(array) => (array.clone(), Some(*id)),
            Err(_) => (Vec::new(), None),
        },
        Ok(Object::Array(array)) => (array.clone(), None),
        _ => (Vec::new(), None),
    }
}

/// `Signature1`, or the first `SignatureN` no top-level field uses.
fn field_name(pdf: &Pdf, fields: &[Object]) -> String {
    let taken: Vec<Vec<u8>> = fields
        .iter()
        .filter_map(|f| match f {
            Object::Reference(id) => pdf.get_dictionary(*id).ok(),
            Object::Dictionary(d) => Some(d),
            _ => None,
        })
        .filter_map(|d| d.get(b"T").and_then(Object::as_str).ok())
        .map(<[u8]>::to_vec)
        .collect();
    (1..)
        .map(|n| format!("Signature{n}"))
        .find(|name| !taken.iter().any(|t| t == name.as_bytes()))
        .expect("a free name")
}

/// A PDF date such as `D:20261107120000Z`.
fn pdf_date(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .unwrap_or_default()
        .format("D:%Y%m%d%H%M%SZ")
        .to_string()
}

/// One object as `N G obj ... endobj`, framed as `lopdf` frames its own.
fn write_object(out: &mut Vec<u8>, id: ObjectId, object: &Object) -> Result<()> {
    write!(
        out,
        "{} {} obj\n{}",
        id.0,
        id.1,
        if Writer::need_separator(object) {
            " "
        } else {
            ""
        }
    )
    .map_err(writing)?;
    Writer::write_object(out, object).map_err(writing)?;
    write!(
        out,
        "{}\nendobj\n",
        if Writer::need_end_separator(object) {
            " "
        } else {
            ""
        }
    )
    .map_err(writing)
}

/// Consecutive runs of object numbers, as `(first, count)`.
fn runs(ids: &[u32]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &id in ids {
        match runs.last_mut() {
            Some((first, count)) if *first + *count == id => *count += 1,
            _ => runs.push((id, 1)),
        }
    }
    runs
}

/// Appends the signature update to `original`, with `reserve` bytes for the
/// DER signature in `/Contents`. `pdf` is `original` parsed.
pub(super) fn prepare(
    original: &[u8],
    pdf: &Pdf,
    name: &str,
    options: &SignOptions,
    reserve: usize,
    now: SystemTime,
) -> Result<Update> {
    let prev = last_startxref(original)
        .filter(|&at| at < original.len())
        .ok_or_else(|| invalid(name, "it has no cross-reference section to append to"))?;
    let xref_stream = !original[prev..].trim_ascii_start().starts_with(b"xref");

    let root_id = pdf
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| invalid(name, "its trailer names no catalog"))?;
    let mut catalog = pdf
        .get_dictionary(root_id)
        .map_err(|_| invalid(name, "its catalog is not a dictionary"))?
        .clone();
    let size = pdf
        .trailer
        .get(b"Size")
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .max(0) as u32;
    let mut next = size.max(pdf.max_id + 1);
    let mut alloc = || {
        let id = (next, 0);
        next += 1;
        id
    };
    // What the update rewrites or adds, by id, so nothing is written twice.
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let sig_id = alloc();
    let field_id = alloc();

    // The page the widget sits on: the appearance's, or the first for an
    // invisible signature, which still needs a widget in some page's /Annots.
    let pages = pdf.get_pages();
    let page_number = options.appearance.as_ref().map_or(1, |a| a.page);
    let page_id = *pages.get(&page_number).ok_or_else(|| {
        Error::InvalidRequest(format!(
            "'{name}' has {} pages; there is no page {page_number} to sign on",
            pages.len()
        ))
    })?;

    let (rect, form) = match &options.appearance {
        Some(appearance) => {
            let mut image =
                ::lopdf::xobject::image_from(appearance.image.clone()).map_err(|e| {
                    Error::InvalidRequest(format!(
                        "'{}' is not a usable PNG or JPEG: {e}",
                        appearance.name
                    ))
                })?;
            let px = |key: &[u8]| {
                image
                    .dict
                    .get(key)
                    .and_then(Object::as_i64)
                    .map(|v| v as f32)
                    .unwrap_or(1.0)
                    .max(1.0)
            };
            let (pw, ph) = (px(b"Width"), px(b"Height"));
            let w = appearance.width.unwrap_or(pw);
            let h = w * ph / pw;
            if image.dict.get(b"Filter").is_err() {
                image.compress().map_err(writing)?;
            }
            let image_id = alloc();
            objects.insert(image_id, Object::Stream(image));
            let (x, y) = (appearance.x, appearance.y);
            let rect = [x, y, x + w, y + h];
            let content = format!("q {} 0 0 {} 0 0 cm /Img Do Q", num(w), num(h));
            let form = Stream::new(
                dictionary! {
                    "Type" => "XObject", "Subtype" => "Form",
                    "BBox" => vec![0.into(), 0.into(), w.into(), h.into()],
                    "Resources" => dictionary! { "XObject" => dictionary! { "Img" => image_id } },
                },
                content.into_bytes(),
            );
            (rect, form)
        }
        // An empty appearance: PDF/A and some validators want one even on an
        // invisible widget.
        None => (
            [0.0; 4],
            Stream::new(
                dictionary! {
                    "Type" => "XObject", "Subtype" => "Form",
                    "BBox" => vec![0.into(), 0.into(), 0.into(), 0.into()],
                },
                Vec::new(),
            ),
        ),
    };
    let form_id = alloc();
    objects.insert(form_id, Object::Stream(form));

    // The AcroForm, inline in the catalog, an object of its own, or new.
    let (acroform_id, mut acroform) = match catalog.get(b"AcroForm") {
        Ok(Object::Reference(id)) => (
            Some(*id),
            pdf.get_dictionary(*id).cloned().unwrap_or_default(),
        ),
        Ok(Object::Dictionary(d)) => (None, d.clone()),
        _ => (None, Dictionary::new()),
    };
    let (mut fields, fields_id) = array_in(pdf, &acroform, b"Fields");
    let field_name = field_name(pdf, &fields);
    fields.push(field_id.into());
    match fields_id {
        Some(id) => {
            objects.insert(id, Object::Array(fields));
        }
        None => acroform.set("Fields", fields),
    }
    // SignaturesExist and AppendOnly: later changes must be incremental too.
    acroform.set("SigFlags", 3);
    match acroform_id {
        Some(id) => {
            objects.insert(id, Object::Dictionary(acroform));
        }
        None => {
            let id = alloc();
            objects.insert(id, Object::Dictionary(acroform));
            catalog.set("AcroForm", id);
            objects.insert(root_id, Object::Dictionary(catalog));
        }
    }

    // The widget, in the page's /Annots.
    let mut page = pdf
        .get_dictionary(page_id)
        .map_err(|_| invalid(name, "a page is not a dictionary"))?
        .clone();
    let (mut annots, annots_id) = array_in(pdf, &page, b"Annots");
    annots.push(field_id.into());
    match annots_id {
        Some(id) => {
            objects.insert(id, Object::Array(annots));
        }
        None => {
            page.set("Annots", annots);
            objects.insert(page_id, Object::Dictionary(page));
        }
    }
    objects.insert(
        field_id,
        Object::Dictionary(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Sig",
            "T" => Object::string_literal(field_name),
            "V" => sig_id,
            "F" => WIDGET_FLAGS,
            "Rect" => rect.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
            "P" => page_id,
            "AP" => dictionary! { "N" => form_id },
        }),
    );

    // The signature dictionary, with /ByteRange and /Contents last so their
    // places are known once it is written.
    let mut sig = dictionary! {
        "Type" => "Sig",
        "Filter" => "Adobe.PPKLite",
        "SubFilter" => "ETSI.CAdES.detached",
        "M" => Object::string_literal(pdf_date(now)),
    };
    for (key, value) in [
        ("Name", &options.signer_name),
        ("Reason", &options.reason),
        ("Location", &options.location),
        ("ContactInfo", &options.contact_info),
    ] {
        if let Some(value) = value {
            sig.set(key, ::lopdf::text_string(value));
        }
    }
    let mut head = Vec::new();
    Writer::write_object(&mut head, &Object::Dictionary(sig)).map_err(writing)?;
    let head = head
        .strip_suffix(b">>")
        .ok_or_else(|| writing("a dictionary that does not end with >>"))?;

    let mut out = Vec::with_capacity(original.len() + 2 * reserve + 16 * 1024);
    out.extend_from_slice(original);
    if !matches!(original.last(), Some(b'\n' | b'\r')) {
        out.push(b'\n');
    }
    let mut offsets: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
    offsets.insert(sig_id.0, (out.len(), 0));
    writeln!(out, "{} 0 obj", sig_id.0).map_err(writing)?;
    out.extend_from_slice(head);
    out.extend_from_slice(b"/ByteRange [");
    let byte_range_at = out.len();
    out.extend(std::iter::repeat_n(b' ', BYTE_RANGE_WIDTH));
    out.extend_from_slice(b"]/Contents <");
    let contents_start = out.len();
    out.extend(std::iter::repeat_n(b'0', 2 * reserve));
    let contents_end = out.len();
    out.extend_from_slice(b">>>\nendobj\n");
    for (id, object) in &objects {
        offsets.insert(id.0, (out.len(), id.1));
        write_object(&mut out, *id, object)?;
    }

    // The trailer keeps the catalog, the Info dictionary and the file
    // identifier, and points back at the section before it.
    let id_array = match pdf.trailer.get(b"ID") {
        Ok(id) => id.clone(),
        Err(_) => {
            let first = Sha256::digest(original)[..16].to_vec();
            Object::Array(vec![
                Object::String(first.clone(), ::lopdf::StringFormat::Hexadecimal),
                Object::String(first, ::lopdf::StringFormat::Hexadecimal),
            ])
        }
    };
    let mut trailer = dictionary! { "Root" => root_id, "ID" => id_array, "Prev" => prev as i64 };
    if let Ok(info) = pdf.trailer.get(b"Info") {
        trailer.set("Info", info.clone());
    }

    let xref_at = out.len();
    if xref_stream {
        let xref_id = alloc();
        offsets.insert(xref_id.0, (xref_at, 0));
        let ids: Vec<u32> = offsets.keys().copied().collect();
        let mut data = Vec::with_capacity(ids.len() * 7);
        for (offset, generation) in offsets.values() {
            let offset = u32::try_from(*offset).map_err(|_| writing("an offset past 4 GiB"))?;
            data.push(1);
            data.extend_from_slice(&offset.to_be_bytes());
            data.extend_from_slice(&generation.to_be_bytes());
        }
        let index: Vec<Object> = runs(&ids)
            .into_iter()
            .flat_map(|(first, count)| [Object::from(first as i64), Object::from(count as i64)])
            .collect();
        trailer.set("Type", "XRef");
        trailer.set("Size", next as i64);
        trailer.set("Index", index);
        trailer.set("W", vec![1.into(), 4.into(), 2.into()]);
        write_object(
            &mut out,
            xref_id,
            &Object::Stream(Stream::new(trailer, data)),
        )?;
    } else {
        trailer.set("Size", next as i64);
        out.extend_from_slice(b"xref\n");
        let ids: Vec<u32> = offsets.keys().copied().collect();
        for (first, count) in runs(&ids) {
            writeln!(out, "{first} {count}").map_err(writing)?;
            for id in first..first + count {
                let (offset, generation) = offsets[&id];
                write!(out, "{offset:010} {generation:05} n\r\n").map_err(writing)?;
            }
        }
        out.extend_from_slice(b"trailer\n");
        Writer::write_object(&mut out, &Object::Dictionary(trailer)).map_err(writing)?;
        out.push(b'\n');
    }
    write!(out, "startxref\n{xref_at}\n%%EOF\n").map_err(writing)?;

    // Everything is in place: fill in the byte range, padded to its width.
    let after = contents_end + 1;
    let range = format!("0 {} {} {}", contents_start - 1, after, out.len() - after);
    if range.len() > BYTE_RANGE_WIDTH {
        return Err(writing("a byte range wider than its placeholder"));
    }
    out[byte_range_at..byte_range_at + range.len()].copy_from_slice(range.as_bytes());
    Ok(Update {
        bytes: out,
        contents: contents_start..contents_end,
    })
}
