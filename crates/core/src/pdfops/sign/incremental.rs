//! The incremental updates a signature is made of. The original bytes are
//! kept as they are and each update is appended after them, with a
//! cross-reference section of the same kind as the original's last one.
//!
//! A signature update holds the signature field, its widget on a page, the
//! AcroForm that lists it, and the `/Sig` (or `/DocTimeStamp`) dictionary with
//! its `/ByteRange` and `/Contents` placeholder. A certification signature
//! also gets `/Reference` with DocMDP transform parameters, and the catalog's
//! `/Perms`. The DSS update of B-LT uses the same [`Appender`] with no
//! signature in it.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::ops::Range;
use std::time::SystemTime;

use ::lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, Writer, dictionary};
use sha2::{Digest, Sha256};

use super::SignOptions;
use crate::error::{Error, Result};
use crate::pdfops::draw::{num, width as helvetica_width, winansi};

/// Room for `0 a b c` with each offset up to ten digits.
const BYTE_RANGE_WIDTH: usize = 1 + 3 * 11;

/// `/F 132`: Print and Locked, as other signers set on signature widgets.
const WIDGET_FLAGS: i64 = 132;

/// The width of a text appearance when the request gives none, in points.
const TEXT_WIDTH: f32 = 200.0;
const TEXT_SIZE: f32 = 9.0;
const TEXT_LEADING: f32 = 11.0;
const TEXT_PADDING: f32 = 4.0;

/// The document with an update appended. For a signature update, before the
/// signature is in it.
pub(super) struct Update {
    pub bytes: Vec<u8>,
    /// The hex digits of `/Contents`, between its `<` and `>`. Empty for an
    /// update with no signature.
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

    /// The SHA-256 of [`Update::signed_ranges`].
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for range in self.signed_ranges() {
            hasher.update(range);
        }
        hasher.finalize().into()
    }

    /// Writes `der` into the placeholder, as hex, padded with the zeros
    /// already there. Fails when it does not fit.
    pub fn fill(&mut self, der: &[u8]) -> Result<()> {
        let hex = hex::encode_upper(der);
        if hex.len() > self.contents.len() {
            return Err(Error::Engine {
                engine: "signer",
                message: format!(
                    "the signature is {} hex digits, more than the {} reserved for it",
                    hex.len(),
                    self.contents.len()
                ),
            });
        }
        let start = self.contents.start;
        self.bytes[start..start + hex.len()].copy_from_slice(hex.as_bytes());
        Ok(())
    }
}

/// What a signature update carries.
pub(super) enum Kind<'a> {
    /// A signature made with a key: approval, or certification when
    /// `options.certify` is set.
    Signature {
        options: &'a SignOptions,
        now: SystemTime,
        /// Who signed, as a text appearance shows it.
        shown_name: &'a str,
    },
    /// An RFC 3161 document timestamp (`/Type /DocTimeStamp`).
    DocTimeStamp,
}

pub(super) fn invalid(name: &str, what: impl std::fmt::Display) -> Error {
    Error::InvalidRequest(format!("'{name}' cannot be signed: {what}"))
}

