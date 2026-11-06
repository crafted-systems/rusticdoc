use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::Response;
use rusticdoc_core::convert::ConvertOptions;
use rusticdoc_core::{Document, Format, Output, RenderRequest, Step};
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult, BinaryFile, ErrorBody};
use crate::routes::form::{Form, request_id, zip};
use crate::routes::forms::finish_within;
use crate::routes::gotenberg;
use crate::state::AppState;
use crate::webhook::{self, Hook, Webhook};

/// The multipart form `/v1/convert` and `/forms/libreoffice/convert` accept.
/// Gotenberg's LibreOffice options (`landscape`, `nativePageRanges`,
/// `exportFormFields`, `singlePageSheets`, `password`, ...) and its
/// post-processing fields (`merge`, `splitMode`, `watermarkSource`,
/// `stampSource`, `rotateAngle`, `flatten`, `metadata`, `bookmarks`,
/// `embeds`, `facturxXml`, `userPassword`, ...) are accepted as well.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct ConvertForm {
    /// One or more documents.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Vec<String>,
    /// Output format. Default `pdf`.
    pub to: Option<String>,
    /// PDF/A level: `PDF/A-1b`, `PDF/A-2b`, `PDF/A-3b`, `PDF/A-4`.
    pub pdfa: Option<String>,
    /// Export as PDF/UA.
    pub pdfua: Option<bool>,
    /// Merge several inputs into one PDF instead of returning a ZIP.
    pub merge: Option<bool>,
    pub landscape: Option<bool>,
    #[schema(rename = "nativePageRanges", example = "1-3,5")]
    pub native_page_ranges: Option<String>,
    /// Opens a protected input.
    pub password: Option<String>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

