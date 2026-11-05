//! rusticdoc server: the HTTP API over the core pipeline.
//!
//! Routes are versioned under `/v1`, with Gotenberg's form paths kept as
//! aliases so existing clients keep working. Every response carries an
//! `x-request-id`; webhooks deliver results asynchronously with an HMAC
//! signature; `/openapi.json` describes the API for SDK generation.

pub mod auth;
pub mod error;
pub mod metrics;
pub mod openapi;
pub mod routes;
pub mod state;
pub mod telemetry;
pub mod webhook;

use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use state::AppState;

pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The full application router with its middleware.
pub fn app(state: AppState) -> Router {
    let timeout = state.config.api_timeout + Duration::from_secs(5);
    let body_limit = state.config.body_limit;
    let version = HeaderValue::from_static(rusticdoc_core::VERSION);

    let api = routes::router()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require,
        ))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            timeout,
        ));

    let router = Router::new()
        .merge(routes::public())
        .merge(api)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            metrics::observe,
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(telemetry::make_span)
                .on_response(telemetry::on_response),
        )
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-rusticdoc-version"),
            version,
        ))
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
        .layer(axum::middleware::from_fn(gotenberg_headers))
        .layer(cors(&state.config))
        .layer(CatchPanicLayer::new())
        .with_state(state.clone());

    // Gotenberg's API_ROOT_PATH: everything, health included, under a prefix.
    let root = state.config.root_path.trim_end_matches('/');
    if root.is_empty() {
        router
    } else {
        Router::new().nest(root, router)
    }
}

/// Gotenberg's request conventions: `Gotenberg-Trace` becomes the request
/// id and is echoed back, and `Gotenberg-Output-Filename` renames the file
/// in the response, keeping the extension the route produced.
async fn gotenberg_headers(
    mut req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Some(trace) = req.headers().get("gotenberg-trace").cloned()
        && trace
            .to_str()
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false)
    {
        req.headers_mut().insert(REQUEST_ID, trace);
    }
    let wanted = req
        .headers()
        .get("gotenberg-output-filename")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let mut res = next.run(req).await;
    if let Some(id) = res.headers().get(REQUEST_ID).cloned() {
        res.headers_mut().insert("gotenberg-trace", id);
    }
    if let Some(name) = wanted
        && let Some(current) = res
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split("filename=\"").nth(1))
            .map(|v| v.trim_end_matches('"').to_string())
    {
        let ext = std::path::Path::new(&current)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let base = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&name)
            .replace('"', "");
        let renamed = if std::path::Path::new(&base).extension().is_some() {
            base
        } else {
            format!("{base}{ext}")
        };
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{renamed}\"")) {
            res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
    }
    res
}

/// Cross-origin access is off unless `RUSTICDOC_CORS_ORIGINS` lists origins,
/// or `*` for any.
fn cors(config: &rusticdoc_core::Config) -> CorsLayer {
    use tower_http::cors::{AllowOrigin, Any};
    if config.cors_origins.is_empty() {
        return CorsLayer::new();
    }
    let origin = if config.cors_origins.iter().any(|o| o == "*") {
        AllowOrigin::any()
    } else {
        AllowOrigin::list(
            config
                .cors_origins
                .iter()
                .filter_map(|o| o.parse::<HeaderValue>().ok()),
        )
    };
    CorsLayer::new()
        .allow_origin(origin)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers([header::CONTENT_DISPOSITION, REQUEST_ID])
}

/// Binds, serves, and stops cleanly on SIGINT or SIGTERM.
pub async fn serve(state: AppState) -> std::io::Result<()> {
    let bind = state.config.bind.clone();
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, version = rusticdoc_core::VERSION, engines = %state.engines.describe(), "rusticdoc listening");
    // Start the LibreOffice workers now rather than on the first request,
    // so the first conversions are as fast as the rest.
    if let Some(lo) = state.engines.libreoffice.clone() {
        tokio::spawn(async move {
            match lo.warm_up().await {
                Ok(()) => tracing::info!("LibreOffice workers warm"),
                Err(e) => {
                    tracing::warn!(error = %e, "LibreOffice warm-up failed; workers start on demand")
                }
            }
        });
    }
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown requested; finishing in-flight requests");
}
