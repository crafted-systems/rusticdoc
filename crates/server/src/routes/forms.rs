//! PDF templates: list fields, fill a form, lay a document over a base PDF.

use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::Response;
use rusticdoc_core::{Document, Output, pdfform};
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult, BinaryFile, ErrorBody};
use crate::routes::form::{Form, download, request_id};
use crate::state::AppState;
use crate::webhook::{self, Webhook};

/// One form field, as `/v1/pdf/forms/fields` lists them.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct FieldSchema {
    /// Fully qualified name, such as `customer.name`.
    pub name: String,
    /// `text`, `checkbox`, `radio`, `choice`, `signature` or `button`.
    pub kind: String,
    /// 1-based page of the first widget.
    pub page: u32,
    pub value: Option<String>,
    pub options: Vec<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct FillForm {
    /// The PDF with form fields, and optionally tags in annotations.
    #[schema(value_type = String, format = Binary)]
    pub files: String,
    /// JSON object; a field named `customer.name` takes that path.
    pub data: Option<serde_json::Value>,
    pub ctx: Option<serde_json::Value>,
    /// Fix the values into the page content.
    pub flatten: Option<bool>,
    /// Remove the annotations that carried tags. Default true.
    #[schema(rename = "stripAnnotations")]
    pub strip_annotations: Option<bool>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct OverlayForm {
    /// Two PDFs: the base, then the overlay laid over it page by page.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Vec<String>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

/// Lists a PDF's form fields with their kinds, pages and values.
#[utoipa::path(
    post, path = "/v1/pdf/forms/fields", tag = "pdf-forms",
    request_body(content_type = "multipart/form-data", content = OverlayForm),
    responses((status = 200, description = "The form fields", body = Vec<FieldSchema>), (status = 400, description = "The request is wrong", body = ErrorBody), (status = 501, description = "Not supported in this version", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn fields(
    State(state): State<AppState>,
    multipart: Multipart,
) -> ApiResult<Json<Vec<pdfform::FieldInfo>>> {
    let mut form = Form::read(multipart, &state).await?;
    let file = form.require_file("one PDF")?;
    rusticdoc_core::pdfops::require_pdf(&file)?;
    let fields = tokio::task::spawn_blocking(move || pdfform::fields(&file.bytes))
        .await
        .map_err(|e| {
            ApiError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "io_error",
                e.to_string(),
            )
        })??;
    Ok(Json(fields))
}

/// Fills a PDF form from data, applying tags in annotations, optionally
/// flattening the result.
#[utoipa::path(
    post, path = "/v1/pdf/forms/fill", tag = "pdf-forms",
    request_body(content_type = "multipart/form-data", content = FillForm),
    responses(
        (status = 200, description = "The filled PDF", content_type = "application/pdf", body = BinaryFile),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 422, description = "The template failed to render", body = ErrorBody), (status = 501, description = "Not supported in this version", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn fill(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let hook = Webhook::from_headers(&headers, state.config.webhook_allow_private).await?;
    let mut form = Form::read(multipart, &state).await?;
    let file = form.require_file("one PDF")?;
    rusticdoc_core::pdfops::require_pdf(&file)?;
    let mut data = form.json("data")?;
    let ctx = form.json("ctx")?;
    if let serde_json::Value::Object(map) = &mut data {
        map.entry("ctx").or_insert(ctx);
    } else {
        return Err(ApiError::bad_request("'data' must be a JSON object"));
    }
    let options = pdfform::FillOptions {
        flatten: form.bool("flatten"),
        strip_tag_annotations: form
            .field("stripAnnotations")
            .map(|v| v != "false")
            .unwrap_or(true),
    };
    let name = form
        .field("outputFilename")
        .map(|s| format!("{}.pdf", s.trim()))
        .filter(|s| s != ".pdf")
        .unwrap_or_else(|| file.name.clone());

    let job = async move {
        let bytes = tokio::task::spawn_blocking(move || {
            let env = rusticdoc_core::template::environment();
            pdfform::fill(&file.bytes, &data, &env, &options)
        })
        .await
        .map_err(|e| {
            ApiError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "io_error",
                e.to_string(),
            )
        })??;
        Ok::<Output, ApiError>(Output {
            name,
            content_type: "application/pdf",
            bytes: bytes.into(),
            timings: vec![("fill".into(), 0)],
        })
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Lays the second PDF over the first, page by page.
#[utoipa::path(
    post, path = "/v1/pdf/overlay", tag = "pdf-forms",
    request_body(content_type = "multipart/form-data", content = OverlayForm),
    responses(
        (status = 200, description = "The combined PDF", content_type = "application/pdf", body = BinaryFile),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 501, description = "Not supported in this version", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn overlay(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let hook = Webhook::from_headers(&headers, state.config.webhook_allow_private).await?;
    let mut form = Form::read(multipart, &state).await?;
    if form.files.len() != 2 {
        return Err(ApiError::bad_request(
            "overlay needs exactly two PDFs: the base, then the overlay",
        ));
    }
    let base: Document = form.files.remove(0);
    let over: Document = form.files.remove(0);
    rusticdoc_core::pdfops::require_pdf(&base)?;
    rusticdoc_core::pdfops::require_pdf(&over)?;
    let name = form
        .field("outputFilename")
        .map(|s| format!("{}.pdf", s.trim()))
        .filter(|s| s != ".pdf")
        .unwrap_or_else(|| base.name.clone());
    let job = async move {
        let bytes = tokio::task::spawn_blocking(move || pdfform::overlay(&base.bytes, &over.bytes))
            .await
            .map_err(|e| {
                ApiError::new(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "io_error",
                    e.to_string(),
                )
            })??;
        Ok::<Output, ApiError>(Output {
            name,
            content_type: "application/pdf",
            bytes: bytes.into(),
            timings: vec![("overlay".into(), 0)],
        })
    };
    finish(state, hook, request_id(&headers), job).await
}

/// Returns the output now, or hands it to a webhook task.
pub async fn finish<F>(
    state: AppState,
    hook: Option<Webhook>,
    request_id: String,
    job: F,
) -> ApiResult<Response>
where
    F: std::future::Future<Output = ApiResult<Output>> + Send + 'static,
{
    match hook {
        None => {
            let out = job.await?;
            state.metrics.record_timings(&out.timings);
            Ok(download(out))
        }
        Some(hook) => {
            let rid = request_id.clone();
            tokio::spawn(async move {
                let result = job.await;
                webhook::deliver(state, hook, rid, result).await;
            });
            Ok(webhook::accepted(&request_id))
        }
    }
}
