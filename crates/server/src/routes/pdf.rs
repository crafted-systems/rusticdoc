use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::Response;
use rusticdoc_core::{RenderRequest, Step};
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::routes::form::{Form, request_id};
use crate::routes::render;
use crate::state::AppState;
use crate::webhook::Webhook;

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct MergeForm {
    /// Two or more PDFs, merged in order.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Vec<String>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct EncryptForm {
    /// One PDF.
    #[schema(value_type = String, format = Binary)]
    pub files: String,
    #[schema(rename = "userPassword")]
    pub user_password: Option<String>,
    #[schema(rename = "ownerPassword")]
    pub owner_password: String,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

/// Merges PDFs in the order sent.
#[utoipa::path(
    post, path = "/v1/pdf/merge", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = MergeForm),
    responses(
        (status = 200, description = "The merged PDF", content_type = "application/pdf", body = Vec<u8>),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 503, description = "An engine is unavailable or the request timed out", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn merge(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let hook = Webhook::from_headers(&headers, state.config.webhook_allow_private).await?;
    let mut form = Form::read(multipart).await?;
    if form.files.len() < 2 {
        return Err(ApiError::bad_request("merge needs at least two PDF files"));
    }
    let mut files = std::mem::take(&mut form.files);
    let mut req = RenderRequest::new(files.remove(0));
    req.templating = false;
    req.extra = files;
    req.steps = vec![Step::Merge];
    req.output_name = form
        .field("outputFilename")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    render::run(state, hook, request_id(&headers), req).await
}

/// Encrypts a PDF with AES-256.
#[utoipa::path(
    post, path = "/v1/pdf/encrypt", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = EncryptForm),
    responses(
        (status = 200, description = "The encrypted PDF", content_type = "application/pdf", body = Vec<u8>),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 503, description = "An engine is unavailable or the request timed out", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn encrypt(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let hook = Webhook::from_headers(&headers, state.config.webhook_allow_private).await?;
    let mut form = Form::read(multipart).await?;
    let file = form.require_file("one PDF")?;
    let owner_password = form
        .field("ownerPassword")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("'ownerPassword' is required"))?
        .to_string();
    let mut req = RenderRequest::new(file);
    req.templating = false;
    req.steps = vec![Step::Encrypt {
        user_password: form.field("userPassword").unwrap_or("").to_string(),
        owner_password,
    }];
    req.output_name = form
        .field("outputFilename")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    render::run(state, hook, request_id(&headers), req).await
}
