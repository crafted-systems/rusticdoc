use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::{Extension, Json};
use rusticdoc_core::job::{parse_steps, safe_file_name};
use rusticdoc_core::render::MAX_BATCH_NAME_BYTES;
use rusticdoc_core::{RenderRequest, Step, template};
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, BinaryFile, ErrorBody};
use crate::metrics::StageLabels;
use crate::routes::form::{Form, request_id};
use crate::routes::forms::{finish, finish_within};
use crate::routes::{options, templates};
use crate::state::AppState;
use crate::webhook::{self, Hook, Webhook};

/// The multipart form `/v1/render` accepts. Documented here; parsed by hand.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct RenderForm {
    /// The template file. Send this or `templateId`.
    #[schema(value_type = String, format = Binary)]
    pub template: Option<String>,
    /// The id of a stored template, from `POST /v1/templates`. Send this or
    /// `template`. Files in the form are then only further inputs, for `merge`.
    #[schema(rename = "templateId")]
    pub template_id: Option<String>,
    /// JSON object the template sees as its root, sent as text or as a file.
    #[schema(example = json!({"customer": {"name": "Ada"}}))]
    pub data: Option<serde_json::Value>,
    /// JSON object visible as `ctx`, sent as text or as a file.
    pub ctx: Option<serde_json::Value>,
    /// Comma-separated steps in order: `convert`, `pdfa`, `metadata`,
    /// `encrypt`, `merge`, `sign`. `sign` signs cryptographically with the
    /// service's key; it must be last and cannot be combined with `encrypt`.
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
    /// For `sign`: why the document was signed.
    pub reason: Option<String>,
    /// For `sign`: where it was signed.
    pub location: Option<String>,
    /// For `sign`: how to reach the signer.
    #[schema(rename = "contactInfo")]
    pub contact_info: Option<String>,
    /// For `sign`: the signer's name as shown.
    #[schema(rename = "signerName")]
    pub signer_name: Option<String>,
    /// File name of the result, without extension.
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
    /// What turns HTML into PDF: `fulgur` (default), a layout engine with
    /// no browser and no network, or `libreoffice`.
    #[schema(rename = "htmlEngine", example = "fulgur")]
    pub html_engine: Option<String>,
    /// Images (PNG, JPEG, GIF, SVG), stylesheets and fonts an HTML page names
    /// by file name, as in `<img src="logo.png">`. Every stylesheet applies to
    /// the whole page. Nothing is fetched from anywhere else.
    #[schema(value_type = Vec<String>, format = Binary, required = false)]
    pub assets: Option<Vec<String>>,
    /// Further files, used by `merge`.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Option<Vec<String>>,
}

/// Fills a template and runs the steps.
///
/// Add `X-Rusticdoc-Webhook-Url` to get 204 at once and the result by webhook.
/// With `RUSTICDOC_WEBHOOK_SECRET` set, each caller's deliveries are signed with its
/// own key: `X-Rusticdoc-Signature` is `v1=` and the hex HMAC-SHA256 of
/// `<timestamp>.<event>.<request id>.<body>`, with the timestamp in
/// `X-Rusticdoc-Timestamp`, the event in `X-Rusticdoc-Event` and the request id in
/// `X-Request-Id`. The key is `HMAC-SHA256(secret, principal)`, or the secret itself
/// when the service requires no credentials; `rusticdoc webhook-secret` prints it.
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
    Extension(principal): Extension<Principal>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    // First, because the first file left is the template.
    let data = form.json("data")?;
    let ctx = form.json("ctx")?;
    let stored = templates::named_in(&mut form, &state, &principal).await?;
    let from_store = stored.is_some();
    let template = match stored {
        Some(template) => template,
        None => match form
            .files
            .iter()
            .position(|f| f.name != "data.json" && f.name != "ctx.json")
        {
            Some(pos) => form.files.remove(pos),
            None => match form
                .field("templateHash")
                .map(str::trim)
                .filter(|h| !h.is_empty())
            {
                // Stateless by contract: a miss is a 404 and the client sends
                // the file again.
                Some(hash) => template::recall(&principal.0, hash).ok_or_else(|| {
                    ApiError::new(
                        axum::http::StatusCode::NOT_FOUND,
                        "template_not_cached",
                        "no template with that hash is cached; send the file",
                    )
                })?,
                None => {
                    return Err(ApiError::bad_request(
                        "no template in the form; send it as a file part, a 'templateId' or a 'templateHash'",
                    ));
                }
            },
        },
    };
    // A stored template is there to be asked for by id, and a delete has to end it.
    if !from_store {
        template::remember(&principal.0, &template);
    }
    let mut req = RenderRequest::new(template);
    req.data = data;
    req.ctx = ctx;
    let mut options = options::convert_options(&form)?;
    // Here PDF/A and PDF/UA happen where the step list says, not on every
    // conversion.
    options.pdfa = None;
    options.pdfua = false;
    req.convert_options = options;
    req.steps = named_steps(&mut form)?;
    // Here, so a webhook request hears about a bad step list at once.
    state.engines.check_steps(&req.steps)?;
    req.output_name = form.output_name();
    req.extra = std::mem::take(&mut form.files);

    // A PDF template is a form: fill it here, then run the steps on the
    // result like any other document.
    if req.template.extension() == "pdf" && !req.data.as_object().is_none_or(|m| m.is_empty()) {
        rusticdoc_core::pdfops::require_pdf(&req.template)?;
        let mut data = req.data.clone();
        if let serde_json::Value::Object(map) = &mut data {
            map.entry("ctx").or_insert(req.ctx.clone());
        }
        let options = rusticdoc_core::pdfform::FillOptions {
            flatten: form.bool("flatten"),
            strip_tag_annotations: form
                .field("stripAnnotations")
                .map(|v| v != "false")
                .unwrap_or(true),
        };
        let filled = state
            .pipeline
            .renderer()
            .fill(
                &req.template.bytes,
                &data,
                &options,
                state.config.max_render_bytes,
                state.config.api_timeout,
            )
            .await?;
        req.template = rusticdoc_core::Document::new(req.template.name.clone(), filled);
        req.templating = false;
    }

    run(state, hook, request_id(&headers), req).await
}

