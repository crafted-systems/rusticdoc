use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::Response;
use rusticdoc_core::{Format, RenderRequest, Step};
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::routes::form::{Form, download, request_id, zip};
use crate::routes::render;
use crate::state::AppState;
use crate::webhook::Webhook;

/// The multipart form `/v1/convert` and `/forms/libreoffice/convert` accept.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct ConvertForm {
    /// One or more documents.
    #[schema(value_type = Vec<String>, format = Binary)]
    pub files: Vec<String>,
    /// Output format. Default `pdf`.
    pub to: Option<String>,
    /// PDF/A level; implies PDF output.
    pub pdfa: Option<String>,
    /// Merge several inputs into one PDF instead of returning a ZIP.
    pub merge: Option<bool>,
    #[schema(rename = "outputFilename")]
    pub output_filename: Option<String>,
}

/// Converts uploads with LibreOffice. Several files come back as a ZIP, or
/// as one PDF with `merge=true`.
#[utoipa::path(
    post, path = "/v1/convert", tag = "convert",
    request_body(content_type = "multipart/form-data", content = ConvertForm),
    responses(
        (status = 200, description = "The converted file, or a ZIP of them", content_type = "application/octet-stream"),
        (status = 204, description = "Accepted; the result goes to the webhook"),
        (status = 400, body = ErrorBody), (status = 503, body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn convert(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<Response> {
    let hook = Webhook::from_headers(&headers, state.config.webhook_allow_private).await?;
    let mut form = Form::read(multipart).await?;
    if form.files.is_empty() {
        return Err(ApiError::bad_request(
            "no file in the form; send one or more as 'files'",
        ));
    }
    let to = Format::parse(form.field("to").unwrap_or("pdf"))?;
    let mut steps = vec![Step::Convert { to }];
    if let Some(level) = form.field("pdfa").filter(|s| !s.trim().is_empty()) {
        steps = vec![Step::Pdfa {
            level: rusticdoc_core::job::PdfaLevel::parse(level)?,
        }];
    }
    let output_name = form
        .field("outputFilename")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let merge = form.bool("merge");
    let files = std::mem::take(&mut form.files);

    if files.len() == 1 || merge {
        let mut req = RenderRequest::new(files[0].clone());
        req.templating = false;
        req.steps = steps;
        req.output_name = output_name;
        if merge && files.len() > 1 {
            // Convert the rest first, then merge them after the main one.
            let mut extra = Vec::new();
            for doc in &files[1..] {
                let mut r = RenderRequest::new(doc.clone());
                r.templating = false;
                r.steps = vec![Step::Convert { to: Format::Pdf }];
                let out = tokio::time::timeout(state.config.api_timeout, state.pipeline.run(r))
                    .await
                    .map_err(|_| rusticdoc_core::Error::Timeout(state.config.api_timeout))??;
                extra.push(rusticdoc_core::Document::new(out.name, out.bytes));
            }
            req.extra = extra;
            req.steps.push(Step::Merge);
        }
        return render::run(state, hook, request_id(&headers), req).await;
    }

    // Several files, no merge: each converted on its own, zipped together.
    let rid = request_id(&headers);
    let st = state.clone();
    let job = async move {
        let mut outputs = Vec::with_capacity(files.len());
        for doc in files {
            let mut r = RenderRequest::new(doc);
            r.templating = false;
            r.steps = steps.clone();
            let out = tokio::time::timeout(st.config.api_timeout, st.pipeline.run(r))
                .await
                .map_err(|_| rusticdoc_core::Error::Timeout(st.config.api_timeout))??;
            outputs.push(out);
        }
        zip(output_name.as_deref().unwrap_or("converted"), &outputs)
    };
    match hook {
        None => Ok(download(job.await?)),
        Some(hook) => {
            let st = state.clone();
            let rid2 = rid.clone();
            tokio::spawn(async move {
                let result = job.await;
                crate::webhook::deliver(st, hook, rid2, result).await;
            });
            Ok(crate::webhook::accepted(&rid))
        }
    }
}
