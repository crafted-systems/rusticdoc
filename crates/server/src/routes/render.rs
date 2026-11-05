use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::Response;
use rusticdoc_core::job::parse_steps;
use rusticdoc_core::{RenderRequest, template};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult, BinaryFile, ErrorBody};
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
        (status = 200, description = "The rendered file", content_type = "application/octet-stream", body = BinaryFile),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 401, description = "Credentials required", body = ErrorBody), (status = 422, description = "The template failed to render", body = ErrorBody),
        (status = 501, description = "Not supported in this version", body = ErrorBody), (status = 503, description = "An engine is unavailable or the request timed out", body = ErrorBody),
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

/// Renders the template once per element of an array in the data and
/// returns the files in a ZIP, as Carbone's batch rendering does.
///
/// `splitBy` names the array with a dotted path such as `items` or
/// `order.lines`. Each render sees the whole data with that array replaced
/// by the one element, plus `item` (the element) and `batch` (`index`,
/// `count`). `batchName` is a template for each file's name, default
/// `{{ batch.index }}`.
#[utoipa::path(
    post, path = "/v1/render/batch", tag = "render",
    request_body(content_type = "multipart/form-data", content = RenderForm),
    params(
        ("X-Rusticdoc-Webhook-Url" = Option<String>, Header, description = "Deliver the ZIP here instead of in the response"),
    ),
    responses(
        (status = 200, description = "A ZIP of the rendered files", content_type = "application/zip", body = BinaryFile),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 422, description = "The template failed to render", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn batch(
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
    let data = form.json("data")?;
    let ctx = form.json("ctx")?;
    let steps = parse_steps(form.field("steps").unwrap_or(""), &form.fields)?;
    let split_by = form
        .field("splitBy")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("'splitBy' names the array to split on"))?
        .to_string();
    let name_template = form
        .field("batchName")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("{{ batch.index }}")
        .to_string();
    let zip_name = form
        .field("outputFilename")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::path::Path::new(&template.name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("batch")
                .to_string()
        });

    let elements = match lookup(&data, &split_by) {
        Some(serde_json::Value::Array(items)) => items.clone(),
        Some(_) => {
            return Err(ApiError::bad_request(format!(
                "'{split_by}' is not an array in the data"
            )));
        }
        None => {
            return Err(ApiError::bad_request(format!(
                "'{split_by}' is not in the data"
            )));
        }
    };
    if elements.is_empty() {
        return Err(ApiError::bad_request(format!("'{split_by}' is empty")));
    }
    let limit = state.config.batch_limit.max(1);
    if elements.len() > limit {
        return Err(ApiError::bad_request(format!(
            "'{split_by}' has {} elements; the limit per batch is {limit}",
            elements.len()
        )));
    }
    // Each render works on its own copy of the data, so the memory a batch
    // needs is elements x data; keep that within a few bodies.
    let data_size = serde_json::to_vec(&data).map(|v| v.len()).unwrap_or(0);
    if data_size.saturating_mul(elements.len()) > state.config.body_limit.saturating_mul(4) {
        return Err(ApiError::bad_request(format!(
            "{} elements of {data_size} bytes of data each is more than this service renders in one batch; split the request",
            elements.len()
        )));
    }

    let count = elements.len();
    let total_timeout = state
        .config
        .api_timeout
        .saturating_mul(u32::try_from(count).unwrap_or(u32::MAX));
    let rid = request_id(&headers);
    let st = state.clone();
    let job = async move {
        let env = rusticdoc_core::template::environment();
        let mut outputs = Vec::with_capacity(count);
        for (index, element) in elements.into_iter().enumerate() {
            // Built just before it runs and dropped after, never all at once.
            let mut one = data.clone();
            replace(
                &mut one,
                &split_by,
                serde_json::Value::Array(vec![element.clone()]),
            );
            if let serde_json::Value::Object(map) = &mut one {
                map.insert("item".into(), element);
                map.insert(
                    "batch".into(),
                    serde_json::json!({ "index": index + 1, "count": count }),
                );
            }
            let name = env
                .render_str(
                    &name_template,
                    minijinja::Value::from(minijinja::value::Serde(&one)),
                )
                .map_err(|e| {
                    ApiError::new(
                        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                        "template_error",
                        format!("batchName: {e}"),
                    )
                })?;
            let mut req = RenderRequest::new(template.clone());
            req.data = one;
            req.ctx = ctx.clone();
            req.steps = steps.clone();
            req.output_name = Some(sanitize_name(&name, index + 1));
            outputs.push(execute(&st, req).await?);
        }
        crate::routes::form::zip(&zip_name, &outputs)
    };
    let job = async move {
        match tokio::time::timeout(total_timeout, job).await {
            Ok(result) => result,
            Err(_) => Err(rusticdoc_core::Error::Timeout(total_timeout).into()),
        }
    };
    match hook {
        None => Ok(download(job.await?)),
        Some(hook) => {
            let st = state.clone();
            let rid2 = rid.clone();
            tokio::spawn(async move {
                let result = job.await;
                webhook::deliver(st, hook, rid2, result).await;
            });
            Ok(webhook::accepted(&rid))
        }
    }
}

fn lookup<'a>(data: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    path.split('.').try_fold(data, |v, key| match v {
        serde_json::Value::Object(m) => m.get(key),
        serde_json::Value::Array(a) => key.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    })
}

fn replace(data: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let mut keys = path.split('.').peekable();
    let mut cur = data;
    while let Some(key) = keys.next() {
        let last = keys.peek().is_none();
        let next = match cur {
            serde_json::Value::Object(m) => m.get_mut(key),
            serde_json::Value::Array(a) => {
                key.parse::<usize>().ok().and_then(move |i| a.get_mut(i))
            }
            _ => None,
        };
        match next {
            Some(slot) if last => {
                *slot = value;
                return;
            }
            Some(slot) => cur = slot,
            None => return,
        }
    }
}

/// A file name from a template: no directories, no control characters,
/// never empty.
fn sanitize_name(name: &str, index: usize) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        index.to_string()
    } else {
        cleaned
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
    responses((status = 200, description = "The validation result", body = Validation), (status = 400, description = "The request is wrong", body = ErrorBody)),
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
