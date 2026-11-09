//! Background jobs, for a caller that cannot host a webhook receiver: send a
//! request with `Prefer: respond-async`, get 202 and a `Location`, and ask there
//! until the job is done or failed.

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use rusticdoc_core::store::JobStatus;
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::jobs::{current, scope_of};
use crate::routes::form::disposition;
use crate::routes::templates::{store_of, user_store_error};
use crate::state::AppState;

/// The file a finished job made.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct JobResultSchema {
    #[schema(example = "invoice.pdf")]
    pub name: String,
    #[schema(example = "application/pdf")]
    pub content_type: String,
    pub size: u64,
    /// Hex SHA-256 of the content.
    pub sha256: String,
}

/// A background job.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct JobSchema {
    #[schema(example = "01966f3e-7c1a-7d3c-9a44-1d2f5a6b7c8d")]
    pub id: String,
    /// `queued`, `running`, `done` or `failed`.
    #[schema(example = "done")]
    pub status: String,
    /// RFC 3339, to the millisecond.
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// When the job and its result are removed.
    pub expires_at: Option<String>,
    /// A job that has not ended by then is reported as failed with
    /// `job_lost`: the container running it stopped.
    pub runs_until: String,
    /// The `X-Request-Id` of the request that started it.
    pub request_id: String,
    /// Present once it is done; download it from `/v1/jobs/{id}/result`.
    pub result: Option<JobResultSchema>,
    /// Present once it failed: what the request would have answered, as
    /// `error`, `message` and `status`.
    #[schema(value_type = Option<Object>)]
    pub error: Option<serde_json::Value>,
}

/// Where a background job is. Ask again until `status` is `done` or `failed`;
/// `Retry-After` says how soon. A job and its result are kept for
/// `RUSTICDOC_JOB_TTL` after it ends, then removed.
#[utoipa::path(
    get, path = "/v1/jobs/{id}", tag = "jobs", operation_id = "getJob",
    params(("id" = String, Path, description = "The job's id, from the 202's body or Location")),
    responses(
        (status = 200, description = "The job", body = JobSchema),
        (status = 404, description = "This caller has no such job, or it expired", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn show(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let store = store_of(&state)?;
    let record = current(&state, &store, &scope_of(&principal.0), &id).await?;
    let ended = record.status.ended();
    let mut res = Json(record).into_response();
    if !ended {
        res.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    Ok(res)
}

/// Downloads what a finished job made, as the request would have answered it.
#[utoipa::path(
    get, path = "/v1/jobs/{id}/result", tag = "jobs", operation_id = "getJobResult",
    params(("id" = String, Path, description = "The job's id")),
    responses(
        (status = 200, description = "The file", content_type = "application/octet-stream", body = crate::error::BinaryFile),
        (status = 404, description = "This caller has no such job, or it expired", body = ErrorBody),
        (status = 409, description = "The job has not finished, or it failed", body = ErrorBody),
        (status = 501, description = "RUSTICDOC_STORE is not set", body = ErrorBody),
        (status = 503, description = "The store cannot be reached", body = ErrorBody),
    ),
    security(("api_key" = []), ("bearer" = []), ("basic" = []))
)]
pub async fn result(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let store = store_of(&state)?;
    let scope = scope_of(&principal.0);
    let record = current(&state, &store, &scope, &id).await?;
    match record.status {
        JobStatus::Done => {}
        JobStatus::Failed => {
            let message = record
                .error
                .as_ref()
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("the job failed");
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "job_failed",
                format!("the job failed, so it has no result: {message}"),
            ));
        }
        JobStatus::Queued | JobStatus::Running => {
            let mut res = ApiError::new(
                StatusCode::CONFLICT,
                "job_not_done",
                "the job has not finished; ask /v1/jobs/{id} until it is done",
            )
            .into_response();
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            return Ok(res);
        }
    }
    let bytes = store
        .job_result(&scope, &record)
        .await
        .map_err(user_store_error)?;
    let meta = record.result.as_ref();
    let content_type = meta
        .map(|m| m.content_type.clone())
        .unwrap_or_else(|| "application/octet-stream".into());
    let name = meta.map(|m| m.name.as_str()).unwrap_or("result");
    // The type is the one this service gave the file; never sniffed, always a
    // download, as a stored template is.
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_DISPOSITION, disposition(name)),
        ],
        bytes,
    )
        .into_response())
}
