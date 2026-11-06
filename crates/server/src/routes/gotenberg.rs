//! Gotenberg's post-processing chain, read from its form fields.
//!
//! Gotenberg's convert, merge and split routes all accept the same extra
//! fields and apply them in one fixed order after the main operation:
//! merge, split, watermark, stamp, rotate, flatten, optimize, PDF/A and
//! PDF/UA, bookmarks, metadata, embeds, Factur-X, encrypt. This module turns
//! those fields into the pipeline's steps in that order, so the aliases
//! behave as Gotenberg's clients expect and `/v1/*` gains the same options.

use std::collections::BTreeMap;

use rusticdoc_core::Step;
use rusticdoc_core::job::PdfaLevel;
use rusticdoc_core::pdfops::{
    Attachment, Bookmark, FacturxProfile, Metadata, Overlay, OverlayKind, Position, Rotation,
    SplitMode,
};

use crate::error::{ApiError, ApiResult};
use crate::routes::form::Form;

/// Most watermarks or stamps one request may apply, each a pass over the
/// document.
const MAX_OVERLAYS: usize = 5;

/// Steps for the fields present in the form, in Gotenberg's order. `merge`
/// is included only when `allow_merge` (the convert route); the merge
/// route merges first on its own.
pub fn post_steps(form: &mut Form, allow_merge: bool) -> ApiResult<Vec<Step>> {
    let mut steps = Vec::new();

    if allow_merge && form.bool("merge") {
        steps.push(Step::Merge);
    }
    if let Some(mode) = split_mode(form)? {
        steps.push(Step::Split { mode });
    }
    for overlay in overlays(form, "watermark")? {
        steps.push(Step::Watermark { overlay });
    }
    for overlay in overlays(form, "stamp")? {
        steps.push(Step::Stamp { overlay });
    }
    if let Some(angle) = form
        .field("rotateAngle")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        steps.push(Step::Rotate {
            rotation: Rotation::parse(angle)?,
            pages: form.field("rotatePages").unwrap_or("").trim().to_string(),
        });
    }
    if form.bool("flatten") {
        steps.push(Step::Flatten);
    }
    if form.bool("optimizeImages") || form.field("imageQuality").is_some() {
        steps.push(Step::Optimize);
    }
    if let Some(level) = form.field("pdfa").map(str::trim).filter(|s| !s.is_empty()) {
        steps.push(Step::Pdfa {
            level: PdfaLevel::parse(level).map_err(|_| {
                ApiError::bad_request(format!(
                    "The PDF format '{level}' is not supported. Valid formats include PDF/A-1b, PDF/A-2b, PDF/A-3b, PDF/A-4, and PDF/UA."
                ))
            })?,
        });
    }
    if form.bool("pdfua") {
        steps.push(Step::Pdfua);
    }
    if let Some(raw) = form
        .field("bookmarks")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let bookmarks: Vec<Bookmark> = serde_json::from_str(raw).map_err(|e| {
            ApiError::bad_request(format!(
                "Invalid form data: form field 'bookmarks' is invalid ({e})"
            ))
        })?;
        steps.push(Step::Bookmarks { bookmarks });
    }
    if let Some(raw) = form
        .field("metadata")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let metadata: Metadata = match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(serde_json::Value::Object(map)) => map.into_iter().collect(),
            _ => {
                return Err(ApiError::bad_request(
                    "Invalid form data: form field 'metadata' is invalid (not a JSON object)",
                ));
            }
        };
        steps.push(Step::Metadata { metadata });
    }
    let embeds = form.take_field("embeds");
    if !embeds.is_empty() {
        let metadata: BTreeMap<String, serde_json::Value> = form
            .field("embedsMetadata")
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        let files = embeds
            .into_iter()
            .map(|d| Attachment {
                mime: metadata
                    .get(&d.name)
                    .and_then(|m| m.get("mimeType"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| d.content_type().to_string()),
                name: d.name,
                bytes: d.bytes.to_vec(),
            })
            .collect();
        steps.push(Step::Embed { files });
    }
    let facturx_xml = form.take_field("facturxXml");
    if let Some(xml) = facturx_xml.into_iter().next() {
        let profile = form
            .field("facturxConformanceLevel")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(FacturxProfile::parse)
            .transpose()?
            .unwrap_or(FacturxProfile::En16931);
        steps.push(Step::Facturx {
            xml: xml.bytes.to_vec(),
            profile,
        });
    }
    let user_password = form.field("userPassword").unwrap_or("").to_string();
    let owner_password = form.field("ownerPassword").unwrap_or("").to_string();
    if !user_password.is_empty() || !owner_password.is_empty() {
        steps.push(Step::Encrypt {
            owner_password: if owner_password.is_empty() {
                user_password.clone()
            } else {
                owner_password
            },
            user_password,
        });
    }
    Ok(steps)
}

