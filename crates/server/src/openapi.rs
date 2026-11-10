//! The OpenAPI description. `rusticdoc openapi` prints it; `/openapi.json`
//! serves it; `make sdk-*` generates clients from it.

use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::error::ErrorBody;
use crate::routes::{aliases, convert, fonts, forms, health, jobs, pdf, pdfops, render, templates};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "rusticdoc",
        description = "Fills Office and PDF templates from JSON, converts documents with LibreOffice, and runs PDF operations. One multipart request, or a JSON one for a stored template, does the whole job; webhooks or background jobs deliver results asynchronously.",
        license(name = "Apache-2.0", identifier = "Apache-2.0"),
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
        pdfops::sign,
        templates::upload,
        templates::list,
        templates::show,
        templates::content,
        templates::remove,
        templates::variables,
        templates::preview,
        aliases::set,
        aliases::show,
        aliases::list,
        aliases::remove,
        jobs::show,
        jobs::result,
        fonts::list,
        fonts::upload,
        fonts::remove,
        fonts::reload,
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
        templates::TemplateUploadForm,
        templates::TemplateSchema,
        templates::TemplateList,
        templates::TemplateVariables,
        templates::PreviewForm,
        crate::variables::Variable,
        crate::variables::Kind,
        aliases::AliasSet,
        aliases::AliasSchema,
        aliases::AliasVersionSchema,
        aliases::AliasHistorySchema,
        aliases::AliasList,
        jobs::JobSchema,
        jobs::JobResultSchema,
        render::RenderJson,
        fonts::FontUploadForm,
        fonts::FontsStored,
        fonts::StoredFontSchema,
        fonts::FaceSchema,
        fonts::SkippedSchema,
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
        pdfops::MarkForm,
        pdfops::FacturxForm,
        pdfops::SignForm,
        fonts::FontSchema,
        fonts::Reloaded,
        forms::FillForm,
        forms::FieldsForm,
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
        (name = "templates", description = "Store a template, name it with an alias, and render it by id or alias from any replica"),
        (name = "jobs", description = "Requests run in the background with `Prefer: respond-async`, and their results"),
        (name = "fonts", description = "List, upload and delete fonts, and reload a mounted directory"),
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
