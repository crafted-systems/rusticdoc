//! Assets are data: the images and stylesheets templates and pages name, from a
//! mounted directory or uploaded to the store.

use std::collections::HashMap;

use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use rusticdoc_core::assets::{self, AssetKind};
use rusticdoc_core::fonts::SkippedFile;
use rusticdoc_core::store::AssetMeta;
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::routes::fonts::SkippedSchema;
use crate::routes::form::{Form, disposition};
use crate::routes::templates::{store_of, user_store_error};
use crate::state::AppState;

/// An asset templates and pages can name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssetSchema {
    /// What templates and pages call it, its folders included.
    #[schema(example = "brand/logo.png")]
    pub name: String,
    /// `image` or `stylesheet`.
    #[serde(rename = "type")]
    #[schema(value_type = String)]
    pub kind: AssetKind,
    pub size: u64,
    /// True when it is in the mounted directory, which wins over a stored asset
    /// of the same name.
    pub mounted: bool,
    /// RFC 3339. Absent for mounted assets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<String>,
}

impl AssetSchema {
    fn stored(meta: AssetMeta) -> Option<Self> {
        Some(Self {
            kind: assets::kind_of(&meta.name)?,
            name: meta.name,
            size: meta.size,
            mounted: false,
            uploaded_at: Some(meta.uploaded_at),
        })
    }
}

/// The multipart form `POST /v1/assets` accepts.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct AssetUploadForm {
    /// One PNG, JPEG, GIF, SVG, WebP or CSS file, or a ZIP of them, where each
    /// file's path is its name.
    #[schema(value_type = String, format = Binary)]
    pub file: String,
    /// The name to store one file under, with folders if it has any, such as
    /// `brand/logo.png`. The file's own name by default.
    pub name: Option<String>,
}

/// What an upload stored.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssetsStored {
    pub assets: Vec<AssetSchema>,
    /// Files in the ZIP that were left out, and why.
    #[schema(value_type = Vec<SkippedSchema>)]
    pub skipped: Vec<SkippedFile>,
    /// In use on this replica now, and on the others within this many seconds.
    pub sync_interval_seconds: u64,
}

fn io_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "io_error", e.to_string())
}

/// List assets
///
/// Lists the images and stylesheets templates and pages can name: those in the
/// mounted directory and, with a store, the uploaded ones.
#[utoipa::path(
    get, path = "/v1/assets", tag = "assets", operation_id = "listAssets",
    responses(
        (status = 200, description = "The assets, by name", body = Vec<AssetSchema>),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []))
)]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<AssetSchema>>> {
    let dirs = state.assets.clone();
    let mounted = tokio::task::spawn_blocking(move || dirs.list_mounted())
        .await
        .map_err(io_error)?;
    let mut list: Vec<AssetSchema> = mounted
        .into_iter()
        .filter_map(|asset| {
            Some(AssetSchema {
                kind: assets::kind_of(&asset.name)?,
                name: asset.name,
                size: asset.size,
                mounted: true,
                uploaded_at: None,
            })
        })
        .collect();
    if let Some(store) = &state.store {
        list.extend(
            store
                .list_assets()
                .await?
                .into_iter()
                .filter_map(AssetSchema::stored),
        );
    }
    list.sort_by(|a, b| (&a.name, b.mounted).cmp(&(&b.name, a.mounted)));
    Ok(Json(list))
}

