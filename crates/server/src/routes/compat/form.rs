//! Reading a Gotenberg form: errors accumulate instead of stopping at the
//! first, every message is Gotenberg's own, and a field's first value counts.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusticdoc_core::Document;
use rusticdoc_core::compat::{
    Bookmarks, Facturx, PdfFormats, PostSteps, Split, alphanumeric_less, facturx_file_name,
};
use rusticdoc_core::config::parse_go_bool;
use rusticdoc_core::pdfops::{
    Attachment, EmbedMetadata, EncryptOptions, FacturxProperties, Metadata, Overlay, OverlayKind,
    Permissions, Position, Rotation, require_pdf,
};

use crate::error::{ApiError, ApiResult};
use crate::routes::form::Form;

/// Most watermarks or stamps one request may apply, each a pass over the
/// document.
const MAX_OVERLAYS: usize = 5;
/// JPEG quality when `imageQuality` is absent.
const DEFAULT_IMAGE_QUALITY: u8 = 80;

/// A form being read. Each accessor records what is wrong with its field and
/// returns a default, so one request learns about every problem at once.
#[derive(Debug)]
pub struct FormData {
    form: Form,
    errors: Vec<String>,
    /// The name each file was sent under, before duplicates were renamed.
    received: HashMap<String, String>,
}

impl FormData {
    pub fn new(mut form: Form) -> Self {
        let received = unique_names(&mut form.files);
        Self {
            form,
            errors: Vec::new(),
            received,
        }
    }

    /// `Invalid form data: ...` with everything recorded so far, one per line.
    pub fn validate(&self) -> ApiResult<()> {
        if self.errors.is_empty() {
            return Ok(());
        }
        Err(ApiError::bad_request(format!(
            "Invalid form data: {}",
            self.errors.join("\n")
        )))
    }

