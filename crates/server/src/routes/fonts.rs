//! Fonts are data: list what is loaded and pick up a changed mount without
//! restarting the container.

use std::sync::Arc;

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
}

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
        (status = 503, description = "A worker could not restart", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn reload(State(state): State<AppState>) -> ApiResult<Json<Reloaded>> {
    let list = scan(&state).await?;
    if let Some(lo) = &state.engines.libreoffice {
        lo.reload_fonts().await?;
    }
    let mut families: Vec<&str> = list.iter().map(|f| f.family.as_str()).collect();
    families.sort_unstable();
    families.dedup();
    tracing::info!(
        faces = list.len(),
        families = families.len(),
        "fonts reloaded"
    );
    Ok(Json(Reloaded {
        families: families.len(),
        faces: list.len(),
    }))
}