/// Upload assets
///
/// Stores images and stylesheets for every caller and every replica: one file, or
/// a ZIP. An asset stored again under the same name is replaced. They are in use on
/// this replica when this returns, and on the others within `syncIntervalSeconds`.
#[utoipa::path(
    post, path = "/v1/assets", tag = "assets", operation_id = "uploadAssets",
    request_body(content_type = "multipart/form-data", content = AssetUploadForm),
    responses(
        (status = 201, description = "Stored", body = AssetsStored),
        (status = 400, description = "Not assets, a bad name, or too large", body = ErrorBody),
        (status = 403, description = "The store is full", body = ErrorBody),
        (status = 409, description = "A mounted asset has the name", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []))
)]
pub async fn upload(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    multipart: Multipart,
) -> ApiResult<Response> {
    let store = store_of(&state)?;
    let _one_at_a_time = state
        .asset_uploads
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "busy", "shutting down"))?;
    let mut form = Form::read(multipart, &state).await?;
    let file = match form.files.len() {
        1 => form.files.remove(0),
        0 => {
            return Err(ApiError::bad_request(
                "no file in the form; send an image, a stylesheet or a ZIP",
            ));
        }
        _ => {
            return Err(ApiError::bad_request(
                "send one file: put several assets in a ZIP",
            ));
        }
    };
    let name = form
        .field("name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    let dirs = state.assets.clone();
    let (unpacked, taken) = tokio::task::spawn_blocking(move || {
        let unpacked = assets::unpack_upload(&file, name.as_deref())?;
        let taken = unpacked
            .assets
            .iter()
            .find(|a| dirs.mounted_file(&a.name).is_some())
            .map(|a| a.name.clone());
        Ok::<_, rusticdoc_core::Error>((unpacked, taken))
    })
    .await
    .map_err(io_error)??;
    // It would be stored and never used.
    if let Some(name) = taken {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "asset_mounted",
            format!("'{name}' is in the mounted directory, which wins over a stored asset"),
        ));
    }

    // What the store would hold, with the replaced assets counted once.
    let mut held: HashMap<String, u64> = store
        .list_assets()
        .await?
        .into_iter()
        .map(|meta| (meta.name, meta.size))
        .collect();
    for asset in &unpacked.assets {
        held.insert(asset.name.clone(), asset.bytes.len() as u64);
    }
    let bytes: u64 = held.values().sum();
    if held.len() > state.config.max_stored_assets || bytes > state.config.max_asset_storage {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "quota_exceeded",
            format!(
                "the store would hold {} assets of {bytes} bytes, past its limit of {} assets or {} bytes",
                held.len(),
                state.config.max_stored_assets,
                state.config.max_asset_storage
            ),
        ));
    }
    let mut stored = Vec::with_capacity(unpacked.assets.len());
    for asset in unpacked.assets {
        let meta = store
            .put_asset(&asset.name, asset.bytes)
            .await
            .map_err(user_store_error)?;
        tracing::info!(
            log_type = "audit", action = "asset.upload", caller = %principal.0,
            asset = %meta.name, size = meta.size, "asset stored"
        );
        stored.extend(AssetSchema::stored(meta));
    }
    crate::assetsync::run_once(&state).await?;
    Ok((
        StatusCode::CREATED,
        Json(AssetsStored {
            assets: stored,
            skipped: unpacked.skipped,
            sync_interval_seconds: state.config.store_sync_interval.as_secs(),
        }),
    )
        .into_response())
}

/// Download an asset
///
/// Downloads the asset a name stands for: the mounted one, else the stored one.
#[utoipa::path(
    get, path = "/v1/assets/{name}", tag = "assets", operation_id = "downloadAsset",
    params(("name" = String, Path, description = "The asset's name, folders included, from `GET /v1/assets`")),
    responses(
        (status = 200, description = "The file", content_type = "application/octet-stream", body = crate::error::BinaryFile),
        (status = 404, description = "No such asset", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []))
)]
pub async fn content(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    let mounted = assets::AssetDirs {
        stored: None,
        ..state.assets.clone()
    };
    let wanted = name.clone();
    let found = tokio::task::spawn_blocking(move || mounted.read(&wanted))
        .await
        .map_err(io_error)?
        .map_err(io_error)?;
    let bytes = match (found, &state.store) {
        (Some(bytes), _) => bytes.into(),
        (None, Some(store)) => store.asset(&name).await.map_err(user_store_error)?,
        (None, None) => {
            return Err(user_store_error(
                rusticdoc_core::store::StoreError::NotFound,
            ));
        }
    };
    // Always a download: an SVG must not run on this origin.
    let file = name.rsplit('/').next().unwrap_or(&name);
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_DISPOSITION, disposition(file)),
        ],
        bytes,
    )
        .into_response())
}

/// Delete an asset
///
/// Deletes a stored asset. It leaves this replica when this returns, and the
/// others within the sync interval. A mounted asset cannot be deleted this way.
#[utoipa::path(
    delete, path = "/v1/assets/{name}", tag = "assets", operation_id = "deleteAsset",
    params(("name" = String, Path, description = "The asset's name, folders included, from `GET /v1/assets`")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "No such stored asset", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []))
)]
pub async fn remove(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let store = store_of(&state)?;
    store.delete_asset(&name).await.map_err(user_store_error)?;
    tracing::info!(
        log_type = "audit", action = "asset.delete", caller = %principal.0,
        asset = %name, "asset deleted"
    );
    crate::assetsync::run_once(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}
