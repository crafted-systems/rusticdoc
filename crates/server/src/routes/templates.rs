//! Stored templates: upload once, then render by id from any replica. A caller
//! sees only its own unless `RUSTICDOC_STORE_SCOPE=shared`.

use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use rusticdoc_core::config::StoreScope;
use rusticdoc_core::store::{self, Store, StoreError, TemplateMeta, TemplatePage};
use rusticdoc_core::{Document, template};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::routes::form::{Form, disposition, sanitize};
use crate::state::AppState;

/// The most templates one page lists.
const MAX_PAGE: usize = 200;
const DEFAULT_PAGE: usize = 50;

/// The store, or a clear answer that this service has none.
pub fn store_of(state: &AppState) -> ApiResult<Arc<Store>> {
    state.store.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "store_not_configured",
            "this service stores nothing until RUSTICDOC_STORE names a shared directory or bucket",
        )
    })
}

/// Reads as not found rather than as the store's failure, where the id is the
/// client's to get right.
pub fn user_store_error(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "there is no such object here",
        ),
        other => other.into(),
    }
}

/// Whose templates this caller works with.
fn scope(state: &AppState, principal: &Principal) -> String {
    match state.config.store_scope {
        StoreScope::Caller => store::scope_key(&principal.0),
        StoreScope::Shared => store::SHARED_SCOPE.to_string(),
    }
}

/// The stored template a form's `templateId` names, if it names one. A form that
/// also has a `template` file is asking for two different things.
pub async fn named_in(
    form: &mut Form,
    state: &AppState,
    principal: &Principal,
) -> ApiResult<Option<Document>> {
    let Some(id) = form
        .field("templateId")
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    if form.by_field.contains_key("template") {
        return Err(ApiError::bad_request(
            "send a 'template' file or a 'templateId', not both",
        ));
    }
    let store = store_of(state)?;
    let document = store
        .template_content(&scope(state, principal), id)
        .await
        .map_err(user_store_error)?;
    Ok(Some(document))
}

/// The multipart form `POST /v1/templates` accepts.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct TemplateUploadForm {
    /// The template: DOCX, XLSX, PPTX, ODT, ODS, ODP, PDF or a text format.
    #[schema(value_type = String, format = Binary)]
    pub template: String,
    /// A name for it, in place of the file's. The extension stays the file's.
    pub name: Option<String>,
}

/// A stored template.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct TemplateSchema {
    /// Pass as `templateId` to `/v1/render`, `/v1/render/batch` or `/v1/templates/validate`.
    #[schema(example = "01966f3e-7c1a-7d3c-9a44-1d2f5a6b7c8d")]
    pub id: String,
    #[schema(example = "invoice.docx")]
    pub name: String,
    #[schema(example = "docx")]
    pub format: String,
    pub size: u64,
    /// Hex SHA-256 of the content.
    pub sha256: String,
    /// RFC 3339.
    pub created_at: String,
}

/// One page of templates.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct TemplateList {
    pub templates: Vec<TemplateSchema>,
    /// Pass as `cursor` for the next page. Absent on the last.
    pub next_cursor: Option<String>,
}

#[derive(Deserialize, IntoParams)]
pub struct ListParams {
    /// How many to return, up to 200. Default 50.
    limit: Option<usize>,
    /// `nextCursor` from the page before.
    cursor: Option<String>,
}

/// Why a file cannot be a template, before anything is stored.
async fn check(state: &AppState, doc: &Document) -> ApiResult<()> {
    let format = doc.extension();
    let known = template::is_office_format(&format)
        || template::is_text_template(&format)
        || format == "pdf";
    if !known {
        return Err(ApiError::bad_request(format!(
            "'{format}' is not a template format; use {}, {} or pdf",
            template::OFFICE_FORMATS.join(", "),
            template::TEXT_FORMATS.join(", "),
        )));
    }
    if format == "pdf" {
        rusticdoc_core::pdfops::require_pdf(doc)?;
        return Ok(());
    }
    let found = state
        .pipeline
        .renderer()
        .check_syntax(doc, state.config.api_timeout)
        .await?;
    let errors: Vec<String> = found
        .iter()
        .filter(|d| d.severity == rusticdoc_template::Severity::Error)
        .take(5)
        .map(|d| d.message.clone())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "template_error",
            errors.join("; "),
        ))
    }
}