    fn value(&self, key: &str) -> &str {
        self.form
            .multi
            .get(key)
            .and_then(|values| values.first())
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn string(&self, key: &str) -> String {
        self.value(key).to_string()
    }

    pub fn optional(&self, key: &str) -> Option<String> {
        Some(self.string(key)).filter(|v| !v.is_empty())
    }

    /// Every value of a repeated field.
    pub fn values(&self, key: &str) -> Vec<String> {
        self.form.all(key)
    }

    fn invalid(&mut self, key: &str, value: &str, why: &str) {
        self.errors.push(format!(
            "form field '{key}' is invalid (got '{value}', resulting to {why})"
        ));
    }

    /// A field in Go's boolean syntax.
    pub fn boolean(&mut self, key: &str, default: bool) -> bool {
        let value = self.string(key);
        if value.is_empty() {
            return default;
        }
        match parse_go_bool(&value) {
            Some(parsed) => parsed,
            None => {
                let why = format!("strconv.ParseBool: parsing {value:?}: invalid syntax");
                self.invalid(key, &value, &why);
                default
            }
        }
    }

    /// Runs `assign` on the field's value, even an empty one, and records the
    /// reason it gives for refusing it.
    pub fn custom<T>(
        &mut self,
        key: &str,
        assign: impl FnOnce(&str) -> Result<T, String>,
    ) -> Option<T> {
        let value = self.string(key);
        match assign(&value) {
            Ok(parsed) => Some(parsed),
            Err(why) => {
                self.invalid(key, &value, &why);
                None
            }
        }
    }

    /// As [`FormData::custom`], for a field that must be present.
    pub fn mandatory_custom<T>(
        &mut self,
        key: &str,
        assign: impl FnOnce(&str) -> Result<T, String>,
    ) -> Option<T> {
        if self.value(key).is_empty() {
            self.errors.push(format!("form field '{key}' is required"));
            return None;
        }
        self.custom(key, assign)
    }

    /// The input files with one of `extensions`, in Gotenberg's order: by the
    /// numbers in their names, then as uploaded. Files under `embeds`,
    /// `watermark`, `stamp` and `facturxXml` are not inputs.
    pub fn paths(&self, extensions: &[&str]) -> Vec<Document> {
        let mut inputs: Vec<Document> = self
            .form
            .files
            .iter()
            .filter(|doc| extensions.contains(&lower_extension(&doc.name).as_str()))
            .cloned()
            .collect();
        let received = |doc: &Document| -> String {
            self.received
                .get(&doc.name)
                .cloned()
                .unwrap_or_else(|| doc.name.clone())
        };
        inputs.sort_by(|a, b| {
            let (a, b) = (received(a), received(b));
            if a == b {
                std::cmp::Ordering::Equal
            } else if alphanumeric_less(&a, &b) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        });
        inputs
    }

    /// As [`FormData::paths`], with at least one file.
    pub fn mandatory_paths(&mut self, extensions: &[&str]) -> Vec<Document> {
        let inputs = self.paths(extensions);
        if inputs.is_empty() {
            self.errors.push(format!(
                "no form file found for extensions: [{}]",
                extensions.join(" ")
            ));
        }
        inputs
    }

    /// File parts under a field name.
    pub fn take_files(&mut self, field: &str) -> Vec<Document> {
        self.form.take_field(field)
    }

    /// `true` if a file was sent under `field`.
    pub fn has_files(&self, field: &str) -> bool {
        self.form.by_field.contains_key(field)
    }

    /// The fonts sent for this one conversion.
    pub fn fonts(&self) -> Vec<(String, Vec<u8>)> {
        self.form
            .by_field
            .get("fonts")
            .map(|fonts| {
                fonts
                    .iter()
                    .map(|f| (f.name.clone(), f.bytes.to_vec()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A second file of the same name becomes `name (2).ext`, as uploads of the
/// same file name would otherwise overwrite each other in a ZIP.
fn unique_names(files: &mut [Document]) -> HashMap<String, String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut received = HashMap::new();
    for doc in files {
        if seen.insert(doc.name.clone()) {
            continue;
        }
        let (stem, ext) = split_extension(&doc.name);
        let mut i = 2;
        let unique = loop {
            let candidate = format!("{stem} ({i}){ext}");
            if seen.insert(candidate.clone()) {
                break candidate;
            }
            i += 1;
        };
        received.insert(unique.clone(), std::mem::replace(&mut doc.name, unique));
    }
    received
}

fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(at) => name.split_at(at),
        None => (name, ""),
    }
}

/// `.PDF` as `.pdf`, including the dot; empty without one.
fn lower_extension(name: &str) -> String {
    split_extension(name).1.to_ascii_lowercase()
}

/// `strconv.Atoi`'s failure for a field that should hold an integer.
fn atoi(value: &str) -> Result<i64, String> {
    value.parse::<i64>().map_err(|e| {
        let why = match e.kind() {
            std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
                "value out of range"
            }
            _ => "invalid syntax",
        };
        format!("strconv.Atoi: parsing {value:?}: {why}")
    })
}

/// The first syntax problem in `raw`, in the words of Go's JSON decoder, which
/// clients may match on.
pub fn go_json_error(raw: &str) -> String {
    let trimmed = raw.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return "unexpected end of JSON input".to_string();
    };
    let printable = |c: char| match c {
        '\'' => "'\\''".to_string(),
        c => format!("'{c}'"),
    };
    for (start, literal) in [('t', "true"), ('f', "false"), ('n', "null")] {
        if first == start {
            for (i, (got, want)) in trimmed.chars().zip(literal.chars()).enumerate() {
                if got != want {
                    return format!(
                        "invalid character {} in literal {literal} (expecting {})",
                        printable(got),
                        printable(literal.chars().nth(i).unwrap_or(want))
                    );
                }
            }
            if trimmed.len() < literal.len() {
                return "unexpected end of JSON input".to_string();
            }
            return format!(
                "invalid character {} after top-level value",
                printable(trimmed[literal.len()..].chars().next().unwrap_or(' '))
            );
        }
    }
    if !(first == '{'
        || first == '['
        || first == '"'
        || first == '-'
        || first.is_ascii_digit())
    {
        return format!(
            "invalid character {} looking for beginning of value",
            printable(first)
        );
    }
    match serde_json::from_str::<serde_json::Value>(raw) {
        Err(e) if e.is_eof() => "unexpected end of JSON input".to_string(),
        Err(e) => format!("invalid JSON at line {} column {}", e.line(), e.column()),
        Ok(serde_json::Value::Object(_)) => "json: wrong object shape".to_string(),
        Ok(serde_json::Value::Array(_)) => {
            "json: cannot unmarshal array into Go value of type map[string]interface {}".to_string()
        }
        Ok(serde_json::Value::String(_)) => {
            "json: cannot unmarshal string into Go value of type map[string]interface {}"
                .to_string()
        }
        Ok(serde_json::Value::Number(_)) => {
            "json: cannot unmarshal number into Go value of type map[string]interface {}"
                .to_string()
        }
        Ok(_) => "json: cannot unmarshal value into Go value".to_string(),
    }
}

fn metadata_of(value: &str) -> Result<Option<Metadata>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::Object(map)) => Ok(Some(map.into_iter().collect())),
        Ok(serde_json::Value::Null) => Ok(None),
        _ => Err(format!("unmarshal metadata: {}", go_json_error(value))),
    }
}