pub(super) fn writing(e: impl std::fmt::Display) -> Error {
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
pub(super) fn array_in(
    pdf: &Pdf,
    dict: &Dictionary,
    key: &[u8],
) -> (Vec<Object>, Option<ObjectId>) {
    match dict.get(key) {
        Ok(Object::Reference(id)) => match pdf.get_object(*id).and_then(Object::as_array) {
            Ok(array) => (array.clone(), Some(*id)),
            Err(_) => (Vec::new(), None),
        },
        Ok(Object::Array(array)) => (array.clone(), None),
        _ => (Vec::new(), None),
    }
}

/// A dictionary in `dict` under `key`, followed through one reference.
pub(super) fn dict_in(pdf: &Pdf, dict: &Dictionary, key: &[u8]) -> Option<Dictionary> {
    match dict.get(key) {
        Ok(Object::Reference(id)) => pdf.get_dictionary(*id).ok().cloned(),
        Ok(Object::Dictionary(d)) => Some(d.clone()),
        _ => None,
    }
}

/// `Signature1`, or the first `SignatureN` no top-level field uses.
fn field_name(pdf: &Pdf, fields: &[Object]) -> String {
    // A set, so a form that takes every name up to some large N costs N
    // lookups and not N of them for each.
    let taken: std::collections::HashSet<&[u8]> = fields
        .iter()
        .filter_map(|f| match f {
            Object::Reference(id) => pdf.get_dictionary(*id).ok(),
            Object::Dictionary(d) => Some(d),
            _ => None,
        })
        .filter_map(|d| d.get(b"T").and_then(Object::as_str).ok())
        .collect();
    (1..)
        .map(|n| format!("Signature{n}"))
        .find(|name| !taken.contains(name.as_bytes()))
        .expect("a free name")
}

/// Most field nodes [`has_signature`] looks at before it gives up.
pub(super) const MAX_FIELD_NODES: usize = 10_000;

/// Whether a field in the tree is a signature field with a value, or `None`
/// when the tree has more than [`MAX_FIELD_NODES`] nodes to look at. The
/// walk is iterative and visits each object once, so neither deep nesting
/// nor `/Kids` arrays that share children (a tree that is really a graph
/// with exponentially many paths) costs more than the budget.
pub(super) fn has_signature(pdf: &Pdf, fields: &[Object]) -> Option<bool> {
    let mut visited: std::collections::HashSet<ObjectId> = std::collections::HashSet::new();
    let mut stack: Vec<Object> = fields.iter().rev().cloned().collect();
    let mut budget = MAX_FIELD_NODES;
    while let Some(node) = stack.pop() {
        if budget == 0 {
            return None;
        }
        budget -= 1;
        let field = match &node {
            Object::Reference(id) => {
                if !visited.insert(*id) {
                    continue;
                }
                match pdf.get_dictionary(*id) {
                    Ok(d) => d,
                    Err(_) => continue,
                }
            }
            Object::Dictionary(d) => d,
            _ => continue,
        };
        let is_sig = field
            .get(b"FT")
            .and_then(Object::as_name)
            .is_ok_and(|n| n == b"Sig");
        if is_sig && field.get(b"V").is_ok() {
            return Some(true);
        }
        let (kids, _) = array_in(pdf, field, b"Kids");
        // Only children not seen yet go on the stack, so a Kids array that
        // names one child many times spends the budget once per entry at
        // most, and never re-walks what is below it.
        for kid in kids.into_iter().rev() {
            match &kid {
                Object::Reference(id) if visited.contains(id) => {}
                _ => stack.push(kid),
            }
        }
    }
    Some(false)
}

/// The DocMDP permission of an existing certification signature, if any.
fn certified(pdf: &Pdf, catalog: &Dictionary) -> Option<i64> {
    let perms = dict_in(pdf, catalog, b"Perms")?;
    let sig = dict_in(pdf, &perms, b"DocMDP")?;
    let (references, _) = array_in(pdf, &sig, b"Reference");
    let mut permission = None;
    for reference in references {
        let reference = match reference {
            Object::Reference(id) => pdf.get_dictionary(id).ok().cloned(),
            Object::Dictionary(d) => Some(d),
            _ => None,
        };
        let Some(reference) = reference else { continue };
        if reference
            .get(b"TransformMethod")
            .and_then(Object::as_name)
            .is_ok_and(|n| n == b"DocMDP")
        {
            permission = Some(
                dict_in(pdf, &reference, b"TransformParams")
                    .and_then(|p| p.get(b"P").and_then(Object::as_i64).ok())
                    .unwrap_or(2),
            );
        }
    }
    // A /Perms /DocMDP entry with no readable reference still certifies.
    Some(permission.unwrap_or(2))
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

/// One incremental update being put together: the objects it rewrites or
/// adds, by id, so nothing is written twice.
pub(super) struct Appender<'a> {
    original: &'a [u8],
    pub pdf: &'a Pdf,
    prev: usize,
    xref_stream: bool,
    pub root_id: ObjectId,
    next: u32,
    objects: BTreeMap<ObjectId, Object>,
}

impl<'a> Appender<'a> {
    pub fn new(original: &'a [u8], pdf: &'a Pdf, name: &str) -> Result<Self> {
        let prev = last_startxref(original)
            .filter(|&at| at < original.len())
            .ok_or_else(|| invalid(name, "it has no cross-reference section to append to"))?;
        let xref_stream = !original[prev..].trim_ascii_start().starts_with(b"xref");
        let root_id = pdf
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .map_err(|_| invalid(name, "its trailer names no catalog"))?;
        pdf.get_dictionary(root_id)
            .map_err(|_| invalid(name, "its catalog is not a dictionary"))?;
        let size = pdf
            .trailer
            .get(b"Size")
            .and_then(Object::as_i64)
            .unwrap_or(0)
            .max(0) as u32;
        Ok(Self {
            original,
            pdf,
            prev,
            xref_stream,
            root_id,
            next: size.max(pdf.max_id + 1),
            objects: BTreeMap::new(),
        })
    }

    pub fn alloc(&mut self) -> ObjectId {
        let id = (self.next, 0);
        self.next += 1;
        id
    }

    /// Adds or replaces an object.
    pub fn put(&mut self, id: ObjectId, object: Object) {
        self.objects.insert(id, object);
    }

    /// A new object.
    pub fn add(&mut self, object: Object) -> ObjectId {
        let id = self.alloc();
        self.put(id, object);
        id
    }

    /// The catalog as this update leaves it so far.
    pub fn catalog(&self) -> Dictionary {
        match self.objects.get(&self.root_id) {
            Some(Object::Dictionary(d)) => d.clone(),
            _ => self
                .pdf
                .get_dictionary(self.root_id)
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// Writes the update after the original. With `sig`, the signature
    /// dictionary `head` (a dictionary without its closing `>>`) comes first,
    /// followed by `/ByteRange` and a `/Contents` placeholder of `reserve`
    /// bytes, and the byte range is filled in at the end.
    pub fn finish(self, sig: Option<(ObjectId, &[u8], usize)>) -> Result<Update> {
        let original = self.original;
        let reserve = sig.map_or(0, |(_, _, r)| r);
        let mut out = Vec::with_capacity(original.len() + 2 * reserve + 16 * 1024);
        out.extend_from_slice(original);
        if !matches!(original.last(), Some(b'\n' | b'\r')) {
            out.push(b'\n');
        }
        let mut offsets: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
        let mut placeholder = None;
        if let Some((sig_id, head, reserve)) = sig {
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
            placeholder = Some((byte_range_at, contents_start, contents_end));
        }
        for (id, object) in &self.objects {
            offsets.insert(id.0, (out.len(), id.1));
            write_object(&mut out, *id, object)?;
        }

        // The trailer keeps the catalog, the Info dictionary and the file
        // identifier, and points back at the section before it.
        let pdf = self.pdf;
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
        let mut trailer =
            dictionary! { "Root" => self.root_id, "ID" => id_array, "Prev" => self.prev as i64 };
        if let Ok(info) = pdf.trailer.get(b"Info") {
            trailer.set("Info", info.clone());
        }

        let mut next = self.next;
        let xref_at = out.len();
        if self.xref_stream {
            let xref_id = (next, 0);
            next += 1;
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

        let Some((byte_range_at, contents_start, contents_end)) = placeholder else {
            return Ok(Update {
                bytes: out,
                contents: 0..0,
            });
        };
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
}

/// `text` in WinAnsiEncoding, cut with `...` to fit `width` points at `size`.
fn fit(text: &str, width: f32, size: f32) -> Vec<u8> {
    let measure = |s: &str| s.chars().map(helvetica_width).sum::<f32>() * size / 1000.0;
    let mut shown: String = text.chars().filter(|c| !c.is_control()).collect();
    if measure(&shown) > width {
        while !shown.is_empty() && measure(&format!("{shown}...")) > width {
            shown.pop();
        }
        shown.push_str("...");
    }
    shown.chars().map(winansi).collect()
}

/// A form XObject showing who signed, when, and why: the appearance of a
/// visible signature that has no image. Returns it with its height.
fn text_appearance(lines: &[String], width: f32) -> (Stream, f32) {
    let height = lines.len() as f32 * TEXT_LEADING + 2.0 * TEXT_PADDING;
    let mut content = format!(
        "q 0.5 w 0.35 G 0.25 0.25 {} {} re S Q\nBT /F1 {} Tf 0.1 g {} TL {} {} Td\n",
        num(width - 0.5),
        num(height - 0.5),
        num(TEXT_SIZE),
        num(TEXT_LEADING),
        num(TEXT_PADDING),
        num(height - TEXT_PADDING - TEXT_SIZE),
    )
    .into_bytes();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            content.extend_from_slice(b"T* ");
        }
        content.push(b'<');
        content.extend_from_slice(
            hex::encode_upper(fit(line, width - 2.0 * TEXT_PADDING, TEXT_SIZE)).as_bytes(),
        );
        content.extend_from_slice(b"> Tj\n");
    }
    content.extend_from_slice(b"ET\n");
    let stream = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), width.into(), height.into()],
            "Resources" => dictionary! {
                "Font" => dictionary! {
                    "F1" => dictionary! {
                        "Type" => "Font", "Subtype" => "Type1",
                        "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
                    },
                },
            },
        },
        content,
    );
    (stream, height)
}

