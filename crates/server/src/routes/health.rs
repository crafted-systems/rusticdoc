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
#[utoipa::path(get, path = "/health", tag = "service", responses((status = 200, body = Health)))]
pub async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        status: "up",
        uptime_seconds: state.started.elapsed().as_secs(),
        engines: EngineStatus {
            libreoffice: state.engines.libreoffice.is_some(),
            qpdf: state.engines.qpdf.is_some(),
        },
    })
}

/// Readiness: the same as liveness today, kept separate so a warm-up can be
/// added without clients changing.
#[utoipa::path(get, path = "/ready", tag = "service", responses((status = 200, body = Health)))]
pub async fn ready(state: State<AppState>) -> Json<Health> {
    health(state).await
}

/// Engine versions and paths.
#[utoipa::path(get, path = "/version", tag = "service", responses((status = 200, body = Version)))]
pub async fn version(State(state): State<AppState>) -> Json<Version> {
    Json(Version {
        version: rusticdoc_core::VERSION,
        engines: state.engines.describe(),
    })
}

/// Prometheus text format.
#[utoipa::path(get, path = "/metrics", tag = "service", responses((status = 200, content_type = "text/plain")))]
pub async fn metrics(State(state): State<AppState>) -> Response {
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/openmetrics-text; version=1.0.0; charset=utf-8",
        )],
        state.metrics.render(),
    )
        .into_response()
}

/// The OpenAPI 3.1 description, the source for generated SDKs.
pub async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