/// `metadata`, a JSON object.
pub fn read_metadata(form: &mut FormData, mandatory: bool) -> Option<Metadata> {
    if mandatory {
        form.mandatory_custom("metadata", metadata_of).flatten()
    } else {
        form.custom("metadata", metadata_of).flatten()
    }
}

fn bookmarks_of(value: &str) -> Result<Bookmarks, String> {
    if value.is_empty() {
        return Ok(Bookmarks::None);
    }
    if let Ok(list) = serde_json::from_str(value) {
        return Ok(Bookmarks::List(list));
    }
    if let Ok(map) = serde_json::from_str(value) {
        return Ok(Bookmarks::Map(map));
    }
    Err(format!("unmarshal bookmarks: {}", go_json_error(value)))
}

/// `bookmarks`: a list for every file, or a map keyed by file name.
pub fn read_bookmarks(form: &mut FormData, mandatory: bool) -> Bookmarks {
    let found = if mandatory {
        form.mandatory_custom("bookmarks", bookmarks_of)
    } else {
        form.custom("bookmarks", bookmarks_of)
    };
    found.unwrap_or_default()
}

/// `splitMode`, `splitSpan` and `splitUnify`; `None` when none is set.
pub fn read_split(form: &mut FormData, mandatory: bool) -> Option<Split> {
    let check_mode = |value: &str| -> Result<String, String> {
        if !value.is_empty() && value != "intervals" && value != "pages" {
            return Err("wrong value, expected either 'intervals' or 'pages'".into());
        }
        Ok(value.to_string())
    };
    let mode = if mandatory {
        form.mandatory_custom("splitMode", check_mode)
    } else {
        form.custom("splitMode", check_mode)
    }
    .unwrap_or_default();
    let mode_for_span = mode.clone();
    let check_span = move |value: &str| -> Result<String, String> {
        let value: String = value.split_whitespace().collect();
        if mode_for_span == "intervals" {
            let n = atoi(&value)?;
            if n < 1 {
                return Err("value is inferior to 1".into());
            }
        }
        Ok(value)
    };
    let span = if mandatory {
        form.mandatory_custom("splitSpan", check_span)
    } else {
        form.custom("splitSpan", check_span)
    }
    .unwrap_or_default();
    let unify = form.boolean("splitUnify", false);
    if !form.value("splitUnify").is_empty() && unify && mode != "pages" {
        let why = format!("unify is not available for split mode '{mode}'");
        let value = form.string("splitUnify");
        form.invalid("splitUnify", &value, &why);
    }
    if mode.is_empty() && span.is_empty() && !unify {
        return None;
    }
    Some(Split { mode, span, unify })
}

/// `pdfa` and `pdfua`.
pub fn read_formats(form: &mut FormData) -> PdfFormats {
    PdfFormats {
        pdfa: form.string("pdfa"),
        pdfua: form.boolean("pdfua", false),
    }
}

/// `userPassword`, `ownerPassword` and the permissions.
pub fn read_encrypt(form: &mut FormData) -> EncryptOptions {
    EncryptOptions {
        user_password: form.string("userPassword"),
        owner_password: form.string("ownerPassword"),
        permissions: Permissions {
            printing: form.boolean("allowPrinting", true),
            copying: form.boolean("allowCopying", true),
            modifying: form.boolean("allowModifying", true),
            annotating: form.boolean("allowAnnotating", true),
            filling_forms: form.boolean("allowFillingForms", true),
            assembling: form.boolean("allowAssembling", true),
        },
    }
}

/// The `embeds` files.
pub fn read_embeds(form: &mut FormData) -> Vec<Attachment> {
    form.take_files("embeds")
        .into_iter()
        .map(|doc| Attachment {
            mime: doc.content_type().to_string(),
            name: doc.name,
            bytes: doc.bytes.to_vec(),
        })
        .collect()
}

