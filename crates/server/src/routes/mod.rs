pub mod convert;
pub mod fonts;
pub mod form;
pub mod forms;
pub mod gotenberg;
pub mod health;
pub mod pdf;
pub mod pdfops;
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
        .route("/v1/fonts", get(fonts::list))
        .route("/fonts", get(fonts::list))
        .route("/v1/fonts/reload", post(fonts::reload))
        .route("/fonts/reload", post(fonts::reload))
        .route("/v1/pdf/sign", post(pdfops::sign))
        .route("/v1/pdf/convert", post(convert::convert_pdf))
        .route("/forms/pdfengines/convert", post(convert::convert_pdf))
        .route("/v1/pdf/split", post(pdfops::split))
        .route("/forms/pdfengines/split", post(pdfops::split))
        .route("/v1/pdf/flatten", post(pdfops::flatten))
        .route("/forms/pdfengines/flatten", post(pdfops::flatten))
        .route("/v1/pdf/rotate", post(pdfops::rotate))
        .route("/forms/pdfengines/rotate", post(pdfops::rotate))
        .route("/v1/pdf/optimize", post(pdfops::optimize))
        .route("/forms/pdfengines/optimize", post(pdfops::optimize))
        .route("/v1/pdf/metadata/read", post(pdfops::metadata_read))
        .route(
            "/forms/pdfengines/metadata/read",
            post(pdfops::metadata_read),
        )
        .route("/v1/pdf/metadata/write", post(pdfops::metadata_write))
        .route(
            "/forms/pdfengines/metadata/write",
            post(pdfops::metadata_write),
        )
        .route("/v1/pdf/bookmarks/read", post(pdfops::bookmarks_read))
        .route(
            "/forms/pdfengines/bookmarks/read",
            post(pdfops::bookmarks_read),
        )
        .route("/v1/pdf/bookmarks/write", post(pdfops::bookmarks_write))
        .route(
            "/forms/pdfengines/bookmarks/write",
            post(pdfops::bookmarks_write),
        )
        .route("/v1/pdf/embed", post(pdfops::embed))
        .route("/forms/pdfengines/embed", post(pdfops::embed))
        .route("/v1/pdf/watermark", post(pdfops::watermark))
        .route("/forms/pdfengines/watermark", post(pdfops::watermark))
        .route("/v1/pdf/stamp", post(pdfops::stamp))
        .route("/forms/pdfengines/stamp", post(pdfops::stamp))
        .route("/v1/pdf/factur-x", post(pdfops::facturx))
        .route("/forms/pdfengines/factur-x", post(pdfops::facturx))
        .route("/v1/pdf/forms/fields", post(forms::fields))
        .route("/v1/pdf/forms/fill", post(forms::fill))
        .route("/v1/pdf/overlay", post(forms::overlay))
}

/// Routes that stay open: liveness, readiness, version, metrics and the API
/// description.
pub fn public() -> Router<AppState> {
    Router::new()
        .route("/health", get(health::health))
        .route(
            "/favicon.ico",
            get(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route("/ready", get(health::ready))
        .route("/version", get(health::version))
        .route("/metrics", get(health::metrics))
        .route("/prometheus/metrics", get(health::metrics))
        .route("/openapi.json", get(health::openapi))
        .merge(Scalar::with_url("/docs", ApiDoc::openapi()))
}