/// The form's `steps` list. Steps that carry files or repeat
/// (`watermark`, `stamp`, `embed`, `factur-x`) are built from their own
/// fields, in the position the list names.
fn named_steps(form: &mut Form) -> ApiResult<Vec<Step>> {
    let list = form.field("steps").unwrap_or("").to_string();
    let carried = options::post_steps(form, false)?;
    let mut steps = Vec::new();
    for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let wanted: Option<fn(&Step) -> bool> = match name.to_ascii_lowercase().as_str() {
            "watermark" => Some(|s| matches!(s, Step::Watermark { .. })),
            "stamp" => Some(|s| matches!(s, Step::Stamp { .. })),
            "embed" => Some(|s| matches!(s, Step::Embed { .. })),
            "factur-x" | "facturx" => Some(|s| matches!(s, Step::Facturx { .. })),
            _ => None,
        };
        match wanted {
            Some(matches) => {
                let before = steps.len();
                steps.extend(carried.iter().filter(|s| matches(s)).cloned());
                if steps.len() == before {
                    return Err(ApiError::bad_request(format!(
                        "step '{name}' has nothing to do; send its fields"
                    )));
                }
            }
            None => steps.extend(parse_steps(name, &form.fields)?),
        }
    }
    Ok(steps)
}

/// Runs a request now, or hands it to a webhook task.
pub async fn run(
    state: AppState,
    hook: Option<Webhook>,
    request_id: String,
    req: RenderRequest,
) -> ApiResult<Response> {
    let st = state.clone();
    finish(
        state,
        hook,
        request_id,
        async move { execute(&st, req).await },
    )
    .await
}

