//! `/forms/pdfengines/*`: every route takes many PDFs and answers with one
//! file or a ZIP.

use std::collections::BTreeMap;

use axum::extract::{Request, State};
use axum::response::Response;
use rusticdoc_core::compat::{Bookmarks, Compat, PdfFormats, PostSteps, facturx_formats};
use rusticdoc_core::pdfops::require_pdf;
use rusticdoc_core::{Document, Error};

use super::form::{self, FormData};
use super::{Reply, client_error, engines, run};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const PDF: &[&str] = &[".pdf"];

/// The text for a format no engine can apply.
const NO_FORMATS: &str = "Invalid form data: either 'pdfa' or 'pdfua' form fields must be provided";

/// Inputs are told apart by extension, as Gotenberg does, and then checked: a
/// file that merely calls itself a PDF never reaches an engine.
fn checked(docs: Vec<Document>) -> ApiResult<Vec<Document>> {
    for doc in &docs {
        require_pdf(doc).map_err(client_error)?;
    }
    Ok(docs)
}

fn done(result: rusticdoc_core::Result<Vec<Document>>) -> ApiResult<Reply> {
    result.map(Reply::Files).map_err(client_error)
}

pub async fn merge(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let steps = form::read_steps(&mut form, true)?;
        let inputs = form.mandatory_paths(PDF);
        let flatten = form.boolean("flatten", false);
        let auto_index = form.boolean("autoIndexBookmarks", false);
        let titles = form.boolean("titleBookmarks", false);
        form.validate()?;
        let bookmarks = steps.bookmarks.clone();
        let steps = steps.bind(flatten)?;
        let inputs = checked(inputs)?;
        done(
            engines(&state)
                .merge(inputs, &steps, &bookmarks, auto_index, titles)
                .await,
        )
    })
    .await
}

pub async fn split(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let split = form::read_split(&mut form, true);
        let steps = form::read_steps(&mut form, false)?;
        let inputs = form.mandatory_paths(PDF);
        let flatten = form.boolean("flatten", false);
        form.validate()?;
        let steps = steps.bind(flatten)?;
        let inputs = checked(inputs)?;
        let Some(split) = split else {
            return Ok(Reply::Files(inputs));
        };
        done(engines(&state).split(&split, inputs, &steps).await)
    })
    .await
}

pub async fn flatten(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        done(engines(&state).flatten_stub(checked(inputs)?).await)
    })
    .await
}

pub async fn optimize(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        // This route optimizes either way, so only the quality is read.
        let (_, quality) = form::read_optimize(&mut form);
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        done(
            engines(&state)
                .optimize_stub(quality, checked(inputs)?)
                .await,
        )
    })
    .await
}

pub async fn convert(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let formats = form::read_formats(&mut form);
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        if formats.is_empty() {
            return Err(ApiError::bad_request(NO_FORMATS));
        }
        done(
            engines(&state)
                .convert_stub(&formats, checked(inputs)?)
                .await,
        )
    })
    .await
}

pub async fn metadata_read(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        let compat = engines(&state);
        let mut all = BTreeMap::new();
        for doc in checked(inputs)? {
            let metadata = compat
                .engines
                .forms
                .read_exif(&doc, compat.timeout)
                .await
                .map_err(client_error)?;
            all.insert(doc.name, metadata);
        }
        let value = serde_json::to_value(all).map_err(|e| {
            ApiError::from(Error::Engine {
                engine: "metadata",
                message: e.to_string(),
            })
        })?;
        Ok(Reply::Json(value))
    })
    .await
}

pub async fn metadata_write(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let metadata = form::read_metadata(&mut form, true);
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        done(
            engines(&state)
                .write_metadata_stub(metadata.as_ref(), checked(inputs)?)
                .await,
        )
    })
    .await
}