/// `embedsMetadata`, keyed by file name.
pub fn read_embeds_metadata(form: &mut FormData) -> BTreeMap<String, EmbedMetadata> {
    let raw = form.string("embedsMetadata");
    if raw.is_empty() {
        return BTreeMap::new();
    }
    match serde_json::from_str::<BTreeMap<String, BTreeMap<String, String>>>(&raw) {
        Ok(parsed) => parsed
            .into_iter()
            .map(|(name, fields)| {
                (
                    name,
                    EmbedMetadata {
                        mime_type: fields.get("mimeType").cloned(),
                        relationship: fields.get("relationship").cloned(),
                    },
                )
            })
            .collect(),
        Err(_) => {
            form.errors.push(format!(
                "form field 'embedsMetadata' is invalid: {}",
                go_json_error(&raw)
            ));
            BTreeMap::new()
        }
    }
}

/// The Factur-X fields as sent; both must be present to mean anything.
pub struct RawFacturx {
    pub level: String,
    pub document_type: String,
    pub version: String,
    pub xml: Option<Document>,
}

pub fn read_facturx(form: &mut FormData) -> RawFacturx {
    let level = form
        .custom("facturxConformanceLevel", |value| match value {
            "" | "MINIMUM" | "BASIC WL" | "BASIC" | "EN 16931" | "EXTENDED" | "XRECHNUNG" => {
                Ok(value.to_string())
            }
            other => Err(format!("unsupported conformance level '{other}'")),
        })
        .unwrap_or_default();
    let document_type = form
        .custom("facturxDocumentType", |value| match value {
            "" => Ok("INVOICE".to_string()),
            "INVOICE" | "ORDER" | "ORDER_RESPONSE" | "ORDER_CHANGE" => Ok(value.to_string()),
            other => Err(format!("unsupported document type '{other}'")),
        })
        .unwrap_or_default();
    let version = form
        .optional("facturxVersion")
        .unwrap_or_else(|| "1.0".to_string());
    let xml = form.take_files("facturxXml").into_iter().next();
    RawFacturx {
        level,
        document_type,
        version,
        xml,
    }
}

impl RawFacturx {
    /// The pairing rule: both the level and the XML, or neither.
    pub fn validated(self) -> ApiResult<Option<Facturx>> {
        match (self.level.is_empty(), self.xml) {
            (true, None) => Ok(None),
            (true, Some(_)) => Err(ApiError::bad_request(
                "Invalid form data: 'facturxConformanceLevel' is required when 'facturxXml' is provided",
            )),
            (false, None) => Err(ApiError::bad_request(
                "Invalid form data: 'facturxXml' file is required when 'facturxConformanceLevel' is set",
            )),
            (false, Some(xml)) => Ok(Some(Facturx {
                properties: FacturxProperties {
                    file_name: facturx_file_name(&self.level).to_string(),
                    conformance_level: self.level,
                    document_type: self.document_type,
                    version: self.version,
                },
                xml: xml.bytes.to_vec(),
            })),
        }
    }
}

/// `optimizeImages` and `imageQuality`.
pub fn read_optimize(form: &mut FormData) -> (bool, u8) {
    let optimize = form.boolean("optimizeImages", false);
    let quality = form
        .custom("imageQuality", |value| {
            if value.is_empty() {
                return Ok(DEFAULT_IMAGE_QUALITY);
            }
            let n = atoi(value)?;
            if n < 1 {
                return Err("value is inferior to 1".to_string());
            }
            if n > 100 {
                return Err("value is superior to 100".to_string());
            }
            Ok(n as u8)
        })
        .unwrap_or(DEFAULT_IMAGE_QUALITY);
    (optimize, quality)
}

/// `rotateAngle` and `rotatePages`.
pub fn read_rotate(form: &mut FormData, mandatory: bool) -> Option<(Rotation, String)> {
    let check = |value: &str| -> Result<Option<Rotation>, String> {
        if value.is_empty() {
            return Ok(None);
        }
        match atoi(value)? {
            90 => Ok(Some(Rotation::R90)),
            180 => Ok(Some(Rotation::R180)),
            270 => Ok(Some(Rotation::R270)),
            _ => Err("wrong value, expected 90, 180, or 270".to_string()),
        }
    };
    let angle = if mandatory {
        form.mandatory_custom("rotateAngle", check)
    } else {
        form.custom("rotateAngle", check)
    }
    .flatten()?;
    Some((angle, form.string("rotatePages")))
}

