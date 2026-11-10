//! Stored templates: upload once, then render by id from any replica. A caller
//! sees only its own unless `RUSTICDOC_STORE_SCOPE=shared`.

use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, Request, State};
use axum::http::{HeaderName, StatusCode, header};
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
pub fn scope(state: &AppState, principal: &Principal) -> String {
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
    let (_, document) = resolve(&store, &scope(state, principal), id).await?;
    Ok(Some(document))
}

/// The template id `reference` names: an id as it is, or an alias, as it is now
/// or as `name@version` was. A bad version is the client's to fix.
pub async fn resolve_id(store: &Store, scope: &str, reference: &str) -> ApiResult<String> {
    let reference = reference.trim();
    if store::is_template_id(reference) {
        return Ok(reference.to_string());
    }
    let (name, version) = match reference.rsplit_once('@') {
        Some((name, version)) => match version.parse::<usize>() {
            Ok(v) if v > 0 => (name, Some(v)),
            _ => {
                return Err(ApiError::bad_request(format!(
                    "'{version}' is not a version; use name@1, name@2 and so on"
                )));
            }
        },
        None => (reference, None),
    };
    if !store::is_alias_name(name) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "that is neither a template id nor an alias",
        ));
    }
    match store.resolve_alias(scope, name, version).await {
        Ok(id) => Ok(id),
        Err(StoreError::NotFound) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            match version {
                Some(v) => format!("there is no alias '{name}' with a version {v} here"),
                None => format!("there is no alias '{name}' here"),
            },
        )),
        Err(e) => Err(e.into()),
    }
}

/// The template `reference` names, by id or alias, with its id. An alias whose
/// template was deleted since says so.
pub async fn resolve(store: &Store, scope: &str, reference: &str) -> ApiResult<(String, Document)> {
    let id = resolve_id(store, scope, reference).await?;
    match store.template_content(scope, &id).await {
        Ok(doc) => Ok((id, doc)),
        Err(StoreError::NotFound) if !store::is_template_id(reference.trim()) => {
            Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!(
                    "'{}' names template {id}, which has been deleted",
                    reference.trim()
                ),
            ))
        }
        Err(e) => Err(user_store_error(e)),
    }
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
    /// Pass as `templateId` to `/v1/render`, `/v1/render/batch` or `/v1/templates/validate`,
    /// or point an alias at it with `PUT /v1/aliases/{name}`.
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
    pub limit: Option<usize>,
    /// `nextCursor` from the page before.
    pub cursor: Option<String>,
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

/// Upload a template
///
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

/// List templates
///
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

/// Get a template
///
/// One template's details.
#[utoipa::path(
    get, path = "/v1/templates/{id}", tag = "templates", operation_id = "getTemplate",
    params(("id" = String, Path, description = "The template's id, or an alias as `name` or `name@version`")),
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
    let scope = scope(&state, &principal);
    let id = resolve_id(&store, &scope, &id).await?;
    let meta = store
        .template_meta(&scope, &id)
        .await
        .map_err(user_store_error)?;
    Ok(Json(meta))
}

/// Download a template
///
/// Downloads a template as it was uploaded.
#[utoipa::path(
    get, path = "/v1/templates/{id}/content", tag = "templates", operation_id = "downloadTemplate",
    params(("id" = String, Path, description = "The template's id, or an alias as `name` or `name@version`")),
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
    let (_, doc) = resolve(&store, &scope(&state, &principal), &id).await?;
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

/// Delete a template
///
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

/// What a stored template reads from its data.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TemplateVariables {
    /// The template's id, also when it was asked for by alias.
    pub template_id: String,
    #[schema(example = "docx")]
    pub format: String,
    /// Every path the template reads, parents first: `customer.name`, `items`
    /// (an array), `items[].price`. For a PDF form, its fields' names and the
    /// tags in their values; tags in annotations are not listed.
    pub variables: Vec<crate::variables::Variable>,
    /// Data of the right shape, with empty strings where values go.
    #[schema(value_type = Object)]
    pub example: serde_json::Value,
}

