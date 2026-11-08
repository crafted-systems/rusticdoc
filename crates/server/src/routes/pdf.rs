use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::Response;
use rusticdoc_core::{RenderRequest, Step};
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult, BinaryFile, ErrorBody};
use crate::routes::form::{Form, request_id};
use crate::routes::options;
use crate::routes::render;
use crate::state::AppState;
use crate::webhook::Hook;

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
        (status = 200, description = "The merged PDF", content_type = "application/pdf", body = BinaryFile),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 503, description = "An engine is unavailable or the request timed out", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn merge(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    if form.files.is_empty() {
        return Err(ApiError::bad_request("merge needs at least one PDF file"));
    }
    for doc in &form.files {
        rusticdoc_core::pdfops::require_pdf(doc)?;
    }
    let options = options::convert_options(&form)?;
    let post = options::post_steps(&mut form, false)?;
    let mut files = std::mem::take(&mut form.files);
    let mut req = RenderRequest::new(files.remove(0));
    req.templating = false;
    req.convert_options = options;
    req.extra = files;
    req.steps = vec![Step::Merge];
    req.steps.extend(post);
    req.output_name = form.output_name();
    render::run(state, hook, request_id(&headers), req).await
}

/// Encrypts a PDF with AES-256.
#[utoipa::path(
    post, path = "/v1/pdf/encrypt", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = EncryptForm),
    responses(
        (status = 200, description = "The encrypted PDF", content_type = "application/pdf", body = BinaryFile),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 503, description = "An engine is unavailable or the request timed out", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn encrypt(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    let file = form.require_file("one PDF")?;
    // Either password is enough; the owner password defaults to the
    // user password when only that one is given.
    let user_password = form.field("userPassword").unwrap_or("").to_string();
    let owner_password = form
        .field("ownerPassword")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| user_password.clone());
    if owner_password.is_empty() {
        return Err(ApiError::bad_request(
            "'userPassword' or 'ownerPassword' is required",
        ));
    }
    let mut req = RenderRequest::new(file);
    req.templating = false;
    req.steps = vec![Step::Encrypt {
        user_password,
        owner_password,
    }];
    req.output_name = form.output_name();
    render::run(state, hook, request_id(&headers), req).await
}