fn split_mode(form: &Form) -> ApiResult<Option<SplitMode>> {
    let mode = form.field("splitMode").map(str::trim).unwrap_or("");
    let span = form.field("splitSpan").map(str::trim).unwrap_or("");
    if mode.is_empty() && span.is_empty() {
        return Ok(None);
    }
    if mode.is_empty() || span.is_empty() {
        return Err(ApiError::bad_request(
            "Invalid form data: 'splitMode' and 'splitSpan' go together",
        ));
    }
    Ok(Some(match mode {
        "pages" => SplitMode::Pages {
            ranges: span.to_string(),
            unify: form.bool("splitUnify"),
        },
        "intervals" => SplitMode::Intervals {
            span: span.parse().map_err(|_| {
                ApiError::bad_request(format!(
                    "Invalid form data: form field 'splitSpan' is invalid (got '{span}', expected a page count)"
                ))
            })?,
        },
        other => {
            return Err(ApiError::bad_request(format!(
                "Invalid form data: form field 'splitMode' is invalid (got '{other}', expected 'pages' or 'intervals')"
            )));
        }
    }))
}

/// `<prefix>Source`, `<prefix>Expression`, `<prefix>Pages` and
/// `<prefix>Options` (a JSON object in pdfcpu's vocabulary: `pos`, `op`,
/// `rot`, `sc`, `fontsize`, `fillc`), with image and PDF sources bound to
/// the files uploaded under the `<prefix>` field in order.
fn overlays(form: &mut Form, prefix: &str) -> ApiResult<Vec<Overlay>> {
    let sources = form.all(&format!("{prefix}Source"));
    if sources.is_empty() {
        if form.field(&format!("{prefix}Expression")).is_some() {
            return Err(ApiError::bad_request(format!(
                "Invalid form data: form field '{prefix}Source' is required with '{prefix}Expression'"
            )));
        }
        return Ok(Vec::new());
    }
    if sources.len() > MAX_OVERLAYS {
        return Err(ApiError::bad_request(format!(
            "Invalid form data: at most {MAX_OVERLAYS} {prefix}s per request"
        )));
    }
    let expressions = form.all(&format!("{prefix}Expression"));
    let pages = form.all(&format!("{prefix}Pages"));
    let options = form.all(&format!("{prefix}Options"));
    let mut files = form.take_field(prefix).into_iter();
    let mut out = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        let at = |v: &[String]| v.get(i).cloned().unwrap_or_default();
        let opts: BTreeMap<String, String> = match at(&options).trim() {
            "" => BTreeMap::new(),
            raw => serde_json::from_str(raw).map_err(|_| {
                ApiError::bad_request(format!(
                    "Invalid form data: form field '{prefix}Options' is invalid"
                ))
            })?,
        };
        let kind = match source.trim() {
            "text" => {
                let text = at(&expressions);
                if text.trim().is_empty() {
                    return Err(ApiError::bad_request(format!(
                        "Invalid form data: form field '{prefix}Expression' is required for a text {prefix}"
                    )));
                }
                OverlayKind::Text {
                    text,
                    font_size: opts
                        .get("fontsize")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(24.0),
                    colour: opts
                        .get("fillc")
                        .or(opts.get("fillcolor"))
                        .cloned()
                        .unwrap_or_else(|| "#808080".into()),
                }
            }
            "image" | "pdf" => {
                let file = files.next().ok_or_else(|| {
                    ApiError::bad_request(format!(
                        "Invalid form data: a {prefix} with source '{source}' needs a file uploaded under '{prefix}'"
                    ))
                })?;
                if source.trim() == "pdf" {
                    rusticdoc_core::pdfops::require_pdf(&file)?;
                    OverlayKind::Pdf {
                        bytes: file.bytes.to_vec(),
                        name: file.name,
                    }
                } else {
                    let png = file.bytes.starts_with(b"\x89PNG\r\n\x1a\n");
                    let jpeg = file.bytes.starts_with(b"\xff\xd8\xff");
                    if !(png || jpeg)
                        || !matches!(file.extension().as_str(), "png" | "jpg" | "jpeg")
                    {
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
            other => {
                return Err(ApiError::bad_request(format!(
                    "Invalid form data: form field '{prefix}Source' is invalid (got '{other}', resulting to wrong value, expected either 'text', 'image' or 'pdf')"
                )));
            }
        };
        let position = match opts.get("pos").map(|s| s.trim()).unwrap_or("c") {
            "tl" => Position::TopLeft,
            "tc" | "t" => Position::TopCenter,
            "tr" => Position::TopRight,
            "c" | "" => Position::Center,
            "bl" => Position::BottomLeft,
            "bc" | "b" => Position::BottomCenter,
            "br" => Position::BottomRight,
            "l" => Position::Center,
            "r" => Position::Center,
            other => {
                return Err(ApiError::bad_request(format!(
                    "Invalid form data: form field '{prefix}Options' has an unknown pos '{other}'"
                )));
            }
        };
        let num = |key: &str, default: f32| {
            opts.get(key)
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(default)
        };
        out.push(Overlay {
            placement: None,
            kind,
            position,
            opacity: num("op", 1.0).clamp(0.0, 1.0),
            rotation: num("rot", 0.0),
            scale: num("sc", 0.5).clamp(0.01, 1.0),
            pages: at(&pages).trim().to_string(),
        });
    }
    Ok(out)
}

/// A boolean field in Gotenberg's strict form: `true` or `false`, or a 400.
fn bool_field(form: &Form, name: &str) -> ApiResult<Option<bool>> {
    match form.field(name).map(|v| v.trim().to_ascii_lowercase()) {
        None => Ok(None),
        Some(v) if v == "true" || v == "1" => Ok(Some(true)),
        Some(v) if v == "false" || v == "0" || v.is_empty() => Ok(Some(false)),
        Some(v) => Err(ApiError::bad_request(format!(
            "Invalid form data: form field '{name}' is invalid (got '{v}', resulting to wrong value, expected a boolean)"
        ))),
    }
}

/// LibreOffice conversion options from Gotenberg's field names.
pub fn convert_options(form: &Form) -> ApiResult<rusticdoc_core::convert::ConvertOptions> {
    use rusticdoc_core::convert::ConvertOptions;
    let mut o = ConvertOptions::default();
    let flag = |name: &str, slot: &mut bool| -> ApiResult<()> {
        if let Some(v) = bool_field(form, name)? {
            *slot = v;
        }
        Ok(())
    };
    flag("landscape", &mut o.landscape)?;
    flag("exportFormFields", &mut o.export_form_fields)?;
    flag(
        "allowDuplicateFieldNames",
        &mut o.allow_duplicate_field_names,
    )?;
    flag("exportBookmarks", &mut o.export_bookmarks)?;
    flag(
        "exportBookmarksToPdfDestination",
        &mut o.export_bookmarks_to_pdf_destination,
    )?;
    flag("exportPlaceholders", &mut o.export_placeholders)?;
    flag("exportNotes", &mut o.export_notes)?;
    flag("exportNotesPages", &mut o.export_notes_pages)?;
    flag("exportOnlyNotesPages", &mut o.export_only_notes_pages)?;
    flag("exportNotesInMargin", &mut o.export_notes_in_margin)?;
    flag(
        "convertOooTargetToPdfTarget",
        &mut o.convert_ooo_target_to_pdf_target,
    )?;
    flag("exportLinksRelativeFsys", &mut o.export_links_relative_fsys)?;
    flag("exportHiddenSlides", &mut o.export_hidden_slides)?;
    flag("skipEmptyPages", &mut o.skip_empty_pages)?;
    flag(
        "addOriginalDocumentAsStream",
        &mut o.add_original_document_as_stream,
    )?;
    flag("singlePageSheets", &mut o.single_page_sheets)?;
    flag(
        "losslessImageCompression",
        &mut o.lossless_image_compression,
    )?;
    flag("reduceImageResolution", &mut o.reduce_image_resolution)?;
    flag("pdfua", &mut o.pdfua)?;
    if let Some(v) = form
        .field("nativePageRanges")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        o.native_page_ranges = Some(v.to_string());
    }
    if let Some(v) = form.field("password").filter(|s| !s.is_empty()) {
        o.password = Some(v.to_string());
    }
    if let Some(v) = form
        .field("quality")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        o.quality = v.parse().map_err(|_| {
            ApiError::bad_request(format!(
                "Invalid form data: form field 'quality' is invalid (got '{v}', expected a number)"
            ))
        })?;
    }
    if let Some(v) = form
        .field("maxImageResolution")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        o.max_image_resolution = v.parse().map_err(|_| {
            ApiError::bad_request(format!(
                "Invalid form data: form field 'maxImageResolution' is invalid (got '{v}', expected a number)"
            ))
        })?;
    }
    if let Some(level) = form.field("pdfa").map(str::trim).filter(|s| !s.is_empty()) {
        o.pdfa = Some(PdfaLevel::parse(level).map_err(|_| {
            ApiError::bad_request(format!(
                "The PDF format '{level}' is not supported. Valid formats include PDF/A-1b, PDF/A-2b, PDF/A-3b, PDF/A-4, and PDF/UA."
            ))
        })?);
    }
    if let Some(fonts) = form.by_field.get("fonts") {
        o.fonts = fonts
            .iter()
            .map(|f| (f.name.clone(), f.bytes.to_vec()))
            .collect();
    }
    o.validate()?;
    Ok(o)
}