/// Renders the template once per element of an array in the data and
/// returns the files in a ZIP.
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
    Extension(principal): Extension<Principal>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    // The route's time limit is the batch's; reading the form gets the usual one.
    let read = Form::read(multipart, &state);
    let mut form = webhook::within(state.config.api_timeout, read).await?;
    // First, because the first file left is the template.
    let data = form.json("data")?;
    let ctx = form.json("ctx")?;
    let stored = templates::named_in(&mut form, &state, &principal).await?;
    let template = match stored {
        Some(template) => template,
        None => match form
            .files
            .iter()
            .position(|f| f.name != "data.json" && f.name != "ctx.json")
        {
            Some(pos) => form.files.remove(pos),
            None => {
                return Err(ApiError::bad_request(
                    "no template in the form; send it as a file part or a 'templateId'",
                ));
            }
        },
    };
    let steps = parse_steps(form.field("steps").unwrap_or(""), &form.fields)?;
    state.engines.check_steps(&steps)?;
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
    let zip_name = form.output_name().unwrap_or_else(|| {
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
    let limit = state.config.batch_timeout;
    let max_render = state.config.max_render_bytes;
    let st = state.clone();
    let job = async move {
        let renderer = st.pipeline.renderer().clone();
        let started = std::time::Instant::now();
        // One worker for the job while nothing else runs between renders. With
        // steps, a conversion can take far longer than a render, so the place
        // among the workers is not held through it.
        let mut session = None;
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
            let current = match &mut session {
                Some(current) => current,
                empty => {
                    let left = limit.saturating_sub(started.elapsed());
                    empty.insert(renderer.session(left).await?)
                }
            };
            let name = current
                .text(&name_template, &one, MAX_BATCH_NAME_BYTES)
                .await
                .map_err(|e| match e {
                    rusticdoc_core::Error::Template(m) => {
                        rusticdoc_core::Error::Template(format!("batchName: {m}"))
                    }
                    other => other,
                })?;
            let rendered = current.render(&template, &one, &ctx, max_render).await?;
            if !steps.is_empty() {
                session = None;
            }
            let mut req = RenderRequest::new(rendered);
            req.templating = false;
            req.steps = steps.clone();
            req.output_name = Some(safe_file_name(&name, &(index + 1).to_string()));
            outputs.push(execute(&st, req).await?);
        }
        crate::routes::form::zip(&zip_name, &outputs, st.config.api_timeout).await
    };
    finish_within(state, hook, request_id(&headers), limit, job).await
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

pub async fn execute(state: &AppState, req: RenderRequest) -> ApiResult<rusticdoc_core::Output> {
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
    /// The engine that would handle it: `office`, `text` or `none`.
    pub engine: &'static str,
    /// The first error, when `ok` is false.
    pub error: Option<String>,
    /// Every problem found: `severity`, `message`, and a `location` with
    /// `part`, `place` and `snippet` where known.
    #[schema(value_type = Vec<Object>)]
    pub diagnostics: Vec<serde_json::Value>,
    /// Fonts the template names that the service has neither loaded nor
    /// mapped to a substitute with the same metrics. Text set in them renders
    /// in a fallback font, which moves line and page breaks. Each also appears
    /// in `diagnostics` as a warning.
    pub missing_fonts: Vec<String>,
}

/// Checks a template against sample data without converting anything.
#[utoipa::path(
    post, path = "/v1/templates/validate", tag = "render",
    request_body(content_type = "multipart/form-data", content = RenderForm),
    responses((status = 200, description = "The validation result", body = Validation), (status = 400, description = "The request is wrong", body = ErrorBody)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn validate(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    multipart: Multipart,
) -> ApiResult<Json<Validation>> {
    let mut form = Form::read(multipart, &state).await?;
    // First, because the first file left is the template.
    let data = form.json("data")?;
    let ctx = form.json("ctx")?;
    let template = match templates::named_in(&mut form, &state, &principal).await? {
        Some(template) => template,
        None => form.require_file("a template file, or a 'templateId',")?,
    };
    let format = template.extension();
    let engine = if template::is_office_format(&format) {
        "office"
    } else if template::is_text_template(&format) {
        "text"
    } else {
        "none"
    };
    let hash = template::template_hash(&template.bytes);
    let diagnostics = state
        .pipeline
        .renderer()
        .validate(
            &template,
            &data,
            &ctx,
            state.config.max_render_bytes,
            state.config.api_timeout,
        )
        .await?;
    let mut diagnostics = diagnostics;
    let wanted = rusticdoc_core::fonts::referenced(&template);
    let missing_fonts = if wanted.is_empty() {
        Vec::new()
    } else {
        let loaded = crate::routes::fonts::loaded(&state).await?;
        rusticdoc_core::fonts::missing(&wanted, &loaded)
    };
    for family in &missing_fonts {
        diagnostics.push(rusticdoc_template::Diagnostic {
            severity: rusticdoc_template::Severity::Warning,
            message: format!(
                "the template uses the font '{family}', which the service does not have; \
                 another font will stand in, which can move line and page breaks. \
                 Upload it to /v1/fonts or mount it in the fonts directory"
            ),
            location: None,
        });
    }
    let error = diagnostics
        .iter()
        .find(|d| matches!(d.severity, rusticdoc_template::Severity::Error))
        .map(|d| match &d.location {
            Some(l) => format!("{} ({} {}: {})", d.message, l.part, l.place, l.snippet),
            None => d.message.clone(),
        });
    Ok(Json(Validation {
        ok: error.is_none(),
        template_hash: hash,
        format,
        engine,
        error,
        diagnostics: diagnostics
            .iter()
            .map(|d| serde_json::to_value(d).unwrap_or_default())
            .collect(),
        missing_fonts,
    }))
}
