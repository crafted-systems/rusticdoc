//! Structured logs and request spans.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, Response};
use tracing::Span;
use tracing_subscriber::EnvFilter;

/// Installs the global subscriber: JSON lines by default, `pretty` for a
/// terminal. Safe to call once per process.
pub fn init(format: &str, level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    if format == "pretty" {
        builder.pretty().init();
    } else {
        builder
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .init();
    }
}

pub fn make_span(req: &Request<Body>) -> Span {
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    tracing::info_span!(
        "request",
        method = %req.method(),
        path = %req.uri().path(),
        request_id = %request_id,
    )
}

pub fn on_response(res: &Response<Body>, latency: Duration, _span: &Span) {
    let status = res.status().as_u16();
    if status >= 500 {
        tracing::error!(status, latency_ms = latency.as_millis(), "response");
    } else {
        tracing::info!(status, latency_ms = latency.as_millis(), "response");
    }
}
