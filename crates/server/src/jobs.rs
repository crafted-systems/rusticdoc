//! How many webhook jobs may run at once, in all and per caller.
//!
//! A webhook request answers 204 at once, so nothing but these permits stops one
//! caller from queuing work without end. A permit lives as long as the job,
//! through its delivery attempts, because that is how long the job holds memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;

use crate::error::ApiError;

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
                "the service is running as many webhook jobs as it may; try again later",
            ));
        }
        let mine = running.by_caller.entry(caller.to_string()).or_insert(0);
        if *mine >= self.max_per_caller {
            return Err(busy(
                "this caller is running as many webhook jobs as it may; try again later",
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
}
