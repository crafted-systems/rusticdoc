//! Structured logs and request spans.

use std::time::Instant;

use axum::body::Body;
use axum::extract::{OriginalUri, State};
use axum::http::{Request, header};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

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

/// Longest correlation id kept, in bytes.
const MAX_CORRELATION_ID: usize = 128;

/// A client's correlation id as it may be logged and echoed: printable ASCII,
/// under a length cap. `None` when nothing is left of it.
pub fn sanitize_correlation_id(value: &str) -> Option<String> {
    let clean: String = value
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(MAX_CORRELATION_ID)
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

/// True for the routes the settings leave out of the access log. `path` is
/// relative to the root path.
fn telemetry_off(state: &AppState, path: &str) -> bool {
    let compat = &state.config.compat;
    match path {
        "/health" => compat.health_telemetry_off,
        "/" => compat.root_telemetry_off,
        "/debug" => compat.debug_telemetry_off,
        "/version" => compat.version_telemetry_off,
        p if p == compat.prometheus_path => compat.prometheus_telemetry_off,
        _ => false,
    }
}

/// One span and one log line per request, in the fields Gotenberg's access log
/// has, except for the routes that are configured out of it.
pub async fn access(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if telemetry_off(&state, &path) {
        return next.run(req).await;
    }
    let full_path = req
        .extensions()
        .get::<OriginalUri>()
        .map(|uri| uri.path().to_string())
        .unwrap_or(path);
    let (host, referer, user_agent) = {
        let text = |name: header::HeaderName| -> String {
            req.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.chars().take(256).collect())
                .unwrap_or_default()
        };
        (
            text(header::HOST),
            text(header::REFERER),
            text(header::USER_AGENT),
        )
    };
    let correlation_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(sanitize_correlation_id)
        .unwrap_or_else(|| "-".to_string());
    let bytes_in = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let method = req.method().to_string();
    let span = tracing::info_span!("request", request_id = %correlation_id);
    let started = Instant::now();
    let res = next.run(req).instrument(span.clone()).await;
    let latency = started.elapsed();
    let status = res.status().as_u16();
    let bytes_out = res
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    span.in_scope(|| {
        macro_rules! access_line {
            ($level:ident) => {
                tracing::$level!(
                    log_type = "access",
                    correlation_id = %correlation_id,
                    host = %host,
                    method = %method,
                    path = %full_path,
                    referer = %referer,
                    user_agent = %user_agent,
                    status,
                    latency = latency.as_nanos() as u64,
                    latency_human = %format!("{latency:?}"),
                    latency_ms = latency.as_millis() as u64,
                    bytes_in,
                    bytes_out,
                    "request handled"
                )
            };
        }
        if status >= 500 {
            access_line!(error);
        } else {
            access_line!(info);
        }
    });
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlation_ids_are_cleaned() {
        assert_eq!(sanitize_correlation_id("abc-1").as_deref(), Some("abc-1"));
        assert_eq!(
            sanitize_correlation_id("a\nb\u{7}c\u{e9}").as_deref(),
            Some("abc")
        );
        assert_eq!(sanitize_correlation_id("\u{7}"), None);
        assert_eq!(
            sanitize_correlation_id(&"x".repeat(500)).unwrap().len(),
            MAX_CORRELATION_ID
        );
    }
}