pub async fn bookmarks_read(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        let compat = engines(&state);
        let mut all = BTreeMap::new();
        for doc in checked(inputs)? {
            let marks = compat
                .engines
                .forms
                .read_bookmarks(&doc, compat.timeout)
                .await
                .map_err(client_error)?;
            all.insert(doc.name, marks);
        }
        let value = serde_json::to_value(all).map_err(|e| {
            ApiError::from(Error::Engine {
                engine: "bookmarks",
                message: e.to_string(),
            })
        })?;
        Ok(Reply::Json(value))
    })
    .await
}

pub async fn bookmarks_write(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let bookmarks: Bookmarks = form::read_bookmarks(&mut form, true);
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        done(
            engines(&state)
                .write_bookmarks_stub(&bookmarks, checked(inputs)?)
                .await,
        )
    })
    .await
}

pub async fn encrypt(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let encrypt = form::read_encrypt(&mut form);
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        if encrypt.user_password.is_empty() && encrypt.owner_password.is_empty() {
            return Err(ApiError::bad_request(
                "Invalid form data: a 'userPassword' or 'ownerPassword' is required",
            ));
        }
        done(
            engines(&state)
                .encrypt_stub(&encrypt, checked(inputs)?)
                .await,
        )
    })
    .await
}

pub async fn embed(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let embeds = form::read_embeds(&mut form);
        let metadata = form::read_embeds_metadata(&mut form);
        let facturx = form::read_facturx(&mut form);
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        let facturx = facturx.validated()?;
        done(
            engines(&state)
                .embed(
                    checked(inputs)?,
                    &PdfFormats::default(),
                    &embeds,
                    &metadata,
                    facturx.as_ref(),
                )
                .await,
        )
    })
    .await
}

/// A watermark or a stamp, which only differ in which side of the content the
/// mark goes.
async fn marks(state: AppState, mut form: FormData, kind: &'static str) -> ApiResult<Reply> {
    let raw = form::read_overlays(&form, kind)?;
    let inputs = form.mandatory_paths(PDF);
    let files = form.take_files(kind);
    form.validate()?;
    if raw.is_empty() {
        return Err(ApiError::bad_request(format!(
            "Invalid form data: form field '{kind}Source' is required"
        )));
    }
    let overlays = form::bind_overlays(raw, files, kind)?;
    let inputs = checked(inputs)?;
    let compat: Compat<'_> = engines(&state);
    done(if kind == "stamp" {
        compat.stamp_stub(&overlays, inputs).await
    } else {
        compat.watermark_stub(&overlays, inputs).await
    })
}

pub async fn watermark(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, form| marks(state, form, "watermark")).await
}

pub async fn stamp(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, form| marks(state, form, "stamp")).await
}

pub async fn rotate(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let rotate = form::read_rotate(&mut form, true);
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        let inputs = checked(inputs)?;
        let Some((rotation, selection)) = rotate else {
            return Ok(Reply::Files(inputs));
        };
        done(
            engines(&state)
                .rotate_stub(rotation, &selection, inputs)
                .await,
        )
    })
    .await
}

pub async fn facturx(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, |state, mut form| async move {
        let formats = form::read_formats(&mut form);
        let facturx = form::read_facturx(&mut form);
        let inputs = form.mandatory_paths(PDF);
        form.validate()?;
        if facturx.level.is_empty() || facturx.xml.is_none() {
            return Err(ApiError::bad_request(
                "Invalid form data: 'facturxConformanceLevel' and 'facturxXml' are both required",
            ));
        }
        let facturx = facturx.validated()?;
        let steps = PostSteps {
            formats: formats.clone(),
            facturx: facturx.clone(),
            ..PostSteps::default()
        };
        rusticdoc_core::compat::validate_steps(&steps).map_err(client_error)?;
        let inputs = checked(inputs)?;
        let formats = facturx_formats(true, &formats, false, &inputs, state.config.api_timeout)
            .await
            .map_err(client_error)?;
        done(
            engines(&state)
                .embed(inputs, &formats, &[], &BTreeMap::new(), facturx.as_ref())
                .await,
        )
    })
    .await
}
