//! Gotenberg's API, on Gotenberg's paths.
//!
//! `/forms/*` and the service routes around them answer as Gotenberg does:
//! many inputs and a ZIP, its file naming, `text/plain` errors with its exact
//! strings. `/v1/*` is the service's own API and keeps JSON errors. The work
//! itself is in [`rusticdoc_core::compat`]; here, forms become its structures
//! and results become responses or webhook deliveries.

pub mod form;
pub mod libreoffice;
pub mod pdfengines;
pub mod service;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::multipart::Multipart;
use axum::extract::{FromRequest, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use rusticdoc_core::compat::Compat;
use rusticdoc_core::{Config, Document, Error, Output, job};
use tracing::Instrument;
use uuid::Uuid;

use crate::auth::Principal;
use crate::error::{ApiError, ErrorText, public_text};
use crate::routes::form::{self as uploads, Form, download, request_id};
use crate::state::AppState;
use crate::webhook::{self, Webhook};
use form::FormData;

/// Longest file name in a response or a ZIP, in bytes: what filesystems take.
const MAX_NAME_BYTES: usize = 255;

/// The routes of `/forms/*`. A disabled group is not routed at all, so a request
/// to it gets 404 before anything reads its form.
pub fn forms(config: &Config) -> Router<AppState> {
    let mut router = Router::new();
    if !config.compat.libreoffice_disable_routes {
        router = router.route("/forms/libreoffice/convert", post(libreoffice::convert));
    }
    if !config.compat.pdfengines_disable_routes {
        router = router
            .route("/forms/pdfengines/merge", post(pdfengines::merge))
            .route("/forms/pdfengines/split", post(pdfengines::split))
            .route("/forms/pdfengines/flatten", post(pdfengines::flatten))
            .route("/forms/pdfengines/optimize", post(pdfengines::optimize))
            .route("/forms/pdfengines/convert", post(pdfengines::convert))
            .route(
                "/forms/pdfengines/metadata/read",
                post(pdfengines::metadata_read),
            )
            .route(
                "/forms/pdfengines/metadata/write",
                post(pdfengines::metadata_write),
            )
            .route(
                "/forms/pdfengines/bookmarks/read",
                post(pdfengines::bookmarks_read),
            )
            .route(
                "/forms/pdfengines/bookmarks/write",
                post(pdfengines::bookmarks_write),
            )
            .route("/forms/pdfengines/encrypt", post(pdfengines::encrypt))
            .route("/forms/pdfengines/embed", post(pdfengines::embed))
            .route("/forms/pdfengines/watermark", post(pdfengines::watermark))
            .route("/forms/pdfengines/stamp", post(pdfengines::stamp))
            .route("/forms/pdfengines/rotate", post(pdfengines::rotate))
            .route("/forms/pdfengines/factur-x", post(pdfengines::facturx));
    }
    router
}

/// What a route produced.
pub enum Reply {
    /// One response file, or a ZIP of several.
    Files(Vec<Document>),
    /// A JSON answer, which cannot go to a webhook.
    Json(serde_json::Value),
}

/// A core failure as the client should read it: the text alone, without the
/// prefix `/v1/*` uses.
pub fn client_error(error: Error) -> ApiError {
    match error {
        Error::InvalidRequest(message) => ApiError::bad_request(message),
        other => other.into(),
    }
}

/// The engines of this request.
pub fn engines(state: &AppState) -> Compat<'_> {
    Compat {
        engines: &state.engines,
        timeout: state.config.api_timeout,
    }
}

/// Every error on Gotenberg's paths is `text/plain`, in its words. A middleware,
/// so it also covers what never reaches a handler: authentication, an unknown
/// route, a request that ran out of time.
pub async fn plain_errors(req: Request, next: Next) -> Response {
    let res = next.run(req).await;
    let status = res.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return res;
    }
    let (mut parts, _body) = res.into_parts();
    let text = match parts.extensions.remove::<ErrorText>() {
        Some(error) => public_text(status, error.kind, &error.message),
        None => status.canonical_reason().unwrap_or("Error").to_string(),
    };
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=UTF-8"),
    );
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(text))
}

