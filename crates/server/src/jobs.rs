//! Background work: how many jobs may run at once, in all and per caller, and
//! jobs whose results wait in the store for the caller to fetch.
//!
//! A webhook request answers 204 at once, and an asynchronous one 202, so
//! nothing but these permits stops one caller from queuing work without end. A
//! permit lives as long as the job, through its delivery or until its result is
//! stored, because that is how long the job holds memory. Webhooks and
//! asynchronous requests share the one limit.
//!
//! An asynchronous job is asked for with `Prefer: respond-async` (RFC 7240). Its
//! states and its result are objects in the store, each written once, so any
//! replica can answer `GET /v1/jobs/{id}` and none has to be the one that ran it.
//! Without a store there is nowhere every replica can see, so the request is
//! refused rather than kept on one container. Jobs are always the caller's own,
//! whatever `RUSTICDOC_STORE_SCOPE` says: results hold the caller's data.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rusticdoc_core::Output;
use rusticdoc_core::store::{self, JobRecord, JobResult, JobStatus, Store};
use sha2::{Digest, Sha256};
use tracing::Instrument;

use crate::error::ApiError;
use crate::state::AppState;
use crate::webhook::Webhook;

/// Time past a job's own limit for storing its result, before a job that has
/// not ended counts as lost.
const STORE_GRACE: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct Running {
    total: usize,
    by_caller: HashMap<String, usize>,
}

/// The limits and what is running against them. Cheap to clone.
#[derive(Debug, Clone)]
pub struct JobLimiter {
    running: Arc<Mutex<Running>>,
    max_total: usize,
    max_per_caller: usize,
}

/// One running job's place. Dropping it frees the place.
#[derive(Debug)]
pub struct JobPermit {
    running: Arc<Mutex<Running>>,
    caller: String,
}

impl JobLimiter {
    pub fn new(max_total: usize, max_per_caller: usize) -> Self {
        Self {
            running: Arc::default(),
            max_total,
            max_per_caller,
        }
    }

    /// A place for `caller`'s job, or a 429.
    pub fn try_acquire(&self, caller: &str) -> Result<JobPermit, ApiError> {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let busy =
            |message: &str| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "too_many_jobs", message);
        if running.total >= self.max_total {
            return Err(busy(
                "the service is running as many webhook and background jobs as it may; try again later",
            ));
        }
        let mine = running.by_caller.entry(caller.to_string()).or_insert(0);
        if *mine >= self.max_per_caller {
            return Err(busy(
                "this caller is running as many webhook and background jobs as it may; try again later",
            ));
        }
        *mine += 1;
        running.total += 1;
        Ok(JobPermit {
            running: self.running.clone(),
            caller: caller.to_string(),
        })
    }
}

impl Drop for JobPermit {
    fn drop(&mut self) {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        running.total = running.total.saturating_sub(1);
        if let Some(mine) = running.by_caller.get_mut(&self.caller) {
            *mine = mine.saturating_sub(1);
            if *mine == 0 {
                running.by_caller.remove(&self.caller);
            }
        }
    }
}

/// How a request's result reaches its caller.
#[derive(Debug)]
pub enum Delivery {
    /// In the response.
    Now,
    /// To a webhook, after a 204.
    Webhook(Box<Webhook>),
    /// Into the store, for `GET /v1/jobs/{id}`, after a 202.
    Job(JobTicket),
}

/// A background job that has its place and will keep its result in the store.
#[derive(Debug)]
pub struct JobTicket {
    pub id: String,
    /// The caller's scope in the store.
    pub scope: String,
    pub store: Arc<Store>,
    pub permit: JobPermit,
}

/// Where a caller's jobs live in the store: always its own.
pub fn scope_of(principal: &str) -> String {
    store::scope_key(principal)
}

/// True when the request prefers an answer before the work is done:
/// `Prefer: respond-async`, among any other preferences.
pub fn prefers_async(headers: &HeaderMap) -> bool {
    headers
        .get_all("prefer")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split([',', ';']))
        .any(|p| p.trim().eq_ignore_ascii_case("respond-async"))
}

/// A place and an id for a job `caller` asked to run in the background, or why
/// not: no store, or too many jobs running.
pub fn ticket(state: &AppState, caller: &str) -> Result<JobTicket, ApiError> {
    let Some(store) = state.store.clone() else {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "store_not_configured",
            "background jobs keep their results in the store, and RUSTICDOC_STORE is not set; leave out 'Prefer: respond-async', or set RUSTICDOC_STORE (a directory will do for one container)",
        ));
    };
    let permit = state.jobs.try_acquire(caller)?;
    Ok(JobTicket {
        id: store::new_job_id(),
        scope: scope_of(caller),
        store,
        permit,
    })
}

/// Records the job as queued, starts it, and answers 202 with where to look.
pub async fn start<F>(
    state: AppState,
    ticket: JobTicket,
    request_id: String,
    limit: Duration,
    job: F,
) -> Result<Response, ApiError>
where
    F: std::future::Future<Output = Result<Output, ApiError>> + Send + 'static,
{
    let record = JobRecord::queued(ticket.id.clone(), request_id, limit + STORE_GRACE);
    ticket.store.put_job(&ticket.scope, &record).await?;
    tracing::info!(job = %ticket.id, "background job queued");
    let response = accepted(&state, &record);
    let span = tracing::Span::current();
    tokio::spawn(run(state, ticket, record, job).instrument(span));
    Ok(response)
}

