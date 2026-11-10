//! Fonts are data: list what is loaded and pick up a changed mount without
//! restarting the container.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use rusticdoc_core::fonts::{self, FontInfo, SkippedFile};
use rusticdoc_core::store::FontMeta;
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::{self, Admin, Principal};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::routes::form::Form;
use crate::routes::templates::{store_of, user_store_error};
use crate::state::AppState;

#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct FontSchema {
    pub family: String,
    pub style: String,
    pub weight: u16,
    /// The file's base name.
    pub file: String,
    /// True when it came from the mounted fonts directory.
    pub mounted: bool,
    /// The stored font this face belongs to, for deleting it. Absent for fonts
    /// in the image or the mounted directory.
    pub id: Option<String>,
}

/// The multipart form `POST /v1/fonts` accepts.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct FontUploadForm {
    /// A ZIP of TTF, OTF or TTC files, or one such file.
    #[schema(value_type = String, format = Binary)]
    pub file: String,
}

#[derive(Serialize, ToSchema)]
pub struct FaceSchema {
    pub family: String,
    pub style: String,
    pub weight: u16,
}

/// A stored font.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct StoredFontSchema {
    /// The hex SHA-256 of the file, so storing it twice changes nothing.
    pub id: String,
    pub file: String,
    pub size: u64,
    pub faces: Vec<FaceSchema>,
    /// RFC 3339.
    pub uploaded_at: String,
}

#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct SkippedSchema {
    pub file: String,
    pub reason: String,
}

/// What an upload stored.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontsStored {
    #[schema(value_type = Vec<StoredFontSchema>)]
    pub fonts: Vec<FontMeta>,
    /// Files in the ZIP that were left out, and why.
    #[schema(value_type = Vec<SkippedSchema>)]
    pub skipped: Vec<SkippedFile>,
    /// In use on this replica now, and on the others within this many seconds.
    pub sync_interval_seconds: u64,
}

#[derive(Serialize, ToSchema)]
pub struct Reloaded {
    pub families: usize,
    pub faces: usize,
    /// False when the directory has not changed, so no worker restarted.
    pub changed: bool,
}

/// Restarting workers interrupts nothing but costs warm capacity, so a
/// reload acts at most this often and only when the directory changed.
const RELOAD_INTERVAL: Duration = Duration::from_secs(10);

/// The loaded fonts, scanning once and keeping the result until a reload.
pub async fn loaded(state: &AppState) -> ApiResult<Arc<Vec<FontInfo>>> {
    if let Some(list) = state.fonts.read().await.clone() {
        return Ok(list);
    }
    scan(state).await
}

async fn scan(state: &AppState) -> ApiResult<Arc<Vec<FontInfo>>> {
    let dir = state.config.fonts_dir.clone();
    let stored = state
        .store
        .as_ref()
        .map(|_| state.config.font_cache_dir.clone());
    // The copy is named by hash; clients know a font by what it was uploaded as.
    let names = state
        .store
        .as_ref()
        .map(|s| s.known_fonts())
        .unwrap_or_default();
    let list = tokio::task::spawn_blocking(move || {
        let mut list = fonts::list(&dir, stored.as_deref());
        for face in &mut list {
            if let Some(meta) = face.id.as_ref().and_then(|id| names.get(id)) {
                face.file = meta.file.clone();
            }
        }
        list
    })
    .await
    .map_err(|e| {
        ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "io_error",
            e.to_string(),
        )
    })?;
    let list = Arc::new(list);
    *state.fonts.write().await = Some(list.clone());
    Ok(list)
}