/// One `{prefix}Source` with the fields at its position.
pub struct RawOverlay {
    source: String,
    expression: String,
    pages: String,
    options: BTreeMap<String, String>,
}

/// The repeated stamp or watermark fields, aligned by position. An empty source
/// means no entry and does not shift the others. These fail at once, as they do
/// in Gotenberg, ahead of everything the form accumulates.
pub fn read_overlays(form: &FormData, prefix: &str) -> ApiResult<Vec<RawOverlay>> {
    let sources = form.values(&format!("{prefix}Source"));
    let expressions = form.values(&format!("{prefix}Expression"));
    let pages = form.values(&format!("{prefix}Pages"));
    let options = form.values(&format!("{prefix}Options"));
    let at = |values: &[String], i: usize| values.get(i).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        if source.is_empty() {
            continue;
        }
        if !matches!(source.as_str(), "text" | "image" | "pdf") {
            return Err(ApiError::bad_request(format!(
                "Invalid form data: form field '{prefix}Source' is invalid (got '{source}', resulting to wrong value, expected either 'text', 'image' or 'pdf')"
            )));
        }
        let raw_options = at(&options, i);
        let parsed = if raw_options.is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_str(&raw_options).map_err(|_| {
                ApiError::bad_request(format!(
                    "Invalid form data: form field '{prefix}Options' is invalid"
                ))
            })?
        };
        out.push(RawOverlay {
            source: source.clone(),
            expression: at(&expressions, i),
            pages: at(&pages, i),
            options: parsed,
        });
    }
    if out.len() > MAX_OVERLAYS {
        return Err(ApiError::bad_request(format!(
            "Invalid form data: at most {MAX_OVERLAYS} {prefix}s per request"
        )));
    }
    Ok(out)
}

/// Gives each image or PDF entry its uploaded file, in order. Without one it is
/// refused, so an expression can never name a path on the server.
pub fn bind_overlays(
    raw: Vec<RawOverlay>,
    files: Vec<Document>,
    kind: &str,
) -> ApiResult<Vec<Overlay>> {
    let mut files = files.into_iter();
    let mut out = Vec::with_capacity(raw.len());
    for entry in raw {
        let num = |keys: &[&str], default: f32| -> f32 {
            keys.iter()
                .find_map(|key| entry.options.get(*key))
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<f32>().ok())
                .filter(|value| value.is_finite())
                .unwrap_or(default)
        };
        let overlay_kind = match entry.source.as_str() {
            "text" => {
                if entry.expression.trim().is_empty() {
                    return Err(ApiError::bad_request(format!(
                        "Invalid form data: form field '{kind}Expression' is required for a text {kind}"
                    )));
                }
                OverlayKind::Text {
                    text: entry.expression.clone(),
                    font_size: num(&["points", "fontsize", "po"], 24.0),
                    colour: ["fillcolor", "fillc", "fillcolour", "color", "colour"]
                        .iter()
                        .find_map(|key| entry.options.get(*key))
                        .cloned()
                        .unwrap_or_else(|| "#808080".into()),
                }
            }
            source => {
                let file = files.next().ok_or_else(|| {
                    ApiError::bad_request(format!(
                        "Invalid form data: a {kind} file is required for image or pdf source"
                    ))
                })?;
                if source == "pdf" {
                    require_pdf(&file).map_err(|_| {
                        ApiError::bad_request(format!(
                            "Invalid form data: '{}' is not a PDF",
                            file.name
                        ))
                    })?;
                    OverlayKind::Pdf {
                        bytes: file.bytes.to_vec(),
                        name: file.name,
                    }
                } else {
                    let png = file.bytes.starts_with(b"\x89PNG\r\n\x1a\n");
                    let jpeg = file.bytes.starts_with(b"\xff\xd8\xff");
                    if !(png || jpeg) {
                        return Err(ApiError::bad_request(format!(
                            "Invalid form data: '{}' is not a PNG or JPEG image",
                            file.name
                        )));
                    }
                    OverlayKind::Image {
                        bytes: file.bytes.to_vec(),
                        name: file.name,
                    }
                }
            }
        };
        let position = match ["position", "pos"]
            .iter()
            .find_map(|key| entry.options.get(*key))
            .map(|value| value.trim())
            .unwrap_or("c")
        {
            "tl" | "top-left" => Position::TopLeft,
            "tc" | "t" | "top-center" => Position::TopCenter,
            "tr" | "top-right" => Position::TopRight,
            "bl" | "bottom-left" => Position::BottomLeft,
            "bc" | "b" | "bottom-center" => Position::BottomCenter,
            "br" | "bottom-right" => Position::BottomRight,
            _ => Position::Center,
        };
        out.push(Overlay {
            placement: None,
            kind: overlay_kind,
            position,
            opacity: num(&["opacity", "op"], 1.0).clamp(0.0, 1.0),
            rotation: num(&["rotation", "rot"], 0.0),
            scale: num(&["scale", "sc"], 0.5).clamp(0.01, 1.0),
            pages: entry.pages.trim().to_string(),
        });
    }
    Ok(out)
}