/// The lines of a text appearance.
fn text_lines(options: &SignOptions, now: SystemTime, shown_name: &str) -> Vec<String> {
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let date = chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();
    let mut lines = vec![
        format!("Digitally signed by {shown_name}"),
        format!("Date: {date}"),
    ];
    if let Some(reason) = &options.reason {
        lines.push(format!("Reason: {reason}"));
    }
    if let Some(location) = &options.location {
        lines.push(format!("Location: {location}"));
    }
    lines
}

/// Appends a signature update to `original`, with `reserve` bytes for the DER
/// in `/Contents`. `pdf` is `original` parsed.
pub(super) fn prepare(
    original: &[u8],
    pdf: &Pdf,
    name: &str,
    kind: Kind<'_>,
    reserve: usize,
) -> Result<Update> {
    let mut up = Appender::new(original, pdf, name)?;
    let root_id = up.root_id;
    let mut catalog = up.catalog();
    let catalog_before = catalog.clone();
    let sig_id = up.alloc();
    let field_id = up.alloc();

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

    let options = match &kind {
        Kind::Signature { options, .. } => Some(*options),
        Kind::DocTimeStamp => None,
    };
    if let Some(options) = options {
        if certified(pdf, &catalog) == Some(1) {
            return Err(invalid(
                name,
                "it is certified with no changes allowed (DocMDP permission 1)",
            ));
        }
        if options.certify.is_some() {
            match has_signature(pdf, &fields) {
                Some(false) => {}
                Some(true) => {
                    return Err(invalid(
                        name,
                        "it is signed already, and a certification signature must be the first",
                    ));
                }
                // Fail closed: what cannot be checked is not certified.
                None => {
                    return Err(invalid(
                        name,
                        format!(
                            "its form has more than {MAX_FIELD_NODES} fields to check for signatures before certifying"
                        ),
                    ));
                }
            }
        }
    }

    // The page the widget sits on: the appearance's, or the first for an
    // invisible signature, which still needs a widget in some page's /Annots.
    let pages = pdf.get_pages();
    let page_number = options
        .and_then(|o| o.appearance.as_ref())
        .map_or(1, |a| a.page);
    let page_id = *pages.get(&page_number).ok_or_else(|| {
        Error::InvalidRequest(format!(
            "'{name}' has {} pages; there is no page {page_number} to sign on",
            pages.len()
        ))
    })?;

    let appearance = match &kind {
        Kind::Signature {
            options,
            now,
            shown_name,
        } => options
            .appearance
            .as_ref()
            .map(|a| (a, *options, *now, *shown_name)),
        Kind::DocTimeStamp => None,
    };
    let (rect, form) = match appearance {
        Some((appearance, options, now, shown_name)) => {
            let (w, h, form) = match &appearance.image {
                Some(bytes) => {
                    let mut image = crate::pdfops::draw::image_xobject(bytes, &appearance.name)?;
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
                    let image_id = up.add(Object::Stream(image));
                    let content = format!("q {} 0 0 {} 0 0 cm /Img Do Q", num(w), num(h));
                    let form = Stream::new(
                        dictionary! {
                            "Type" => "XObject", "Subtype" => "Form",
                            "BBox" => vec![0.into(), 0.into(), w.into(), h.into()],
                            "Resources" => dictionary! { "XObject" => dictionary! { "Img" => image_id } },
                        },
                        content.into_bytes(),
                    );
                    (w, h, form)
                }
                None => {
                    let w = appearance.width.unwrap_or(TEXT_WIDTH);
                    let (form, h) = text_appearance(&text_lines(options, now, shown_name), w);
                    (w, h, form)
                }
            };
            let (x, y) = (appearance.x, appearance.y);
            ([x, y, x + w, y + h], Some(form))
        }
        // A document timestamp has no appearance at all (ISO 32000-2
        // 12.8.5), which is also how validators tell it is maintenance.
        None if matches!(kind, Kind::DocTimeStamp) => ([0.0; 4], None),
        // An empty appearance: PDF/A and some validators want one even on an
        // invisible widget.
        None => (
            [0.0; 4],
            Some(Stream::new(
                dictionary! {
                    "Type" => "XObject", "Subtype" => "Form",
                    "BBox" => vec![0.into(), 0.into(), 0.into(), 0.into()],
                },
                Vec::new(),
            )),
        ),
    };
    let form_id = form.map(|form| up.add(Object::Stream(form)));

    let field_name = field_name(pdf, &fields);
    fields.push(field_id.into());
    match fields_id {
        Some(id) => up.put(id, Object::Array(fields)),
        None => acroform.set("Fields", fields),
    }
    // SignaturesExist and AppendOnly: later changes must be incremental too.
    acroform.set("SigFlags", 3);
    match acroform_id {
        Some(id) => up.put(id, Object::Dictionary(acroform)),
        None => {
            let id = up.add(Object::Dictionary(acroform));
            catalog.set("AcroForm", id);
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
        Some(id) => up.put(id, Object::Array(annots)),
        None => {
            page.set("Annots", annots);
            up.put(page_id, Object::Dictionary(page));
        }
    }
    let mut widget = dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Sig",
        "T" => Object::string_literal(field_name),
        "V" => sig_id,
        "F" => WIDGET_FLAGS,
        "Rect" => rect.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
        "P" => page_id,
    };
    if let Some(form_id) = form_id {
        widget.set("AP", dictionary! { "N" => form_id });
    }
    up.put(field_id, Object::Dictionary(widget));

    // The signature dictionary, with /ByteRange and /Contents last so their
    // places are known once it is written.
    let sig = match &kind {
        Kind::Signature { options, now, .. } => {
            let mut sig = dictionary! {
                "Type" => "Sig",
                "Filter" => "Adobe.PPKLite",
                "SubFilter" => "ETSI.CAdES.detached",
                "M" => Object::string_literal(pdf_date(*now)),
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
            if let Some(permission) = options.certify {
                // ISO 32000-2 12.8.2.2: the signature reference names the
                // DocMDP transform, and the catalog's /Perms names the
                // signature.
                sig.set(
                    "Reference",
                    vec![Object::Dictionary(dictionary! {
                        "Type" => "SigRef",
                        "TransformMethod" => "DocMDP",
                        "TransformParams" => dictionary! {
                            "Type" => "TransformParams",
                            "P" => i64::from(permission.level()),
                            "V" => "1.2",
                        },
                    })],
                );
                let mut perms = dict_in(pdf, &catalog, b"Perms").unwrap_or_default();
                perms.set("DocMDP", sig_id);
                catalog.set("Perms", perms);
            }
            sig
        }
        Kind::DocTimeStamp => dictionary! {
            "Type" => "DocTimeStamp",
            "Filter" => "Adobe.PPKLite",
            "SubFilter" => "ETSI.RFC3161",
        },
    };
    if catalog != catalog_before {
        up.put(root_id, Object::Dictionary(catalog));
    }
    let mut head = Vec::new();
    Writer::write_object(&mut head, &Object::Dictionary(sig)).map_err(writing)?;
    let head = head
        .strip_suffix(b">>")
        .ok_or_else(|| writing("a dictionary that does not end with >>"))?
        .to_vec();
    up.finish(Some((sig_id, &head, reserve)))
}
