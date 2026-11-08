pub mod convert;
pub mod fonts;
pub mod form;
pub mod forms;
pub mod health;
pub mod options;
pub mod pdf;
pub mod pdfops;
pub mod render;
pub mod templates;

use axum::Router;
use axum::http::{HeaderValue, header};
use axum::routing::{delete, get, post};
use rusticdoc_core::Config;
use tower_http::set_header::SetResponseHeaderLayer;
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use crate::openapi::ApiDoc;
use crate::state::AppState;

/// The service's routes behind authentication, less the groups the
/// configuration switches off.
pub fn router(config: &Config) -> Router<AppState> {
    let mut router = Router::new()
        .route("/v1/render", post(render::render))
        .route("/v1/templates/validate", post(render::validate))
        .route("/v1/version", get(health::version))
        .route(
            "/v1/templates",
            post(templates::upload).get(templates::list),
        )
        .route(
            "/v1/templates/{id}",
            get(templates::show).delete(templates::remove),
        )
        .route("/v1/templates/{id}/content", get(templates::content))
        .route("/v1/fonts", get(fonts::list).post(fonts::upload))
        .route("/v1/fonts/{id}", delete(fonts::remove))
        .route("/fonts", get(fonts::list))
        .route("/v1/fonts/reload", post(fonts::reload))
        .route("/fonts/reload", post(fonts::reload));
    if !config.pdf_disabled {
        router = router
            .route("/v1/pdf/merge", post(pdf::merge))
            .route("/v1/pdf/encrypt", post(pdf::encrypt))
            .route("/v1/pdf/sign", post(pdfops::sign))
            .route("/v1/pdf/split", post(pdfops::split))
            .route("/v1/pdf/flatten", post(pdfops::flatten))
            .route("/v1/pdf/rotate", post(pdfops::rotate))
            .route("/v1/pdf/optimize", post(pdfops::optimize))
            .route("/v1/pdf/metadata/read", post(pdfops::metadata_read))
            .route("/v1/pdf/metadata/write", post(pdfops::metadata_write))
            .route("/v1/pdf/bookmarks/read", post(pdfops::bookmarks_read))
            .route("/v1/pdf/bookmarks/write", post(pdfops::bookmarks_write))
            .route("/v1/pdf/embed", post(pdfops::embed))
            .route("/v1/pdf/watermark", post(pdfops::watermark))
            .route("/v1/pdf/stamp", post(pdfops::stamp))
            .route("/v1/pdf/factur-x", post(pdfops::facturx))
            .route("/v1/pdf/forms/fields", post(forms::fields))
            .route("/v1/pdf/forms/fill", post(forms::fill))
            .route("/v1/pdf/overlay", post(forms::overlay));
        if !config.convert_disabled {
            router = router.route("/v1/pdf/convert", post(convert::convert_pdf));
        }
    }
    if config.metrics_guarded() {
        router = router.route("/metrics", get(health::metrics));
    }
    router
}

/// The routes that can run many documents, under the batch time limit. Each job
/// inside still has its own, shorter limit when it is a single document.
pub fn long_router(config: &Config) -> Router<AppState> {
    let mut router = Router::new().route("/v1/render/batch", post(render::batch));
    if !config.convert_disabled {
        router = router.route("/v1/convert", post(convert::convert));
    }
    router
}

/// Routes that stay open: liveness, readiness and the API description, and
/// `/metrics` when nothing else is guarded or the operator says so. `/docs` is
/// opt-in, because it loads a third-party script on the API's origin.
pub fn public(config: &Config) -> Router<AppState> {
    let mut router = Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready))
        .route("/openapi.json", get(health::openapi));
    if !config.metrics_guarded() {
        router = router.route("/metrics", get(health::metrics));
    }
    if config.enable_docs {
        let docs: Router<AppState> = Scalar::with_url("/docs", ApiDoc::openapi()).into();
        router = router.merge(docs.layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(DOCS_POLICY),
        )));
    }
    router
}

/// What the docs page may do: load its one script and styles, and talk to this
/// service.
const DOCS_POLICY: &str = "default-src 'none'; script-src https://cdn.jsdelivr.net; style-src 'unsafe-inline' https://cdn.jsdelivr.net; font-src https://fonts.scalar.com data:; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