/// What the routes that end in post-processing read ahead of their inputs, in
/// Gotenberg's order, so its multi-line messages come out alike.
pub struct RawSteps {
    pub formats: PdfFormats,
    pub metadata: Option<Metadata>,
    pub bookmarks: Bookmarks,
    pub encrypt: EncryptOptions,
    pub embeds: Vec<Attachment>,
    watermarks: Vec<RawOverlay>,
    stamps: Vec<RawOverlay>,
    watermark_files: Vec<Document>,
    stamp_files: Vec<Document>,
    pub rotate: Option<(Rotation, String)>,
    pub embeds_metadata: BTreeMap<String, EmbedMetadata>,
    pub facturx: RawFacturx,
    pub optimize: (bool, u8),
}

/// Reads the fields, ahead of the route's own. `with_bookmarks` is the merge
/// route's: elsewhere `bookmarks` is not read at all.
pub fn read_steps(form: &mut FormData, with_bookmarks: bool) -> ApiResult<RawSteps> {
    let formats = read_formats(form);
    let metadata = read_metadata(form, false);
    let bookmarks = if with_bookmarks {
        read_bookmarks(form, false)
    } else {
        Bookmarks::None
    };
    let encrypt = read_encrypt(form);
    let embeds = read_embeds(form);
    let watermarks = read_overlays(form, "watermark")?;
    let stamps = read_overlays(form, "stamp")?;
    let watermark_files = form.take_files("watermark");
    let stamp_files = form.take_files("stamp");
    let rotate = read_rotate(form, false);
    let embeds_metadata = read_embeds_metadata(form);
    let facturx = read_facturx(form);
    let optimize = read_optimize(form);
    Ok(RawSteps {
        formats,
        metadata,
        bookmarks,
        encrypt,
        embeds,
        watermarks,
        stamps,
        watermark_files,
        stamp_files,
        rotate,
        embeds_metadata,
        facturx,
        optimize,
    })
}

