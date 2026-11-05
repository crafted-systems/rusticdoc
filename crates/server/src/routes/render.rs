use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::Response;
use rusticdoc_core::job::parse_steps;
use rusticdoc_core::{RenderRequest, template};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::metrics::StageLabels;
use crate::routes::form::{Form, download, request_id};
use crate::state::AppState;
use crate::webhook::{self, Webhook};

/// The multipart form `/v1/render` accepts. Documented here; parsed by hand.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct RenderForm {
    /// The template. Text formats are filled today; Office formats are
    /// converted as they are until phase 1.
    #[schema(value_type = String, format = Binary)]
    pub template: String,
    /// JSON object the template sees as its root.
    #[schema(example = json!({"customer": {"name": "Ada"}}))]
    pub data: Option<serde_json::Value>,
    /// JSON object visible as `ctx`.
    pub ctx: Option<serde_json::Value>,
    /// Comma-separated steps in order: `convert`, `pdfa`, `metadata`,
    /// `encrypt`, `merge`.
    #[schema(example = "convert,pdfa,encrypt")]
    pub steps: Option<String>,
    /// Output format for `convert`. Default `pdf`.
    pub to: Option<String>,
    /// PDF/A level for `pdfa`: `PDF/A-1b`, `PDF/A-2b`, `PDF/A-3b`.
    pub pdfa: Option<String>,
    /// JSON object of metadata fields for `metadata`.
    pub metadata: Option<serde_json::Value>,
    pub user_password: Option<String>,
    pub owner_password: Option<String>,
    /// File name of the result, without extension.
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
    /// Further files, used by `merge`.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Option<Vec<String>>,
}

/// Fills a template and runs the steps.
///
/// Add `X-Rusticdoc-Webhook-Url` to get 204 at once and the result by
/// webhook, signed with `X-Rusticdoc-Signature` when a secret is configured.
#[utoipa::path(
    post, path = "/v1/render", tag = "render",
    request_body(content_type = "multipart/form-data", content = RenderForm),
    params(
        ("X-Rusticdoc-Webhook-Url" = Option<String>, Header, description = "Deliver the result here instead of in the response"),
        ("X-Rusticdoc-Webhook-Error-Url" = Option<String>, Header, description = "Deliver failures here as JSON"),
    ),
    responses(
        (status = 200, description = "The rendered file", content_type = "application/octet-stream"),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody), (status = 422, body = ErrorBody),
        (status = 501, body = ErrorBody), (status = 503, body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn render(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let hook = Webhook::from_headers(&headers, state.config.webhook_allow_private).await?;
    let mut form = Form::read(multipart).await?;
    let template = match form
        .files
        .iter()
        .position(|f| f.name != "data.json" && f.name != "ctx.json")
    {
        Some(pos) => form.files.remove(pos),
        None => {
            return Err(ApiError::bad_request(
                "no template in the form; send it as a file part",
            ));
        }
    };
    let mut req = RenderRequest::new(template);
    req.data = form.json("data")?;
    req.ctx = form.json("ctx")?;
    req.steps = parse_steps(form.field("steps").unwrap_or(""), &form.fields)?;
    req.output_name = form
        .field("outputFilename")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    req.extra = std::mem::take(&mut form.files);

    run(state, hook, request_id(&headers), req).await
}

/// Runs a request now, or hands it to a webhook task.
pub async fn run(
    state: AppState,
    hook: Option<Webhook>,
    request_id: String,
    req: RenderRequest,
) -> ApiResult<Response> {
    match hook {
        None => Ok(download(execute(&state, req).await?)),
        Some(hook) => {
            let st = state.clone();
            let rid = request_id.clone();
            tokio::spawn(async move {
                let result = execute(&st, req).await;
                webhook::deliver(st, hook, rid, result).await;
            });
            Ok(webhook::accepted(&request_id))
        }
    }
}

async fn execute(state: &AppState, req: RenderRequest) -> ApiResult<rusticdoc_core::Output> {
    let gauge = state
        .metrics
        .inflight
        .get_or_create(&StageLabels {
            stage: "pipeline".into(),
        })
        .clone();
    gauge.inc();
    let result = tokio::time::timeout(state.config.api_timeout, state.pipeline.run(req)).await;
    gauge.dec();
    let out = match result {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(rusticdoc_core::Error::Timeout(state.config.api_timeout).into()),
    };
    state.metrics.record_timings(&out.timings);
    Ok(out)
}

#[derive(Serialize, ToSchema)]
pub struct Validation {
    /// True when the template renders with the sample data.
    pub ok: bool,
    /// SHA-256 of the template, the cache key.
    pub template_hash: String,
    pub format: String,
    /// The engine that would handle it.
    pub engine: &'static str,
    /// Present when `ok` is false.
    pub error: Option<String>,
}

/// Checks a template against sample data without converting anything.
#[utoipa::path(
    post, path = "/v1/templates/validate", tag = "render",
    request_body(content_type = "multipart/form-data", content = RenderForm),
    responses((status = 200, body = Validation), (status = 400, body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn validate(
    State(_state): State<AppState>,
    multipart: Multipart,
) -> ApiResult<Json<Validation>> {
    let mut form = Form::read(multipart).await?;
    let template = form.require_file("a template file")?;
    let data = form.json("data")?;
    let ctx = form.json("ctx")?;
    let format = template.extension();
    let engine = if template::is_text_template(&format) {
        "text"
    } else if template::is_office_format(&format) {
        "office (phase 1)"
    } else {
        "none"
    };
    let error = template::render(&template, &data, &ctx)
        .err()
        .map(|e| e.to_string());
    Ok(Json(Validation {
        ok: error.is_none(),
        template_hash: template::template_hash(&template.bytes),
        format,
        engine,
        error,
    }))
}