/// List fonts
///
/// Lists the fonts LibreOffice and the PDF stage can use.
#[utoipa::path(
    get, path = "/v1/fonts", tag = "fonts",
    responses((status = 200, description = "The loaded fonts", body = Vec<FontSchema>)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<FontInfo>>> {
    Ok(Json(loaded(&state).await?.as_ref().clone()))
}

/// Reload fonts
///
/// Rescans the fonts directory and restarts idle LibreOffice workers, so a
/// new font needs no container restart. Busy workers restart after their
/// current job.
#[utoipa::path(
    post, path = "/v1/fonts/reload", tag = "fonts",
    responses(
        (status = 200, description = "What is loaded now", body = Reloaded),
        (status = 429, description = "Reloaded moments ago", body = ErrorBody),
        (status = 503, description = "A worker could not restart", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn reload(State(state): State<AppState>) -> ApiResult<Json<Reloaded>> {
    // Stored fonts first, so this picks those up too on this replica.
    if state.store.is_some() {
        crate::fontsync::run_once(&state)
            .await
            .map_err(ApiError::from)?;
    }
    let dir = state.config.fonts_dir.clone();
    let stored = state.config.font_cache_dir.clone();
    let has_store = state.store.is_some();
    let signature = tokio::task::spawn_blocking(move || {
        let mounted = fonts::directory_signature(&dir);
        if has_store {
            format!("{mounted}/{}", fonts::directory_signature(&stored))
        } else {
            mounted
        }
    })
    .await
    .map_err(|e| {
        ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "io_error",
            e.to_string(),
        )
    })?;
    let changed = {
        let mut last = state.fonts_reload.lock().await;
        if let Some((_, at)) = last.as_ref()
            && at.elapsed() < RELOAD_INTERVAL
        {
            return Err(ApiError::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                "fonts were reloaded moments ago; try again in a few seconds",
            ));
        }
        let changed = last.as_ref().is_none_or(|(sig, _)| *sig != signature);
        *last = Some((signature, Instant::now()));
        changed
    };
    let list = if changed {
        scan(&state).await?
    } else {
        loaded(&state).await?
    };
    if changed && let Some(lo) = &state.engines.libreoffice {
        lo.reload_fonts().await?;
    }
    let mut families: Vec<&str> = list.iter().map(|f| f.family.as_str()).collect();
    families.sort_unstable();
    families.dedup();
    tracing::info!(
        faces = list.len(),
        families = families.len(),
        changed,
        "fonts reload"
    );
    Ok(Json(Reloaded {
        families: families.len(),
        faces: list.len(),
        changed,
    }))
}

/// Upload fonts
///
/// Stores fonts for every caller and every replica: a ZIP of TTF, OTF or TTC files,
/// or one such file. Takes an admin. They are in use on this replica when this
/// returns, and on the others within `syncIntervalSeconds`.
#[utoipa::path(
    post, path = "/v1/fonts", tag = "fonts", operation_id = "uploadFonts",
    request_body(content_type = "multipart/form-data", content = FontUploadForm),
    responses(
        (status = 201, description = "Stored", body = FontsStored),
        (status = 400, description = "Not fonts, or too large", body = ErrorBody),
        (status = 403, description = "Needs an admin, or the store is full", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn upload(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(admin): Extension<Admin>,
    multipart: Multipart,
) -> ApiResult<Response> {
    auth::require_admin(&state, admin)?;
    let store = store_of(&state)?;
    let _one_at_a_time = state
        .font_uploads
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "busy", "shutting down"))?;
    let mut form = Form::read(multipart, &state).await?;
    let file = match form.files.len() {
        1 => form.files.remove(0),
        0 => {
            return Err(ApiError::bad_request(
                "no file in the form; send a ZIP or a font",
            ));
        }
        _ => {
            return Err(ApiError::bad_request(
                "send one file: put several fonts in a ZIP",
            ));
        }
    };
    let unpacked = tokio::task::spawn_blocking(move || fonts::unpack_upload(&file))
        .await
        .map_err(|e| {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "io_error", e.to_string())
        })??;

    let usage = store.font_usage().await?;
    let added: u64 = unpacked.fonts.iter().map(|f| f.bytes.len() as u64).sum();
    if usage.count + unpacked.fonts.len() > state.config.max_stored_fonts
        || usage.bytes + added > state.config.max_font_storage
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "quota_exceeded",
            format!(
                "the store holds {} fonts of {} bytes, and this would pass its limit of {} fonts or {} bytes",
                usage.count,
                usage.bytes,
                state.config.max_stored_fonts,
                state.config.max_font_storage
            ),
        ));
    }
    let mut stored = Vec::with_capacity(unpacked.fonts.len());
    for font in &unpacked.fonts {
        stored.push(store.put_font(font).await.map_err(user_store_error)?);
    }
    for meta in &stored {
        tracing::info!(
            log_type = "audit", action = "font.upload", caller = %principal.short(),
            font = %meta.id, file = %meta.file, "font stored"
        );
    }
    crate::fontsync::run_once(&state).await?;
    Ok((
        StatusCode::CREATED,
        Json(FontsStored {
            fonts: stored,
            skipped: unpacked.skipped,
            sync_interval_seconds: state.config.store_sync_interval.as_secs(),
        }),
    )
        .into_response())
}

/// Delete a font
///
/// Deletes a stored font. Takes an admin. It leaves this replica when this
/// returns, and the others within the sync interval. Documents that name it fall
/// back to another font.
#[utoipa::path(
    delete, path = "/v1/fonts/{id}", tag = "fonts", operation_id = "deleteFont",
    params(("id" = String, Path, description = "The font's id, from `GET /v1/fonts`")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Needs an admin", body = ErrorBody),
        (status = 404, description = "No such stored font", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn remove(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(admin): Extension<Admin>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, admin)?;
    let store = store_of(&state)?;
    store.delete_font(&id).await.map_err(user_store_error)?;
    tracing::info!(
        log_type = "audit", action = "font.delete", caller = %principal.short(),
        font = %id, "font deleted"
    );
    crate::fontsync::run_once(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}