impl RawSteps {
    /// Binds the files and checks the combinations, after the form validated.
    pub fn bind(self, flatten: bool) -> ApiResult<PostSteps> {
        let watermarks = bind_overlays(self.watermarks, self.watermark_files, "watermark")?;
        let stamps = bind_overlays(self.stamps, self.stamp_files, "stamp")?;
        let steps = PostSteps {
            watermarks,
            stamps,
            rotate: self.rotate,
            flatten,
            optimize_images: self.optimize.0.then_some(self.optimize.1),
            formats: self.formats,
            metadata: self.metadata,
            embeds: self.embeds,
            embeds_metadata: self.embeds_metadata,
            facturx: None,
            encrypt: self.encrypt,
        };
        rusticdoc_core::compat::validate_formats(
            &steps.formats,
            &steps.encrypt.user_password,
            &steps.embeds,
        )
        .map_err(super::client_error)?;
        rusticdoc_core::compat::validate_permissions(&steps.encrypt)
            .map_err(super::client_error)?;
        let facturx = self.facturx.validated()?;
        let steps = PostSteps { facturx, ..steps };
        rusticdoc_core::compat::validate_steps(&steps).map_err(super::client_error)?;
        Ok(steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(fields: &[(&str, &str)]) -> FormData {
        let mut form = Form::default();
        for (key, value) in fields {
            form.multi
                .entry(key.to_string())
                .or_default()
                .push(value.to_string());
            form.fields.insert(key.to_string(), value.to_string());
        }
        FormData::new(form)
    }

    fn message(form: &FormData) -> String {
        form.validate().unwrap_err().body.message
    }

    #[test]
    fn errors_accumulate_in_order() {
        let mut f = form(&[("flag", "foo")]);
        assert!(!f.boolean("flag", false));
        assert_eq!(
            read_bookmarks(&mut f, true),
            Bookmarks::None,
            "a missing mandatory field records a problem and carries on"
        );
        f.mandatory_paths(&[".pdf"]);
        assert_eq!(
            message(&f),
            "Invalid form data: form field 'flag' is invalid (got 'foo', resulting to strconv.ParseBool: parsing \"foo\": invalid syntax)\nform field 'bookmarks' is required\nno form file found for extensions: [.pdf]"
        );
    }

    #[test]
    fn json_errors_read_like_gos() {
        assert_eq!(
            go_json_error("foo"),
            "invalid character 'o' in literal false (expecting 'a')"
        );
        assert_eq!(
            go_json_error("nope"),
            "invalid character 'o' in literal null (expecting 'u')"
        );
        assert_eq!(
            go_json_error("?"),
            "invalid character '?' looking for beginning of value"
        );
        assert_eq!(go_json_error(""), "unexpected end of JSON input");
        assert_eq!(go_json_error("[1"), "unexpected end of JSON input");
        let mut f = form(&[("metadata", "foo")]);
        assert_eq!(read_metadata(&mut f, false), None);
        assert_eq!(
            message(&f),
            "Invalid form data: form field 'metadata' is invalid (got 'foo', resulting to unmarshal metadata: invalid character 'o' in literal false (expecting 'a'))"
        );
    }

    #[test]
    fn split_fields() {
        let mut f = form(&[("splitMode", "intervals"), ("splitSpan", "foo")]);
        read_split(&mut f, false);
        assert_eq!(
            message(&f),
            "Invalid form data: form field 'splitSpan' is invalid (got 'foo', resulting to strconv.Atoi: parsing \"foo\": invalid syntax)"
        );
        let mut f = form(&[("splitMode", "sideways"), ("splitSpan", "1")]);
        read_split(&mut f, true);
        assert_eq!(
            message(&f),
            "Invalid form data: form field 'splitMode' is invalid (got 'sideways', resulting to wrong value, expected either 'intervals' or 'pages')"
        );
        let mut f = form(&[]);
        assert_eq!(read_split(&mut f, false), None);
        assert!(f.validate().is_ok());
        read_split(&mut f, true);
        assert_eq!(
            message(&f),
            "Invalid form data: form field 'splitMode' is required\nform field 'splitSpan' is required"
        );
    }

    #[test]
    fn names_stay_unique_and_inputs_sort_by_number() {
        let mut raw = Form::default();
        for name in ["page_10.pdf", "page_2.pdf", "page_2.pdf", "note.txt"] {
            raw.files.push(Document::new(name, b"%PDF-".to_vec()));
        }
        let f = FormData::new(raw);
        let names: Vec<String> = f.paths(&[".pdf"]).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["page_2.pdf", "page_2 (2).pdf", "page_10.pdf"]);
    }

    #[test]
    fn overlay_files_are_required_and_checked() {
        let f = form(&[("stampSource", "pdf"), ("stampExpression", "/etc/hostname")]);
        let raw = read_overlays(&f, "stamp").unwrap();
        let err = bind_overlays(raw, Vec::new(), "stamp").unwrap_err();
        assert_eq!(
            err.body.message,
            "Invalid form data: a stamp file is required for image or pdf source"
        );
        let f = form(&[("stampSource", "image")]);
        let raw = read_overlays(&f, "stamp").unwrap();
        let err = bind_overlays(
            raw,
            vec![Document::new("a.png", b"not an image".to_vec())],
            "stamp",
        )
        .unwrap_err();
        assert!(err.body.message.contains("not a PNG or JPEG"));
        let f = form(&[
            ("stampSource", "text"),
            ("stampExpression", "DRAFT"),
            ("stampOptions", r##"{"scale":"0.5 abs","rot":"45","fillcolor":"#FF0000"}"##),
        ]);
        let raw = read_overlays(&f, "stamp").unwrap();
        let bound = bind_overlays(raw, Vec::new(), "stamp").unwrap();
        assert_eq!(bound[0].scale, 0.5);
        assert_eq!(bound[0].rotation, 45.0);
        let f = form(&[("stampSource", "foo")]);
        assert!(read_overlays(&f, "stamp").is_err());
        let six = vec![("stampSource", "text"); 6];
        let f = form(&six);
        assert!(read_overlays(&f, "stamp").is_err());
    }
}
