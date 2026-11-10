//! PDF operations, one route each. Every route reads a multipart form, runs the operation through
//! the engines in fallback order, and returns the file, or a ZIP when the
//! operation yields several, or JSON for reads.

use std::collections::BTreeMap;

use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::{Extension, Json};
use rusticdoc_core::pdfops::{
    Attachment, Bookmark, FacturxProfile, Metadata, Overlay, OverlayKind, Position, Rotation,
    SplitMode, require_pdf, sign,
};
use rusticdoc_core::{Document, Output};
use utoipa::ToSchema;

use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, BinaryFile, ErrorBody};
use crate::routes::form::{Form, request_id, zip};
use crate::routes::forms::finish;
use crate::state::AppState;
use crate::webhook::Hook;

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct PdfForm {
    /// The PDF to work on. Several for merge.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Vec<String>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct SplitForm {
    #[schema(value_type = String, format = Binary)]
    pub files: String,
    /// `pages` or `intervals`.
    #[schema(rename = "splitMode", example = "pages")]
    pub split_mode: String,
    /// For `pages`: ranges such as `1-3,5`. For `intervals`: pages per file.
    #[schema(rename = "splitSpan", example = "1-2")]
    pub split_span: String,
    /// For `pages`: one file holding every range instead of one per range.
    #[schema(rename = "splitUnify")]
    pub split_unify: Option<bool>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct RotateForm {
    #[schema(value_type = String, format = Binary)]
    pub files: String,
    /// 90, 180 or 270.
    #[schema(example = "90")]
    pub rotation: String,
    /// Page selection such as `1-3,5`; all when empty.
    pub pages: Option<String>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct MetadataWriteForm {
    #[schema(value_type = String, format = Binary)]
    pub files: String,
    /// JSON object of keys such as `Title`, `Author`, `Subject`, `Keywords`,
    /// sent as text.
    #[schema(value_type = String, example = r#"{"Title": "Invoice", "Author": "Ada"}"#)]
    pub metadata: serde_json::Value,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct BookmarksWriteForm {
    #[schema(value_type = String, format = Binary)]
    pub files: String,
    /// JSON array of `{title, page, children}`, sent as text.
    #[schema(value_type = String, example = r#"[{"title": "Summary", "page": 1}]"#)]
    pub bookmarks: serde_json::Value,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct EmbedForm {
    /// The PDF first, then the files to attach.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Vec<String>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct MarkForm {
    /// The PDF, and optionally an image or PDF to use as the mark.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Vec<String>,
    /// Text for a text mark; omit when sending an image or PDF.
    pub text: Option<String>,
    /// `top-left`, `top-center`, `top-right`, `center`, `bottom-left`, `bottom-center`, `bottom-right`.
    pub position: Option<String>,
    /// 0 to 1.
    pub opacity: Option<f32>,
    /// Degrees.
    pub rotation: Option<f32>,
    /// Relative to the page width, 0 to 1.
    pub scale: Option<f32>,
    /// Size of a text mark in points. Default 24.
    #[schema(rename = "fontSize")]
    pub font_size: Option<f32>,
    /// Hex colour such as `#ff0000`. Default `#808080`.
    pub colour: Option<String>,
    /// Page selection such as `1-3,5`.
    pub pages: Option<String>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct SignForm {
    /// The PDF first, then one PNG or JPEG per placement. With
    /// `signature=certificate`, at most one image: the visible appearance.
    /// With `signature=renew`, the PDF alone.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Vec<String>,
    /// `image` (default) draws the images and is not a cryptographic
    /// signature. `certificate` signs with the service's key (PAdES,
    /// `/ETSI.CAdES.detached`), invisibly unless an image or a position is
    /// sent. `renew` takes a file signed at `b-lta` and adds validation data
    /// for its last document timestamp and a new document timestamp, which
    /// keeps the signature verifiable after the old timestamp's certificate
    /// expires; it takes no other field than `outputFilename`.
    #[schema(example = "certificate")]
    pub signature: Option<String>,
    /// 1-based page of each image; repeat the field for several.
    pub page: Option<u32>,
    /// Points from the page's left edge to the image's left edge.
    pub x: Option<f32>,
    /// Points from the page's bottom edge to the image's bottom edge.
    pub y: Option<f32>,
    /// Width in points. Default the image's natural size, or 200 for a text
    /// appearance.
    pub width: Option<f32>,
    /// With `signature=certificate`: why the document was signed.
    pub reason: Option<String>,
    /// With `signature=certificate`: where it was signed.
    pub location: Option<String>,
    /// With `signature=certificate`: how to reach the signer.
    #[schema(rename = "contactInfo")]
    pub contact_info: Option<String>,
    /// With `signature=certificate`: the signer's name as shown. Validators
    /// identify the signer by the certificate, not by this.
    #[schema(rename = "signerName")]
    pub signer_name: Option<String>,
    /// With `signature=certificate`: the PAdES level, `b-b`, `b-t` (with an
    /// RFC 3161 timestamp), `b-lt` (and validation data in a DSS) or `b-lta`
    /// (and a document timestamp). Default the service's
    /// `RUSTICDOC_SIGNING_LEVEL`, `b-b` unless set.
    #[schema(example = "b-lt")]
    pub level: Option<String>,
    /// With `signature=certificate`: certify rather than approve, with this
    /// DocMDP permission: `1` no changes, `2` form filling and signing, `3`
    /// also annotations. Only the document's first signature can certify.
    pub certify: Option<u8>,
    /// With `signature=certificate`: a named key from `RUSTICDOC_SIGNING_KEYS`
    /// instead of the default one. A caller not on its allow list gets 403.
    #[schema(rename = "signingKey")]
    pub signing_key: Option<String>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct FacturxForm {
    /// The PDF/A-3 invoice, then the Factur-X XML.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Vec<String>,
    /// `MINIMUM`, `BASIC_WL`, `BASIC`, `EN16931` or `EXTENDED`.
    #[schema(example = "EN16931")]
    pub profile: String,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

fn output_name(form: &Form, doc: &Document, suffix: &str) -> String {
    form.output_name()
        .map(|s| format!("{s}.pdf"))
        .unwrap_or_else(|| {
            if suffix.is_empty() {
                doc.with_extension("pdf")
            } else {
                let stem = std::path::Path::new(&doc.name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document");
                format!("{stem}-{suffix}.pdf")
            }
        })
}

fn one_pdf(form: &mut Form) -> ApiResult<Document> {
    let doc = form.require_file("one PDF")?;
    require_pdf(&doc)?;
    Ok(doc)
}

fn pdf_output(name: String, doc: Document, stage: &str) -> Output {
    Output {
        name,
        content_type: "application/pdf",
        bytes: doc.bytes,
        timings: vec![(stage.into(), 0)],
    }
}

/// Split a PDF
///
/// Splits a PDF by page ranges or into fixed intervals; several outputs
/// come back as a ZIP.
#[utoipa::path(
    post, path = "/v1/pdf/split", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = SplitForm),
    responses((status = 200, description = "A PDF, or a ZIP of PDFs", content_type = "application/octet-stream", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn split(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    let span = form
        .field("splitSpan")
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let mode = match form.field("splitMode").map(str::trim).unwrap_or("pages") {
        "pages" => SplitMode::Pages {
            ranges: span.clone(),
            unify: form.bool("splitUnify"),
        },
        "intervals" => SplitMode::Intervals {
            span: span.parse().map_err(|_| {
                ApiError::bad_request("'splitSpan' must be a page count for intervals")
            })?,
        },
        other => {
            return Err(ApiError::bad_request(format!(
                "'splitMode' must be pages or intervals, not '{other}'"
            )));
        }
    };
    if span.is_empty() {
        return Err(ApiError::bad_request("'splitSpan' is required"));
    }
    let stem = form.output_name().unwrap_or_else(|| {
        std::path::Path::new(&doc.name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("split")
            .to_string()
    });
    let st = state.clone();
    let job = async move {
        let parts = st
            .engines
            .pdf
            .split(&doc, &mode, st.config.api_timeout)
            .await?;
        let outputs: Vec<Output> = parts
            .into_iter()
            .enumerate()
            .map(|(i, d)| pdf_output(format!("{stem}_{}.pdf", i + 1), d, "split"))
            .collect();
        if outputs.len() == 1 {
            let mut only = outputs.into_iter().next().expect("one");
            only.name = format!("{stem}.pdf");
            Ok(only)
        } else {
            zip(&stem, &outputs, st.config.api_timeout).await
        }
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Flatten a PDF
///
/// Flattens forms and annotations into the page content.
#[utoipa::path(
    post, path = "/v1/pdf/flatten", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = PdfForm),
    responses((status = 200, description = "The flattened PDF", content_type = "application/pdf", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn flatten(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        Ok(pdf_output(
            name,
            st.engines.pdf.flatten(&doc, st.config.api_timeout).await?,
            "flatten",
        ))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Rotate pages
///
/// Rotates pages by 90, 180 or 270 degrees.
#[utoipa::path(
    post, path = "/v1/pdf/rotate", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = RotateForm),
    responses((status = 200, description = "The rotated PDF", content_type = "application/pdf", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn rotate(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    let rotation = Rotation::parse(form.field("rotation").unwrap_or("90"))?;
    let pages = form.field("pages").unwrap_or("").trim().to_string();
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        Ok(pdf_output(
            name,
            st.engines
                .pdf
                .rotate(&doc, rotation, &pages, st.config.api_timeout)
                .await?,
            "rotate",
        ))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Optimise a PDF
///
/// Recompresses streams and drops unused objects.
#[utoipa::path(
    post, path = "/v1/pdf/optimize", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = PdfForm),
    responses((status = 200, description = "The smaller PDF", content_type = "application/pdf", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn optimize(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        Ok(pdf_output(
            name,
            st.engines.pdf.optimize(&doc, st.config.api_timeout).await?,
            "optimize",
        ))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Read metadata
///
/// Reads the metadata of each PDF sent, keyed by file name.
#[utoipa::path(
    post, path = "/v1/pdf/metadata/read", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = PdfForm),
    responses((status = 200, description = "Per file name", body = Object), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn metadata_read(
    State(state): State<AppState>,
    multipart: Multipart,
) -> ApiResult<Json<BTreeMap<String, Metadata>>> {
    let mut form = Form::read(multipart, &state).await?;
    if form.files.is_empty() {
        return Err(ApiError::bad_request("send at least one PDF"));
    }
    let mut all = BTreeMap::new();
    for doc in std::mem::take(&mut form.files) {
        require_pdf(&doc)?;
        let meta = state
            .engines
            .pdf
            .read_metadata(&doc, state.config.api_timeout)
            .await?;
        all.insert(doc.name.clone(), meta);
    }
    Ok(Json(all))
}

/// Write metadata
///
/// Writes metadata keys; existing keys not named are kept.
#[utoipa::path(
    post, path = "/v1/pdf/metadata/write", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = MetadataWriteForm),
    responses((status = 200, description = "The PDF with its metadata changed", content_type = "application/pdf", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn metadata_write(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    let metadata: Metadata = match form.json("metadata")? {
        serde_json::Value::Object(map) if !map.is_empty() => map.into_iter().collect(),
        _ => {
            return Err(ApiError::bad_request(
                "'metadata' must be a non-empty JSON object",
            ));
        }
    };
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        Ok(pdf_output(
            name,
            st.engines
                .pdf
                .write_metadata(&doc, &metadata, st.config.api_timeout)
                .await?,
            "metadata",
        ))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Read bookmarks
///
/// Reads the outline of each PDF sent, keyed by file name.
#[utoipa::path(
    post, path = "/v1/pdf/bookmarks/read", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = PdfForm),
    responses((status = 200, description = "Per file name", body = Object), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn bookmarks_read(
    State(state): State<AppState>,
    multipart: Multipart,
) -> ApiResult<Json<BTreeMap<String, Vec<Bookmark>>>> {
    let mut form = Form::read(multipart, &state).await?;
    if form.files.is_empty() {
        return Err(ApiError::bad_request("send at least one PDF"));
    }
    let mut all = BTreeMap::new();
    for doc in std::mem::take(&mut form.files) {
        require_pdf(&doc)?;
        let marks = state
            .engines
            .pdf
            .read_bookmarks(&doc, state.config.api_timeout)
            .await?;
        all.insert(doc.name.clone(), marks);
    }
    Ok(Json(all))
}

/// Write bookmarks
///
/// Replaces the outline.
#[utoipa::path(
    post, path = "/v1/pdf/bookmarks/write", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = BookmarksWriteForm),
    responses((status = 200, description = "The PDF with its outline replaced", content_type = "application/pdf", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn bookmarks_write(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    let bookmarks: Vec<Bookmark> =
        serde_json::from_value(form.json("bookmarks")?).map_err(|e| {
            ApiError::bad_request(format!(
                "'bookmarks' must be an array of {{title, page, children}}: {e}"
            ))
        })?;
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        Ok(pdf_output(
            name,
            st.engines
                .pdf
                .write_bookmarks(&doc, &bookmarks, st.config.api_timeout)
                .await?,
            "bookmarks",
        ))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Attach files
///
/// Attaches files to a PDF.
#[utoipa::path(
    post, path = "/v1/pdf/embed", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = EmbedForm),
    responses((status = 200, description = "The PDF with attachments", content_type = "application/pdf", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn embed(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    if form.files.is_empty() {
        return Err(ApiError::bad_request(
            "send the PDF first, then at least one file to attach",
        ));
    }
    let files: Vec<Attachment> = std::mem::take(&mut form.files)
        .into_iter()
        .map(|f| Attachment {
            mime: f.content_type().to_string(),
            name: f.name,
            bytes: f.bytes.to_vec(),
        })
        .collect();
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        Ok(pdf_output(
            name,
            st.engines
                .pdf
                .embed(&doc, &files, st.config.api_timeout)
                .await?,
            "embed",
        ))
    };
    finish(state, hook, request_id(&headers), job).await
}

fn overlay_from(form: &mut Form) -> ApiResult<Overlay> {
    let kind = if let Some(text) = form.field("text").map(str::trim).filter(|t| !t.is_empty()) {
        OverlayKind::Text {
            text: text.to_string(),
            font_size: form
                .field("fontSize")
                .and_then(|v| v.parse().ok())
                .unwrap_or(24.0),
            colour: form
                .field("colour")
                .or(form.field("color"))
                .unwrap_or("#808080")
                .to_string(),
        }
    } else if let Some(mark) = form.files.first().cloned() {
        form.files.remove(0);
        let png = mark.bytes.starts_with(b"\x89PNG\r\n\x1a\n");
        let jpeg = mark.bytes.starts_with(b"\xff\xd8\xff");
        match mark.extension().as_str() {
            "pdf" => {
                require_pdf(&mark)?;
                OverlayKind::Pdf {
                    bytes: mark.bytes.to_vec(),
                    name: mark.name,
                }
            }
            "png" | "jpg" | "jpeg" if png || jpeg => OverlayKind::Image {
                bytes: mark.bytes.to_vec(),
                name: mark.name,
            },
            other => {
                return Err(ApiError::bad_request(format!(
                    "a mark must be text, a PNG or JPEG image, or a PDF; '{}' (.{other}) is none of these",
                    mark.name
                )));
            }
        }
    } else {
        return Err(ApiError::bad_request(
            "send 'text', or an image or PDF as a second file",
        ));
    };
    let position = match form.field("position").map(str::trim).unwrap_or("center") {
        "top-left" => Position::TopLeft,
        "top-center" | "top" => Position::TopCenter,
        "top-right" => Position::TopRight,
        "center" | "" => Position::Center,
        "bottom-left" => Position::BottomLeft,
        "bottom-center" | "bottom" => Position::BottomCenter,
        "bottom-right" => Position::BottomRight,
        other => return Err(ApiError::bad_request(format!("unknown position '{other}'"))),
    };
    let num = |name: &str, default: f32| {
        form.field(name)
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(default)
    };
    let opacity = num("opacity", 1.0);
    if !(0.0..=1.0).contains(&opacity) {
        return Err(ApiError::bad_request("'opacity' must be between 0 and 1"));
    }
    let overlay = Overlay {
        placement: None,
        kind,
        position,
        opacity,
        rotation: num("rotation", 0.0),
        scale: num("scale", 0.5).clamp(0.01, 1.0),
        pages: form.field("pages").unwrap_or("").trim().to_string(),
    };
    overlay.validate()?;
    Ok(overlay)
}

/// Add a watermark
///
/// Puts text, an image or a PDF under the content of each page.
#[utoipa::path(
    post, path = "/v1/pdf/watermark", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = MarkForm),
    responses((status = 200, description = "The watermarked PDF", content_type = "application/pdf", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn watermark(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    let overlay = overlay_from(&mut form)?;
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        Ok(pdf_output(
            name,
            st.engines
                .pdf
                .watermark(&doc, &overlay, st.config.api_timeout)
                .await?,
            "watermark",
        ))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Add a stamp
///
/// Puts text, an image or a PDF over the content of each page.
#[utoipa::path(
    post, path = "/v1/pdf/stamp", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = MarkForm),
    responses((status = 200, description = "The stamped PDF", content_type = "application/pdf", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn stamp(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    let overlay = overlay_from(&mut form)?;
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        Ok(pdf_output(
            name,
            st.engines
                .pdf
                .stamp(&doc, &overlay, st.config.api_timeout)
                .await?,
            "stamp",
        ))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// A placement coordinate or width: finite and not negative.
fn placement_number(name: &str, raw: &str) -> ApiResult<f32> {
    raw.trim()
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "'{name}' must be a non-negative number, not '{raw}'"
            ))
        })
}

fn page_number(raw: &str) -> ApiResult<u32> {
    raw.trim()
        .parse()
        .ok()
        .filter(|p| *p >= 1)
        .ok_or_else(|| ApiError::bad_request(format!("'page' must be a page number, not '{raw}'")))
}

fn require_image(image: &Document) -> ApiResult<()> {
    let png = image.bytes.starts_with(b"\x89PNG\r\n\x1a\n");
    let jpeg = image.bytes.starts_with(b"\xff\xd8\xff");
    if !(png || jpeg) {
        return Err(ApiError::bad_request(format!(
            "'{}' is not a PNG or JPEG image",
            image.name
        )));
    }
    Ok(())
}

/// The fields only a cryptographic signature has.
const CERTIFICATE_FIELDS: [&str; 7] = [
    "reason",
    "location",
    "contactInfo",
    "signerName",
    "level",
    "certify",
    "signingKey",
];

/// Sign a PDF
///
/// Signs a PDF. With `signature=certificate`, a cryptographic signature with
/// the key in `RUSTICDOC_SIGNING_KEY` and `RUSTICDOC_SIGNING_CERT`, or the
/// named key `signingKey` chooses: an incremental update with a PAdES
/// `/ETSI.CAdES.detached` signature at the `level` asked for (B-B to B-LTA),
/// approving or, with `certify`, certifying. It is invisible, shown with the
/// one image sent, or, with a position and no image, shown as text naming
/// the signer, the date and the reason. Without a configured key that answers
/// 503; a timestamp authority or revocation source that fails answers 502. By
/// default (`signature=image`) the images are drawn at exact coordinates,
/// which is not a cryptographic signature. `signature=renew` signs nothing:
/// it renews the archive timestamp of a file already signed at B-LTA, which
/// must end with a document timestamp from an authority this service trusts
/// and be unchanged since; anything else answers 400.
#[utoipa::path(
    post, path = "/v1/pdf/sign", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = SignForm),
    responses(
        (status = 200, description = "The signed PDF", content_type = "application/pdf", body = BinaryFile),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody),
        (status = 403, description = "The caller may not use the signing key, or may not renew", body = ErrorBody),
        (status = 502, description = "The timestamp authority or a revocation source failed", body = ErrorBody),
        (status = 503, description = "signature=certificate and no signing key is configured, the level or the renewal needs a timestamp authority that is not configured, or a certificate has expired or been revoked", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn sign(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let doc = one_pdf(&mut form)?;
    match form.field("signature").map(str::trim).unwrap_or("image") {
        "" | "image" => sign_with_images(state, hook, headers, form, doc).await,
        "certificate" => sign_with_certificate(state, principal, hook, headers, form, doc).await,
        "renew" => renew_timestamp(state, principal, hook, headers, form, doc).await,
        other => Err(ApiError::bad_request(format!(
            "'signature' must be image, certificate or renew, not '{other}'"
        ))),
    }
}

/// The fields that place a signature on a page.
const PLACEMENT_FIELDS: [&str; 4] = ["page", "x", "y", "width"];

/// `signature=renew`: validation data for the document timestamp the file
/// ends with, and a new document timestamp after it. No key is used.
async fn renew_timestamp(
    state: AppState,
    principal: Principal,
    hook: crate::jobs::Delivery,
    headers: HeaderMap,
    form: Form,
    doc: Document,
) -> ApiResult<Response> {
    // A renewal that quietly dropped a reason or an image would pass for a
    // signature it does not make.
    if let Some(name) = CERTIFICATE_FIELDS
        .iter()
        .chain(&PLACEMENT_FIELDS)
        .find(|n| form.field(n).is_some())
    {
        return Err(ApiError::bad_request(format!(
            "'{name}' belongs to a signature; signature=renew takes the PDF alone"
        )));
    }
    if !form.files.is_empty() {
        return Err(ApiError::bad_request(
            "with signature=renew, send the PDF alone",
        ));
    }
    let signing = &state.engines.signing;
    crate::signing::authorize_renewal(signing, &principal)?;
    signing.check_renewal()?;
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        let renewed = st
            .engines
            .signing
            .renew(&doc, st.config.api_timeout)
            .await?;
        Ok(pdf_output(name, renewed, "sign"))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// `signature=image`: each image drawn at its place, in one budget.
async fn sign_with_images(
    state: AppState,
    hook: crate::jobs::Delivery,
    headers: HeaderMap,
    mut form: Form,
    doc: Document,
) -> ApiResult<Response> {
    // Silently drawing an image for a request that asked for a reason would
    // pass for a signature it is not.
    if let Some(name) = CERTIFICATE_FIELDS.iter().find(|n| form.field(n).is_some()) {
        return Err(ApiError::bad_request(format!(
            "'{name}' belongs to a cryptographic signature; send signature=certificate"
        )));
    }
    let images = std::mem::take(&mut form.files);
    if images.is_empty() || images.len() > 50 {
        return Err(ApiError::bad_request(
            "send the PDF first, then one to 50 PNG or JPEG signatures",
        ));
    }
    let (pages, xs, ys, widths) = (
        form.all("page"),
        form.all("x"),
        form.all("y"),
        form.all("width"),
    );
    if pages.len() != images.len() || xs.len() != images.len() || ys.len() != images.len() {
        return Err(ApiError::bad_request(
            "each signature needs its own 'page', 'x' and 'y' fields",
        ));
    }
    let mut overlays = Vec::with_capacity(images.len());
    for (i, image) in images.into_iter().enumerate() {
        let page = page_number(&pages[i])?;
        let x = placement_number("x", &xs[i])?;
        let y = placement_number("y", &ys[i])?;
        let width = widths
            .get(i)
            .filter(|w| !w.trim().is_empty())
            .map(|w| placement_number("width", w))
            .transpose()?;
        require_image(&image)?;
        overlays.push(Overlay {
            placement: Some(rusticdoc_core::pdfops::Placement { x, y, width }),
            kind: OverlayKind::Image {
                bytes: image.bytes.to_vec(),
                name: image.name,
            },
            position: Position::Center,
            opacity: 1.0,
            rotation: 0.0,
            scale: 0.0,
            pages: page.to_string(),
        });
    }
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        // One budget for every placement, not one each.
        let budget = st.config.api_timeout;
        let work = async {
            let mut current = doc;
            for overlay in &overlays {
                current = st.engines.pdf.stamp(&current, overlay, budget).await?;
            }
            Ok::<_, rusticdoc_core::Error>(current)
        };
        let current = tokio::time::timeout(budget, work)
            .await
            .map_err(|_| rusticdoc_core::Error::Timeout(budget))??;
        Ok(pdf_output(name, current, "sign"))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// `signature=certificate`: one cryptographic signature with one of the
/// service's keys. The key never comes from the request.
async fn sign_with_certificate(
    state: AppState,
    principal: Principal,
    hook: crate::jobs::Delivery,
    headers: HeaderMap,
    mut form: Form,
    doc: Document,
) -> ApiResult<Response> {
    // Before anything else is checked: without a key nothing the client can
    // change makes this work.
    let signing = &state.engines.signing;
    if !signing.is_configured() {
        signing.key(None)?;
    }
    let mut options = sign::SignOptions::from_fields(&|name| form.field(name).map(String::from))?;
    crate::signing::authorize(signing, &principal, &options)?;
    let mut images = std::mem::take(&mut form.files);
    if images.len() > 1 {
        return Err(ApiError::bad_request(
            "with signature=certificate, send the PDF and at most one PNG or JPEG to show the signature with",
        ));
    }
    options.appearance = appearance(&form, images.pop())?;
    signing.check(&options)?;
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        let signed = st
            .engines
            .signing
            .sign(&doc, &options, st.config.api_timeout)
            .await?;
        Ok(pdf_output(name, signed, "sign"))
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Where a cryptographic signature is shown: with `image` at `page`, `x`,
/// `y` and `width`, as text at that place when there is no image, or nowhere
/// when the form names no place and sends no image.
pub fn appearance(form: &Form, image: Option<Document>) -> ApiResult<Option<sign::Appearance>> {
    let (pages, xs, ys, widths) = (
        form.all("page"),
        form.all("x"),
        form.all("y"),
        form.all("width"),
    );
    if image.is_none() && pages.is_empty() && xs.is_empty() && ys.is_empty() && widths.is_empty() {
        return Ok(None);
    }
    if xs.len() != 1 || ys.len() != 1 || pages.len() > 1 || widths.len() > 1 {
        return Err(ApiError::bad_request(
            "a visible signature needs one 'x' and one 'y', and at most one 'page' and 'width'",
        ));
    }
    if let Some(image) = &image {
        require_image(image)?;
    }
    Ok(Some(sign::Appearance {
        page: pages
            .first()
            .map(|p| page_number(p))
            .transpose()?
            .unwrap_or(1),
        x: placement_number("x", &xs[0])?,
        y: placement_number("y", &ys[0])?,
        width: widths
            .first()
            .filter(|w| !w.trim().is_empty())
            .map(|w| placement_number("width", w))
            .transpose()?,
        name: image
            .as_ref()
            .map_or_else(|| "text".to_string(), |i| i.name.clone()),
        image: image.map(|i| i.bytes.to_vec()),
    }))
}

/// Embed Factur-X XML
///
/// Embeds Factur-X invoice XML and marks the PDF for it.
#[utoipa::path(
    post, path = "/v1/pdf/factur-x", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = FacturxForm),
    responses((status = 200, description = "The Factur-X PDF", content_type = "application/pdf", body = BinaryFile), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn facturx(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let form = Form::read(multipart, &state).await?;
    let (pdf_pos, xml_pos) = {
        let pdfs: Vec<usize> = form
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.extension() == "pdf")
            .map(|(i, _)| i)
            .collect();
        let xmls: Vec<usize> = form
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.extension() == "xml")
            .map(|(i, _)| i)
            .collect();
        match (pdfs.as_slice(), xmls.as_slice()) {
            ([p], [x]) => (*p, *x),
            _ => {
                return Err(ApiError::bad_request(
                    "send exactly one PDF and one XML file",
                ));
            }
        }
    };
    let xml = form.files[xml_pos].bytes.to_vec();
    let doc = form.files[pdf_pos].clone();
    require_pdf(&doc)?;
    let profile = FacturxProfile::parse(form.field("profile").unwrap_or("EN16931"))?;
    let name = output_name(&form, &doc, "");
    let st = state.clone();
    let job = async move {
        Ok(pdf_output(
            name,
            st.engines
                .pdf
                .facturx(&doc, &xml, profile, st.config.api_timeout)
                .await?,
            "factur-x",
        ))
    };
    finish(state, hook, request_id(&headers), job).await
}