/// List a template's variables
///
/// Lists the data a template reads: each path, which paths are arrays it loops
/// over, and the fields it reads from their items, with a skeleton of data.
/// Read from the template's tags, so a path built at render time, such as
/// `row[column]`, is listed only as far as it is written.
#[utoipa::path(
    get, path = "/v1/templates/{id}/variables", tag = "templates", operation_id = "templateVariables",
    params(("id" = String, Path, description = "The template's id, or an alias as `name` or `name@version`")),
    responses(
        (status = 200, description = "What the template reads", body = TemplateVariables),
        (status = 404, description = "This caller has no such template", body = ErrorBody),
        (status = 422, description = "The template has syntax errors", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn variables(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<TemplateVariables>> {
    let store = store_of(&state)?;
    let (id, doc) = resolve(&store, &scope(&state, &principal), &id).await?;
    let format = doc.extension();
    let found = if format == "pdf" {
        rusticdoc_core::pdfops::require_pdf(&doc)?;
        let bytes = doc.bytes.clone();
        let fields = rusticdoc_core::pdfops::blocking(state.config.api_timeout, move |_| {
            rusticdoc_core::pdfform::fields(&bytes)
        })
        .await?;
        let fields: Vec<(String, Option<String>)> =
            fields.into_iter().map(|f| (f.name, f.value)).collect();
        crate::variables::analyse_form(&fields)
    } else {
        let tags = state
            .pipeline
            .renderer()
            .tags(&doc, state.config.api_timeout)
            .await?;
        crate::variables::analyse(&tags)
    };
    Ok(Json(TemplateVariables {
        template_id: id,
        format,
        variables: found.variables,
        example: found.example,
    }))
}

/// The fields `POST /v1/templates/{id}/preview` accepts, as JSON or as a
/// multipart form.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct PreviewForm {
    /// Sample data the template sees as its root.
    #[schema(example = json!({"customer": {"name": "Ada"}}))]
    pub data: Option<serde_json::Value>,
    /// Visible as `ctx`.
    pub ctx: Option<serde_json::Value>,
    /// The page to draw, from 1. Default 1.
    pub page: Option<u32>,
    /// The image's width in pixels, from 16 to 2000; the height follows the
    /// page. Default 800. Send this or `dpi`.
    pub width: Option<u32>,
    /// Draw at this resolution, from 18 to 300, as large as the page is. An
    /// image may not pass 4000 pixels on a side.
    pub dpi: Option<u32>,
    /// What turns HTML into PDF: `fulgur` (default) or `libreoffice`.
    #[schema(rename = "htmlEngine")]
    pub html_engine: Option<String>,
}

/// Preview a template
///
/// Renders a stored template with sample data and draws one page as a PNG,
/// for checking a template without opening the document. Office templates
/// convert with LibreOffice, HTML with its HTML engine, and PDF forms are filled;
/// poppler's `pdftoppm` draws the page. The page count is in `X-Page-Count`.
#[utoipa::path(
    post, path = "/v1/templates/{id}/preview", tag = "templates", operation_id = "previewTemplate",
    params(("id" = String, Path, description = "The template's id, or an alias as `name` or `name@version`")),
    request_body(content(
        (PreviewForm = "application/json"),
        (PreviewForm = "multipart/form-data"),
    )),
    responses(
        (status = 200, description = "The page as a PNG", content_type = "image/png", body = crate::error::BinaryFile,
            headers(("X-Page-Count" = u32, description = "How many pages the rendered document has"))),
        (status = 400, description = "The request is wrong, or the page does not exist", body = ErrorBody),
        (status = 404, description = "This caller has no such template", body = ErrorBody),
        (status = 422, description = "The template failed to render", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "pdftoppm or LibreOffice is unavailable, or the request timed out", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn preview(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let store = store_of(&state)?;
    let bin = state.config.pdftoppm_bin.clone();
    // Before any work: without the drawing engine none of it is any use.
    if bin.is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "engine_unavailable",
            "previews need poppler's pdftoppm, which this service does not have; set RUSTICDOC_PDFTOPPM_BIN",
        ));
    }
    let timeout = state.config.api_timeout;
    let started = std::time::Instant::now();
    let mut form = Form::from_request(req, &state).await?;
    let data = form.json("data")?;
    let ctx = form.json("ctx")?;
    let render_choices = crate::routes::render::render_options(&mut form, &state)?;
    let page = match form.field("page").map(str::trim).filter(|p| !p.is_empty()) {
        None => 1,
        Some(text) => match text.parse::<u32>() {
            Ok(n) if (1..=crate::preview::MAX_PAGE).contains(&n) => n,
            _ => {
                return Err(ApiError::bad_request(format!(
                    "'page' is a page number from 1 to {}",
                    crate::preview::MAX_PAGE
                )));
            }
        },
    };
    let size = crate::preview::Size::parse(form.field("width"), form.field("dpi"))?;
    let (_, template) = resolve(&store, &scope(&state, &principal), &id).await?;

    let pdf = if template.extension() == "pdf" {
        rusticdoc_core::pdfops::require_pdf(&template)?;
        if data.as_object().is_none_or(|m| m.is_empty()) {
            template.bytes.clone()
        } else {
            let mut data = data;
            if let serde_json::Value::Object(map) = &mut data {
                map.entry("ctx").or_insert(ctx);
            }
            let options = rusticdoc_core::pdfform::FillOptions {
                flatten: false,
                strip_tag_annotations: true,
            };
            state
                .pipeline
                .renderer()
                .fill(
                    &template.bytes,
                    &data,
                    &options,
                    state.config.max_render_bytes,
                    timeout,
                    &render_choices,
                )
                .await?
                .into()
        }
    } else {
        let mut req = rusticdoc_core::RenderRequest::new(template);
        req.data = data;
        req.ctx = ctx;
        req.render_options = render_choices;
        let mut options = crate::routes::options::convert_options(&form)?;
        options.pdfa = None;
        options.pdfua = false;
        req.convert_options = options;
        req.steps = vec![rusticdoc_core::Step::Convert {
            to: rusticdoc_core::Format::parse("pdf")?,
        }];
        state.engines.check_steps(&req.steps)?;
        crate::routes::render::execute(&state, req).await?.bytes
    };
    let left = timeout.saturating_sub(started.elapsed());
    let drawn = crate::preview::draw(bin.as_deref(), pdf, page, size, left).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png".to_string()),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=\"preview.png\"".to_string(),
            ),
            (
                HeaderName::from_static("x-page-count"),
                drawn.pages.to_string(),
            ),
        ],
        drawn.png,
    )
        .into_response())
}
