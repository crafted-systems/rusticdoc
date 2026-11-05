use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

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

/// Liveness.
#[utoipa::path(get, path = "/health", tag = "service", responses((status = 200, description = "Service status", body = Health)))]
pub async fn health(State(state): State<AppState>) -> Json<Health> {
    let pool = match &state.engines.libreoffice {
        Some(lo) => lo.health().await,
        None => serde_json::Value::Null,
    };
    Json(Health {
        status: "up",
        uptime_seconds: state.started.elapsed().as_secs(),
        engines: EngineStatus {
            libreoffice: state.engines.libreoffice.is_some(),
            qpdf: state.engines.pdf.names().contains(&"qpdf"),
        },
        details: serde_json::json!({
            "libreoffice": {
                "status": if state.engines.libreoffice.is_some() { "up" } else { "down" },
                "timestamp": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                "pool": pool,
            }
        }),
    })
}

/// Readiness: the same as liveness today, kept separate so a warm-up can be
/// added without clients changing.
#[utoipa::path(get, path = "/ready", tag = "service", responses((status = 200, description = "Service status", body = Health)))]
pub async fn ready(state: State<AppState>) -> Json<Health> {
    health(state).await
}

/// Engine versions and paths.
#[utoipa::path(get, path = "/version", tag = "service", responses((status = 200, description = "Version and engines", body = Version)))]
pub async fn version(State(state): State<AppState>) -> Json<Version> {
    Json(Version {
        version: rusticdoc_core::VERSION,
        engines: state.engines.describe(),
    })
}

/// Prometheus text format.
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