/// Reads the form, then runs `op`: now, or in the background when the request
/// carries a webhook. Validation happens inside `op`, so with a webhook its
/// failures arrive at the error URL, as they do on Gotenberg.
pub async fn run<F, Fut>(state: AppState, req: Request, op: F) -> Response
where
    F: FnOnce(AppState, FormData) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Reply, ApiError>> + Send + 'static,
{
    let headers = req.headers().clone();
    // A webhook job's place is taken before the body is read.
    let wants_hook = !state.config.compat.webhook_disabled
        && ["gotenberg-webhook-url", "x-rusticdoc-webhook-url"]
            .iter()
            .any(|name| headers.contains_key(*name));
    let caller = req
        .extensions()
        .get::<Principal>()
        .map(|p| p.0.clone())
        .unwrap_or_else(|| "public".to_string());
    let permit = if wants_hook {
        match state.jobs.try_acquire(&caller) {
            Ok(permit) => Some(Arc::new(permit)),
            Err(e) => return e.into_response(),
        }
    } else {
        None
    };
    let form = match read_form(&state, req).await {
        Ok(form) => form,
        Err(e) => return e.into_response(),
    };
    let hook = if state.config.compat.webhook_disabled {
        None
    } else {
        match Webhook::from_gotenberg_headers(&headers, &state.url_policies).await {
            Ok(hook) => hook.map(|hook| Webhook {
                permit,
                principal: caller,
                ..hook
            }),
            Err(e) => return e.into_response(),
        }
    };
    let rid = request_id(&headers);
    let output_name = output_filename(&headers);
    let st = state.clone();
    let job = async move {
        let limit = st.config.api_timeout;
        let work = op(st.clone(), FormData::new(form));
        match tokio::time::timeout(limit, work).await {
            Ok(result) => result,
            Err(_) => Err(Error::Timeout(limit).into()),
        }
    };
    let Some(hook) = hook else {
        return match job.await {
            Ok(Reply::Files(docs)) => {
                match respond_files(docs, output_name.as_deref(), state.config.api_timeout).await {
                    Ok(out) => download(out).await,
                    Err(e) => e.into_response(),
                }
            }
            Ok(Reply::Json(value)) => Json(value).into_response(),
            Err(e) => e.into_response(),
        };
    };
    let delivery = {
        let state = state.clone();
        let rid = rid.clone();
        async move {
            let result = match job.await {
                Ok(Reply::Files(docs)) => {
                    respond_files(docs, output_name.as_deref(), state.config.api_timeout).await
                }
                Ok(Reply::Json(_)) => Err(ApiError::bad_request(
                    "The webhook middleware can only work with multipart/form-data routes that results in output files",
                )),
                Err(e) => Err(e),
            };
            webhook::deliver(state, hook, rid, result).await;
        }
    };
    if state.config.compat.webhook_sync {
        delivery.await;
    } else {
        tokio::spawn(delivery.instrument(tracing::Span::current()));
    }
    webhook::accepted(&rid)
}

/// The multipart form, with Gotenberg's answers for what is not one.
async fn read_form(state: &AppState, req: Request) -> Result<Form, ApiError> {
    let multipart = Multipart::from_request(req, state).await.map_err(|_| {
        ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_request",
            "Invalid 'Content-Type' header value: want 'multipart/form-data'",
        )
    })?;
    Form::read(multipart, state).await.map_err(|e| {
        if e.body.message.starts_with("multipart form:") {
            if e.body.message.contains("limit") {
                return ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "invalid_request",
                    "The request body exceeds the configured size limit. Increase it with --api-body-limit, or send a smaller body.",
                );
            }
            return ApiError::bad_request(
                "Malformed body: it does not match the 'Content-Type' header boundaries",
            );
        }
        e
    })
}

