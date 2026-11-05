//! The OpenAPI description. `rusticdoc openapi` prints it; `/openapi.json`
//! serves it; `make sdk-*` generates clients from it.

use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::error::ErrorBody;
use crate::routes::{convert, forms, health, pdf, pdfops, render};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "rusticdoc",
        description = "Fills Office and PDF templates from JSON, converts documents with LibreOffice, and runs PDF operations. One multipart request does the whole job; webhooks deliver results asynchronously.",
        license(name = "Not yet chosen", identifier = "LicenseRef-rusticdoc"),
    ),
    paths(
        render::render,
        render::batch,
        render::validate,
        convert::convert,
        convert::convert_pdf,
        pdf::merge,
        pdf::encrypt,
        pdfops::split,
        pdfops::flatten,
        pdfops::rotate,
        pdfops::optimize,
        pdfops::metadata_read,
        pdfops::metadata_write,
        pdfops::bookmarks_read,
        pdfops::bookmarks_write,
        pdfops::embed,
        pdfops::watermark,
        pdfops::stamp,
        pdfops::facturx,
        forms::fields,
        forms::fill,
        forms::overlay,
        health::health,
        health::ready,
        health::version,
        health::metrics,
    ),
    components(schemas(
        ErrorBody,
        render::RenderForm,
        render::Validation,
        convert::ConvertForm,
        pdf::MergeForm,
        pdf::EncryptForm,
        pdfops::PdfForm,
        pdfops::SplitForm,
        pdfops::RotateForm,
        pdfops::MetadataWriteForm,
        pdfops::BookmarksWriteForm,
        pdfops::EmbedForm,
        pdfops::OverlayForm,
        pdfops::FacturxForm,
        forms::FillForm,
        forms::OverlayForm,
        forms::FieldSchema,
        health::Health,
        health::EngineStatus,
        health::Version,
    )),
    tags(
        (name = "render", description = "Templates in, documents out"),
        (name = "convert", description = "Documents in, converted documents out"),
        (name = "pdf", description = "PDF operations"),
        (name = "pdf-forms", description = "PDF templates: form fields, tags in annotations, overlays"),
        (name = "service", description = "Health, version, metrics"),
    ),
    modifiers(&Security)
)]
pub struct ApiDoc;

struct Security;

impl Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "api_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("x-api-key"))),
        );
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
        components.add_security_scheme(
            "basic",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Basic).build()),
        );
    }
}
