//! A budget on the request bodies held in memory at once.
//!
//! Forms are read into memory, so the body limit bounds one request and not N of
//! them. Each request is charged what it declares before its body is read, and
//! waits for room when the budget is spent, which is back-pressure rather than a
//! refusal. A client that never sends what it declared is ended by the stall
//! guard and the route's timeout, so it cannot hold its charge for long.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body::{Body as HttpBody, Frame, SizeHint};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::ApiError;
use crate::state::AppState;

/// The budget is counted in these, because a semaphore counts in `u32`.
const UNIT: u64 = 1024 * 1024;

/// How long a refused client is told to wait, in seconds.
const RETRY_AFTER: &str = "5";

/// The budget. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Admission {
    budget: Arc<Semaphore>,
    units: u32,
}

impl Admission {
    pub fn new(max_bytes: usize) -> Self {
        let units = u32::try_from((max_bytes as u64).div_ceil(UNIT))
            .unwrap_or(u32::MAX >> 4)
            .clamp(1, u32::MAX >> 4);
        Self {
            budget: Arc::new(Semaphore::new(units as usize)),
            units,
        }
    }

    /// What a request costs: what it declares, or the whole body limit when it
    /// streams without saying. Hyper holds a body to its declared length, and
    /// the body limit stops a longer one. A request larger than the budget can
    /// still run, alone.
    fn charge(&self, headers: &HeaderMap, body_limit: usize) -> u32 {
        let bytes = match headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
        {
            Some(declared) => declared.min(body_limit as u64),
            None if headers.contains_key(header::TRANSFER_ENCODING) => body_limit as u64,
            None => 0,
        };
        u32::try_from(bytes.div_ceil(UNIT))
            .unwrap_or(u32::MAX)
            .clamp(1, self.units)
    }

    /// Takes the whole budget, for a test that needs none left.
    #[doc(hidden)]
    pub fn hold_all_for_test(&self) -> OwnedSemaphorePermit {
        self.budget
            .clone()
            .try_acquire_many_owned(self.units)
            .expect("nothing else holds the budget in this test")
    }

    /// Room for `headers`' request, or `None` once `wait` has passed.
    async fn enter(
        &self,
        headers: &HeaderMap,
        body_limit: usize,
        wait: Duration,
    ) -> Option<OwnedSemaphorePermit> {
        let charge = self.charge(headers, body_limit);
        match tokio::time::timeout(wait, self.budget.clone().acquire_many_owned(charge)).await {
            Ok(Ok(permit)) => Some(permit),
            _ => None,
        }
    }
}

/// Holds the charge until the response has gone, because a result built from the
/// request stays in memory until then.
struct Charged {
    inner: Body,
    _permit: OwnedSemaphorePermit,
}

impl HttpBody for Charged {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// Waits for room before the body is read. Sits inside authentication, so a
/// caller with no credentials costs nothing and cannot join the queue.
pub async fn admit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::POST | Method::PUT | Method::PATCH) {
        return next.run(req).await;
    }
    let permit = state
        .admission
        .enter(
            req.headers(),
            state.config.body_limit,
            state.config.api_timeout,
        )
        .await;
    let Some(permit) = permit else {
        let error = ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "the service is holding as many request bodies as it may; try again later",
        );
        let mut res = error.into_response();
        res.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static(RETRY_AFTER));
        return res;
    };
    next.run(req).await.map(|inner| {
        Body::new(Charged {
            inner,
            _permit: permit,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaring(length: Option<&str>, chunked: bool) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(length) = length {
            headers.insert(header::CONTENT_LENGTH, length.parse().unwrap());
        }
        if chunked {
            headers.insert(header::TRANSFER_ENCODING, "chunked".parse().unwrap());
        }
        headers
    }

    #[test]
    fn a_request_pays_what_it_declares() {
        let limit = 64 * UNIT as usize;
        let admission = Admission::new(128 * UNIT as usize);
        let cost = |headers: HeaderMap| admission.charge(&headers, limit);
        assert_eq!(
            cost(declaring(None, false)),
            1,
            "no body still costs a unit"
        );
        assert_eq!(cost(declaring(Some("0"), false)), 1);
        assert_eq!(cost(declaring(Some("1"), false)), 1);
        assert_eq!(cost(declaring(Some("3145729"), false)), 4);
        assert_eq!(cost(declaring(None, true)), 64, "a stream pays the limit");
        assert_eq!(cost(declaring(Some("junk"), true)), 64);
        assert_eq!(cost(declaring(Some("99999999999"), false)), 64);
    }

    #[test]
    fn a_request_larger_than_the_budget_can_run_alone() {
        let admission = Admission::new(2 * UNIT as usize);
        let cost = admission.charge(&declaring(None, true), 64 * UNIT as usize);
        assert_eq!(cost, 2);
    }

    #[tokio::test]
    async fn the_budget_returns_when_the_charge_drops() {
        let admission = Admission::new(4 * UNIT as usize);
        let limit = 64 * UNIT as usize;
        let headers = declaring(Some("4194304"), false);
        let wait = Duration::from_millis(30);
        let first = admission.enter(&headers, limit, wait).await.unwrap();
        assert!(admission.enter(&headers, limit, wait).await.is_none());
        drop(first);
        assert!(admission.enter(&headers, limit, wait).await.is_some());
    }

    #[test]
    fn an_absurd_budget_still_fits_a_semaphore() {
        let admission = Admission::new(usize::MAX);
        assert!(admission.units >= 1);
    }
}
