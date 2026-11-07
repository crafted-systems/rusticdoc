use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

use crate::auth;
use crate::openapi::ApiDoc;
use crate::state::AppState;

#[derive(Serialize, ToSchema)]
pub struct Health {
    /// `up` when the service answers.
    #[schema(example = "up")]
    pub status: &'static str,
    pub uptime_seconds: u64,
    pub engines: EngineStatus,
    /// Per-module status in Gotenberg's shape.
    pub details: serde_json::Value,
}

#[derive(Serialize, ToSchema)]
pub struct EngineStatus {
    pub libreoffice: bool,
    pub qpdf: bool,
}

#[derive(Serialize, ToSchema)]
pub struct Version {
    #[schema(example = "0.1.0")]
    pub version: &'static str,
    /// Paths of the engines in use, or null when absent.
    pub engines: serde_json::Value,
}

/// Liveness. With credentials configured, a caller without them sees the status
/// and nothing else.
#[utoipa::path(get, path = "/health", tag = "service", responses((status = 200, description = "Service status", body = Health)))]
pub async fn health(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Pool numbers and engines are for callers the service knows.
    let open = !state.config.auth_configured() || auth::identify(&state, &headers).await.is_some();
    let mut res = if open {
        Json(status(&state).await).into_response()
    } else {
        Json(serde_json::json!({ "status": "up" })).into_response()
    };
    // Gotenberg's clients match on this exact value.
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    res
}

async fn status(state: &AppState) -> Health {
    let pool = match &state.engines.libreoffice {
        Some(lo) => lo.health().await,
        None => serde_json::Value::Null,
    };
    let mut details = serde_json::json!({
        "libreoffice": {
            "status": if state.engines.libreoffice.is_some() { "up" } else { "down" },
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            "pool": pool,
        }
    });
    // Only with a store: Gotenberg's clients read this object.
    if let Some(store) = &state.store {
        details["store"] = serde_json::json!({
            "location": store.description(),
            "fontsSynced": state.fonts_synced.load(Ordering::Acquire),
        });
    }
    Health {
        status: "up",
        uptime_seconds: state.started.elapsed().as_secs(),
        engines: EngineStatus {
            libreoffice: state.engines.libreoffice.is_some(),
            qpdf: state.engines.pdf.names().contains(&"qpdf"),
        },
        details,
    }
}

/// Readiness: liveness, and with a store, this replica's copy of the stored fonts
/// being complete. A load balancer should send no documents here before that,
/// because they would quietly come out in a fallback font.
#[utoipa::path(get, path = "/ready", tag = "service", responses((status = 200, description = "Service status", body = Health)))]
pub async fn ready(state: State<AppState>, headers: HeaderMap) -> Response {
    if state.store.is_some() && !state.fonts_synced.load(Ordering::Acquire) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "status": "syncing" })),
        )
            .into_response();
    }
    health(state, headers).await
}

/// Engine versions and paths.
#[utoipa::path(get, path = "/v1/version", tag = "service", responses((status = 200, description = "Version and engines", body = Version)), security(("api_key" = []), ("bearer" = []), ("basic" = [])))]
pub async fn version(State(state): State<AppState>) -> Json<Version> {
    Json(Version {
        version: rusticdoc_core::VERSION,
        engines: state.engines.describe(),
    })
}

/// Prometheus text format. With credentials configured it needs them too, unless
/// `RUSTICDOC_METRICS_PUBLIC` is set.
#[utoipa::path(get, path = "/metrics", tag = "service", responses((status = 200, description = "Prometheus text format", content_type = "text/plain")))]
pub async fn metrics(State(state): State<AppState>) -> Response {
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render(),
    )
        .into_response()
}

/// The OpenAPI 3.1 description, the source for generated SDKs.
pub async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