/// Stores a template and returns its id. Syntax is checked now, without data, so a
/// broken template is refused here rather than at its first render. The content
/// never changes: to change a template, store the new one and use its id.
#[utoipa::path(
    post, path = "/v1/templates", tag = "templates", operation_id = "uploadTemplate",
    request_body(content_type = "multipart/form-data", content = TemplateUploadForm),
    responses(
        (status = 201, description = "Stored", body = TemplateSchema),
        (status = 400, description = "Not a template format", body = ErrorBody),
        (status = 403, description = "This caller's quota is used up", body = ErrorBody),
        (status = 413, description = "Too large", body = ErrorBody),
        (status = 422, description = "The template has syntax errors", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn upload(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    multipart: Multipart,
) -> ApiResult<Response> {
    let store = store_of(&state)?;
    let mut form = Form::read(multipart, &state).await?;
    let doc = match form.files.len() {
        1 => form.files.remove(0),
        0 => {
            return Err(ApiError::bad_request(
                "no file in the form; send the template",
            ));
        }
        _ => return Err(ApiError::bad_request("send one template at a time")),
    };
    if doc.bytes.len() > state.config.max_template_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "template_too_large",
            format!(
                "the template is {} bytes; the limit is {}",
                doc.bytes.len(),
                state.config.max_template_bytes
            ),
        ));
    }
    // A chosen name changes what it is called, never what format it is.
    let doc = match form.field("name").map(sanitize).filter(|n| !n.is_empty()) {
        Some(name) if !doc.extension().is_empty() => {
            let stem = name
                .strip_suffix(&format!(".{}", doc.extension()))
                .unwrap_or(&name);
            Document::new(format!("{stem}.{}", doc.extension()), doc.bytes.clone())
        }
        _ => doc,
    };
    check(&state, &doc).await?;

    let scope = scope(&state, &principal);
    let usage = store.template_usage(&scope).await?;
    if usage.count >= state.config.max_templates
        || usage.bytes + doc.bytes.len() as u64 > state.config.max_template_storage
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "quota_exceeded",
            format!(
                "this caller holds {} templates of {} bytes; the limits are {} and {}",
                usage.count,
                usage.bytes,
                state.config.max_templates,
                state.config.max_template_storage
            ),
        ));
    }
    let meta = store.put_template(&scope, &doc).await?;
    tracing::info!(
        log_type = "audit", action = "template.upload", caller = %principal.short(),
        template = %meta.id, size = meta.size, "template stored"
    );
    let root = state.config.root_path.trim_end_matches('/');
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, format!("{root}/v1/templates/{}", meta.id))],
        Json(meta),
    )
        .into_response())
}

/// Lists this caller's templates, oldest first.
#[utoipa::path(
    get, path = "/v1/templates", tag = "templates", operation_id = "listTemplates",
    params(ListParams),
    responses(
        (status = 200, description = "A page of templates", body = TemplateList),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<TemplatePage>> {
    let store = store_of(&state)?;
    let limit = params.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let page = store
        .list_templates(&scope(&state, &principal), limit, params.cursor.as_deref())
        .await?;
    Ok(Json(page))
}

/// One template's details.
#[utoipa::path(
    get, path = "/v1/templates/{id}", tag = "templates", operation_id = "getTemplate",
    params(("id" = String, Path, description = "The template's id")),
    responses(
        (status = 200, description = "The template", body = TemplateSchema),
        (status = 404, description = "This caller has no such template", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn show(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<TemplateMeta>> {
    let store = store_of(&state)?;
    let meta = store
        .template_meta(&scope(&state, &principal), &id)
        .await
        .map_err(user_store_error)?;
    Ok(Json(meta))
}

/// Downloads a template as it was uploaded.
#[utoipa::path(
    get, path = "/v1/templates/{id}/content", tag = "templates", operation_id = "downloadTemplate",
    params(("id" = String, Path, description = "The template's id")),
    responses(
        (status = 200, description = "The file", content_type = "application/octet-stream", body = crate::error::BinaryFile),
        (status = 404, description = "This caller has no such template", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn content(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let store = store_of(&state)?;
    let doc = store
        .template_content(&scope(&state, &principal), &id)
        .await
        .map_err(user_store_error)?;
    // Always a download: a stored HTML template must not run on this origin.
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_DISPOSITION, disposition(&doc.name)),
        ],
        doc.bytes,
    )
        .into_response())
}

/// Deletes a template. Every replica stops serving it at once.
#[utoipa::path(
    delete, path = "/v1/templates/{id}", tag = "templates", operation_id = "deleteTemplate",
    params(("id" = String, Path, description = "The template's id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "This caller has no such template", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn remove(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let store = store_of(&state)?;
    store
        .delete_template(&scope(&state, &principal), &id)
        .await
        .map_err(user_store_error)?;
    tracing::info!(
        log_type = "audit", action = "template.delete", caller = %principal.short(),
        template = %id, "template deleted"
    );
    Ok(StatusCode::NO_CONTENT)
}