/// Converts uploads with LibreOffice and runs any post-processing fields.
/// Several files come back as a ZIP, or as one PDF with `merge=true`.
#[utoipa::path(
    post, path = "/v1/convert", tag = "convert",
    request_body(content_type = "multipart/form-data", content = ConvertForm),
    responses(
        (status = 200, description = "The converted file, or a ZIP of them", content_type = "application/octet-stream", body = BinaryFile),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 503, description = "An engine is unavailable or the request timed out", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn convert(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    // The route's time limit is a multi-file job's; reading the form gets the
    // usual one.
    let read = Form::read(multipart, &state);
    let mut form = webhook::within(state.config.api_timeout, read).await?;
    if form.files.is_empty() {
        return Err(ApiError::bad_request(
            "no file in the form; send one or more as 'files'",
        ));
    }
    let to = Format::parse(form.field("to").unwrap_or("pdf"))?;
    let options = gotenberg::convert_options(&form)?;
    // PDF/A and PDF/UA are exported natively by LibreOffice here, so they
    // leave the post-processing chain.
    let post: Vec<Step> = gotenberg::post_steps(&mut form, true)?
        .into_iter()
        .filter(|s| !matches!(s, Step::Pdfa { .. } | Step::Pdfua))
        .collect();
    let output_name = form.output_name();
    let files = std::mem::take(&mut form.files);
    let merge = post.first() == Some(&Step::Merge);

    if files.len() == 1 || merge {
        let limit = job_limit(&state, files.len());
        let st = state.clone();
        let rid = request_id(&headers);
        let job = async move {
            let mut req = RenderRequest::new(files[0].clone());
            req.templating = false;
            req.convert_options = options.clone();
            req.steps = vec![Step::Convert { to }];
            if merge && files.len() > 1 {
                // The other inputs are converted first, then merged in order.
                let mut extra = Vec::new();
                for doc in &files[1..] {
                    let mut r = RenderRequest::new(doc.clone());
                    r.templating = false;
                    r.convert_options = options.clone();
                    r.steps = vec![Step::Convert { to: Format::Pdf }];
                    let out = tokio::time::timeout(st.config.api_timeout, st.pipeline.run(r))
                        .await
                        .map_err(|_| rusticdoc_core::Error::Timeout(st.config.api_timeout))??;
                    extra.push(Document::new(out.name, out.bytes));
                }
                req.extra = extra;
            }
            req.steps
                .extend(post.into_iter().filter(|s| merge || *s != Step::Merge));
            req.output_name = output_name;
            crate::routes::render::execute(&st, req).await
        };
        return finish_within(state, hook, rid, limit, job).await;
    }

    // Several files, no merge: each through the same steps, zipped together.
    let steps: Vec<Step> = std::iter::once(Step::Convert { to }).chain(post).collect();
    run_each(
        state,
        hook,
        request_id(&headers),
        files,
        steps,
        options,
        output_name,
    )
    .await
}

/// Gotenberg's `/forms/pdfengines/convert`: PDFs to PDF/A or PDF/UA through
/// LibreOffice, with the same post-processing fields.
#[utoipa::path(
    post, path = "/v1/pdf/convert", tag = "pdf",
    request_body(content_type = "multipart/form-data", content = ConvertForm),
    responses(
        (status = 200, description = "The converted PDF, or a ZIP of them", content_type = "application/octet-stream", body = BinaryFile),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, description = "The request is wrong", body = ErrorBody), (status = 503, description = "An engine is unavailable or the request timed out", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn convert_pdf(
    State(state): State<AppState>,
    Hook(hook): Hook,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let mut form = Form::read(multipart, &state).await?;
    if form.files.is_empty() {
        return Err(ApiError::bad_request(
            "no file in the form; send one or more PDFs as 'files'",
        ));
    }
    for doc in &form.files {
        rusticdoc_core::pdfops::require_pdf(doc)?;
    }
    let options = gotenberg::convert_options(&form)?;
    let steps = gotenberg::post_steps(&mut form, false)?;
    if !steps
        .iter()
        .any(|s| matches!(s, Step::Pdfa { .. } | Step::Pdfua))
    {
        return Err(ApiError::bad_request(
            "Invalid form data: either 'pdfa' or 'pdfua' is required",
        ));
    }
    let output_name = form.output_name();
    let files = std::mem::take(&mut form.files);
    run_each(
        state,
        hook,
        request_id(&headers),
        files,
        steps,
        options,
        output_name,
    )
    .await
}

/// Runs every file through the same steps: one file comes back as itself,
/// several as a ZIP; webhooks as everywhere.
pub async fn run_each(
    state: AppState,
    hook: Option<Webhook>,
    rid: String,
    files: Vec<Document>,
    steps: Vec<Step>,
    options: ConvertOptions,
    output_name: Option<String>,
) -> ApiResult<Response> {
    let st = state.clone();
    let single = files.len() == 1;
    let limit = job_limit(&state, files.len());
    let job = async move {
        let mut outputs: Vec<Output> = Vec::with_capacity(files.len());
        for doc in files {
            let mut r = RenderRequest::new(doc);
            r.templating = false;
            r.convert_options = options.clone();
            r.steps = steps.clone();
            if single {
                r.output_name = output_name.clone();
            }
            let out = tokio::time::timeout(st.config.api_timeout, st.pipeline.run(r))
                .await
                .map_err(|_| rusticdoc_core::Error::Timeout(st.config.api_timeout))??;
            st.metrics.record_timings(&out.timings);
            outputs.push(out);
        }
        if outputs.len() == 1 {
            Ok(outputs.remove(0))
        } else {
            zip(
                output_name.as_deref().unwrap_or("converted"),
                &outputs,
                st.config.api_timeout,
            )
            .await
        }
    };
    finish_within(state, hook, rid, limit, job).await
}

/// One file gets the usual time limit, several the batch's.
fn job_limit(state: &AppState, files: usize) -> std::time::Duration {
    if files > 1 {
        state.config.batch_timeout
    } else {
        state.config.api_timeout
    }
}