/// `Gotenberg-Output-Filename`, without any path, as a base name.
fn output_filename(headers: &HeaderMap) -> Option<String> {
    headers
        .get("gotenberg-output-filename")
        .and_then(|v| std::str::from_utf8(v.as_bytes()).ok())
        .map(uploads::sanitize)
        .filter(|name| !name.is_empty() && name != "upload")
}

/// The response file for what a route produced: the one document, or a ZIP of
/// them, under the requested name when there is one.
pub async fn respond_files(
    docs: Vec<Document>,
    wanted: Option<&str>,
    timeout: Duration,
) -> Result<Output, ApiError> {
    let mut docs = docs;
    if docs.len() == 1 {
        let doc = docs.remove(0);
        let name = match wanted {
            Some(wanted) => format!("{wanted}.{}", extension_of(&doc.name)),
            None => doc.name.clone(),
        };
        return Ok(Output {
            name: short_name(&name),
            content_type: doc.content_type(),
            bytes: doc.bytes,
            timings: Vec::new(),
        });
    }
    let parts: Vec<Document> = docs
        .into_iter()
        .map(|doc| Document::new(short_name(&uploads::sanitize(&doc.name)), doc.bytes))
        .collect();
    let stem = wanted
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let archive = job::zip_off_thread(&stem, parts, timeout)
        .await
        .map_err(ApiError::from)?;
    Ok(Output {
        name: short_name(&archive.name),
        content_type: "application/zip",
        bytes: archive.bytes,
        timings: Vec::new(),
    })
}

fn extension_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("pdf")
}

/// A name that fits a filesystem: long names lose the end of their stem, never
/// their extension, and are cut on a character boundary.
pub fn short_name(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(at) if name.len() - at < 32 => name.split_at(at),
        _ => (name, ""),
    };
    let mut keep = MAX_NAME_BYTES.saturating_sub(ext.len());
    while keep > 0 && !stem.is_char_boundary(keep) {
        keep -= 1;
    }
    format!("{}{ext}", &stem[..keep])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_keep_their_extension() {
        let long = format!("{}.pdf", "å".repeat(200));
        let short = short_name(&long);
        assert!(short.len() <= MAX_NAME_BYTES);
        assert!(short.ends_with(".pdf"));
        assert_eq!(short_name("a.pdf"), "a.pdf");
    }

    #[tokio::test]
    async fn one_file_stays_a_file_and_many_become_a_zip() {
        let limit = Duration::from_secs(30);
        let one = respond_files(
            vec![Document::new("a.pdf", b"x".to_vec())],
            Some("foo"),
            limit,
        )
        .await
        .unwrap();
        assert_eq!(one.name, "foo.pdf");
        assert_eq!(one.content_type, "application/pdf");
        let many = respond_files(
            vec![
                Document::new("a.pdf", b"x".to_vec()),
                Document::new("b.pdf", b"y".to_vec()),
            ],
            Some("foo"),
            limit,
        )
        .await
        .unwrap();
        assert_eq!(many.name, "foo.zip");
        assert_eq!(many.content_type, "application/zip");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(many.bytes.to_vec())).unwrap();
        assert_eq!(archive.len(), 2);
        assert!(archive.by_name("b.pdf").is_ok());
    }

    #[test]
    fn engine_failures_stay_out_of_the_text() {
        assert_eq!(
            public_text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "engine_failed",
                "qpdf failed: /tmp/secret"
            ),
            "Internal Server Error"
        );
        assert_eq!(
            public_text(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Invalid form data: x"
            ),
            "Invalid form data: x"
        );
        assert_eq!(
            public_text(StatusCode::INTERNAL_SERVER_ERROR, "reported", "said"),
            "said"
        );
        assert!(
            public_text(StatusCode::SERVICE_UNAVAILABLE, "timeout", "x").contains("time limit")
        );
    }
}
