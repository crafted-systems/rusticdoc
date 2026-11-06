//! A budget on the request bodies held in memory at once.
//!
//! Forms are read into memory, so the body limit bounds one request and not N of
//! them. Each request is charged what it declares before its body is read, and
//! waits for room when the budget is spent, which is back-pressure rather than a
//! refusal.
//!
//! The charge is taken up front, because charging bytes as they arrive would let
//! concurrent uploads each hold part of the budget and wait on one another. So a
//! client can declare a body and not send it. The stall guard and the body timeout
//! end such a client, and until then one caller holds at most half of the budget:
//! a caller is its credential, or its address when the service has none. Many
//! addresses together can still fill it for that long, which is for rate limiting
//! at the proxy.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body::{Body as HttpBody, Frame, SizeHint};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::auth::Principal;
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
    callers: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
}

/// One request's room. Dropping it gives the room back.
#[derive(Debug)]
pub struct Entered {
    budget: Option<OwnedSemaphorePermit>,
    mine: Option<OwnedSemaphorePermit>,
    caller: String,
    callers: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
}

impl Drop for Entered {
    fn drop(&mut self) {
        self.budget.take();
        self.mine.take();
        // A waiter holds a clone, so a count of one means nobody needs this entry.
        let mut callers = self.callers.lock().unwrap_or_else(|e| e.into_inner());
        if callers
            .get(&self.caller)
            .is_some_and(|mine| Arc::strong_count(mine) == 1)
        {
            callers.remove(&self.caller);
        }
    }
}

impl Admission {
    pub fn new(max_bytes: usize) -> Self {
        let units = u32::try_from((max_bytes as u64).div_ceil(UNIT))
            .unwrap_or(u32::MAX >> 4)
            .clamp(1, u32::MAX >> 4);
        Self {
            budget: Arc::new(Semaphore::new(units as usize)),
            units,
            callers: Arc::default(),
        }
    }

    /// The most one caller may hold.
    fn caller_units(&self) -> u32 {
        self.units.div_ceil(2)
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

    /// Room for `caller`'s request, or `None` once `wait` has passed. The caller's
    /// own share comes first, so waiting for it holds nothing others need.
    async fn enter(
        &self,
        caller: &str,
        headers: &HeaderMap,
        body_limit: usize,
        wait: Duration,
    ) -> Option<Entered> {
        let charge = self.charge(headers, body_limit);
        let share = {
            let mut callers = self.callers.lock().unwrap_or_else(|e| e.into_inner());
            callers
                .entry(caller.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(self.caller_units() as usize)))
                .clone()
        };
        let mut entered = Entered {
            budget: None,
            mine: None,
            caller: caller.to_string(),
            callers: self.callers.clone(),
        };
        let taken = tokio::time::timeout(wait, async {
            let mine = share
                .clone()
                .acquire_many_owned(charge.min(self.caller_units()))
                .await?;
            let budget = self.budget.clone().acquire_many_owned(charge).await?;
            Ok::<_, tokio::sync::AcquireError>((mine, budget))
        })
        .await;
        drop(share);
        // On a timeout `entered` drops with nothing held, and tidies the entry.
        let (mine, budget) = taken.ok()?.ok()?;
        entered.mine = Some(mine);
        entered.budget = Some(budget);
        Some(entered)
    }
}

/// Holds the charge until the response has gone, because a result built from the
/// request stays in memory until then.
struct Charged {
    inner: Body,
    _entered: Entered,
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

/// Who a request is charged to: its credential, or with none configured its
/// address, counted as the per-peer limit counts it.
fn caller_of(req: &Request) -> String {
    match req.extensions().get::<Principal>() {
        Some(Principal(name)) if name != "public" => name.clone(),
        _ => req
            .extensions()
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|peer| crate::peer_key(peer.0.ip()).to_string())
            .unwrap_or_else(|| "public".to_string()),
    }
}

/// Waits for room before the body is read. Sits inside authentication, so a
/// caller with no credentials costs nothing and cannot join the queue.
pub async fn admit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::POST | Method::PUT | Method::PATCH) {
        return next.run(req).await;
    }
    let caller = caller_of(&req);
    let entered = state
        .admission
        .enter(
            &caller,
            req.headers(),
            state.config.body_limit,
            state.config.api_timeout,
        )
        .await;
    let Some(entered) = entered else {
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
            _entered: entered,
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
        let first = admission.enter("a", &headers, limit, wait).await.unwrap();
        assert!(admission.enter("b", &headers, limit, wait).await.is_none());
        drop(first);
        assert!(admission.enter("b", &headers, limit, wait).await.is_some());
    }

    #[tokio::test]
    async fn one_caller_holds_at_most_half_of_the_budget() {
        let admission = Admission::new(8 * UNIT as usize);
        let limit = 64 * UNIT as usize;
        let headers = declaring(Some("4194304"), false);
        let wait = Duration::from_millis(30);
        let mine = admission.enter("a", &headers, limit, wait).await.unwrap();
        assert!(
            admission.enter("a", &headers, limit, wait).await.is_none(),
            "the same caller had more than half"
        );
        let other = admission.enter("b", &headers, limit, wait).await;
        assert!(other.is_some(), "another caller found no room");
        drop(mine);
        assert!(
            admission.enter("a", &headers, limit, wait).await.is_some(),
            "the caller's share did not come back"
        );
    }

    #[tokio::test]
    async fn a_request_past_the_caller_share_can_still_run_alone() {
        let admission = Admission::new(4 * UNIT as usize);
        let headers = declaring(Some("4194304"), false);
        let wait = Duration::from_millis(30);
        let alone = admission
            .enter("a", &headers, 64 * UNIT as usize, wait)
            .await;
        assert!(
            alone.is_some(),
            "a body up to the whole budget must fit somehow"
        );
    }

    #[tokio::test]
    async fn callers_leave_no_trace_when_they_are_done() {
        let admission = Admission::new(8 * UNIT as usize);
        let headers = declaring(Some("1"), false);
        let wait = Duration::from_millis(30);
        for caller in ["a", "b", "c"] {
            let entered = admission.enter(caller, &headers, 1024, wait).await;
            drop(entered);
        }
        assert!(admission.callers.lock().unwrap().is_empty());
    }

    #[test]
    fn an_absurd_budget_still_fits_a_semaphore() {
        let admission = Admission::new(usize::MAX);
        assert!(admission.units >= 1);
    }
}
