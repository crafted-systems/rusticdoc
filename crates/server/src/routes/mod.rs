pub mod convert;
pub mod form;
pub mod health;
pub mod pdf;
pub mod render;

use axum::Router;
use axum::routing::{get, post};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use crate::openapi::ApiDoc;
use crate::state::AppState;

/// Routes behind authentication. Gotenberg's paths are aliases of the `/v1`
/// ones, so its clients work unchanged.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/render", post(render::render))
        .route("/v1/render/batch", post(render::batch))
        .route("/v1/templates/validate", post(render::validate))
        .route("/v1/convert", post(convert::convert))
        .route("/forms/libreoffice/convert", post(convert::convert))
        .route("/v1/pdf/merge", post(pdf::merge))
        .route("/forms/pdfengines/merge", post(pdf::merge))
        .route("/v1/pdf/encrypt", post(pdf::encrypt))
        .route("/forms/pdfengines/encrypt", post(pdf::encrypt))
}

/// Routes that stay open: liveness, readiness, version, metrics and the API
/// description.
pub fn public() -> Router<AppState> {
    Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready))
        .route("/version", get(health::version))
        .route("/metrics", get(health::metrics))
        .route("/prometheus/metrics", get(health::metrics))
        .route("/openapi.json", get(health::openapi))
        .merge(Scalar::with_url("/docs", ApiDoc::openapi()))
}
