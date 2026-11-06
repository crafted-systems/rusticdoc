//! Fonts are data: list what is loaded and pick up a changed mount without
//! restarting the container.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use rusticdoc_core::fonts::{self, FontInfo};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult, ErrorBody};
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
    let list = tokio::task::spawn_blocking(move || fonts::list(&dir))
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

/// Lists the fonts LibreOffice and the PDF stage can use.
#[utoipa::path(
    get, path = "/v1/fonts", tag = "fonts",
    responses((status = 200, description = "The loaded fonts", body = Vec<FontSchema>)),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<FontInfo>>> {
    Ok(Json(loaded(&state).await?.as_ref().clone()))
}

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
    let dir = state.config.fonts_dir.clone();
    let signature = tokio::task::spawn_blocking(move || fonts::directory_signature(&dir))
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