/// Runs the job and stores how it ended. The place is given back once the
/// result is stored, not before: until then the job holds it in memory.
async fn run<F>(state: AppState, ticket: JobTicket, mut record: JobRecord, job: F)
where
    F: std::future::Future<Output = Result<Output, ApiError>> + Send + 'static,
{
    let JobTicket {
        scope,
        store,
        permit,
        ..
    } = ticket;
    let ttl = state.config.job_ttl;
    record.status = JobStatus::Running;
    record.started_at = Some(store::job_time(chrono_now()));
    if let Err(e) = store.put_job(&scope, &record).await {
        tracing::warn!(job = %record.id, error = %e, "could not record the job as running");
    }
    let outcome = match job.await {
        Ok(out) => {
            state.metrics.record_timings(&out.timings);
            let result = JobResult {
                name: out.name.clone(),
                content_type: out.content_type.to_string(),
                size: out.bytes.len() as u64,
                sha256: hex::encode(Sha256::digest(&out.bytes)),
            };
            match store.put_job_result(&scope, &record.id, out.bytes).await {
                Ok(()) => Ok(result),
                Err(e) => Err(ApiError::from(e)),
            }
        }
        Err(e) => Err(e),
    };
    match outcome {
        Ok(result) => {
            record.result = Some(result);
            record.ended(JobStatus::Done, ttl);
        }
        Err(e) => {
            // The whole error goes to the log; the job keeps what a client may
            // read, as a webhook delivery would.
            e.log();
            let e = e.public();
            record.error = Some(serde_json::json!({
                "error": e.body.error,
                "message": e.body.message,
                "status": e.status.as_u16(),
            }));
            record.ended(JobStatus::Failed, ttl);
        }
    }
    match store.put_job(&scope, &record).await {
        Ok(()) => tracing::info!(job = %record.id, status = ?record.status, "background job ended"),
        // It will read as lost once its time is up, which is the truth.
        Err(e) => tracing::error!(job = %record.id, error = %e, "could not record how a job ended"),
    }
    drop(permit);
}

fn chrono_now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

/// Where a job is, as a path a client can follow.
pub fn location(state: &AppState, id: &str) -> String {
    let root = state.config.root_path.trim_end_matches('/');
    format!("{root}/v1/jobs/{id}")
}

/// The 202 an asynchronous request gets: the job, where to ask about it, and
/// how soon.
fn accepted(state: &AppState, record: &JobRecord) -> Response {
    let mut res = (StatusCode::ACCEPTED, axum::Json(record)).into_response();
    let headers = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&location(state, &record.id)) {
        headers.insert(header::LOCATION, v);
    }
    headers.insert(
        "preference-applied",
        HeaderValue::from_static("respond-async"),
    );
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    if let Ok(v) = HeaderValue::from_str(&record.request_id) {
        headers.insert("x-request-id", v);
    }
    res
}

/// A caller's job as it stands now, or 404 when there is none or it expired.
pub async fn current(
    state: &AppState,
    store: &Store,
    scope: &str,
    id: &str,
) -> Result<JobRecord, ApiError> {
    let gone = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "there is no such job here, or its result has expired",
        )
    };
    let record = match store.job(scope, id).await {
        Ok(record) => record,
        Err(store::StoreError::NotFound) => return Err(gone()),
        Err(e) => return Err(e.into()),
    };
    record
        .as_of(chrono_now(), state.config.job_ttl)
        .ok_or_else(gone)
}

/// Removes expired jobs now and then, for as long as the service runs. Every
/// replica does it; they share nothing but the store, and deleting twice is
/// harmless.
pub fn spawn_cleanup(state: AppState) {
    let Some(store) = state.store.clone() else {
        return;
    };
    let every = (state.config.job_ttl / 4).clamp(Duration::from_secs(10), Duration::from_secs(600));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match store.expire_jobs(state.config.job_ttl).await {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "expired background jobs removed"),
                Err(e) => tracing::warn!(error = %e, "could not remove expired background jobs"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_are_limited_in_all_and_per_caller() {
        let limiter = JobLimiter::new(3, 2);
        let a1 = limiter.try_acquire("a").unwrap();
        let _a2 = limiter.try_acquire("a").unwrap();
        let refused = limiter.try_acquire("a").unwrap_err();
        assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
        let _b1 = limiter.try_acquire("b").unwrap();
        assert!(limiter.try_acquire("c").is_err(), "three in all");
        drop(a1);
        assert!(limiter.try_acquire("a").is_ok(), "the place came back");
    }

    #[test]
    fn respond_async_is_found_among_other_preferences() {
        let mut headers = HeaderMap::new();
        assert!(!prefers_async(&headers));
        headers.insert("prefer", HeaderValue::from_static("wait=10, Respond-Async"));
        assert!(prefers_async(&headers));
        headers.insert("prefer", HeaderValue::from_static("return=minimal"));
        assert!(!prefers_async(&headers));
    }
}
