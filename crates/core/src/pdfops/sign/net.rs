//! The network a signature can need: a timestamp authority, OCSP responders,
//! CRL distribution points and certificate issuers named by certificates.
//!
//! The core crate makes no requests itself. Whoever builds the engines hands
//! it an [`Outbound`]: the server's checks every URL against the outbound
//! rules (allow and deny lists, private addresses refused, the resolved
//! addresses pinned, no redirects) before it connects.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::{Error, Result};

/// One request: a GET when `body` is `None`, a POST otherwise.
#[derive(Clone)]
pub struct OutboundRequest {
    pub url: String,
    /// `Content-Type` of the body.
    pub content_type: Option<&'static str>,
    pub body: Option<Vec<u8>>,
    /// Further headers, such as a timestamp authority's credentials.
    pub headers: Vec<(String, String)>,
    /// Basic credentials, user and password.
    pub basic_auth: Option<(String, String)>,
    /// Longest response accepted, in bytes.
    pub max_bytes: usize,
    pub timeout: Duration,
    /// What the request is for, in error messages: `the timestamp authority`.
    pub purpose: &'static str,
}

impl std::fmt::Debug for OutboundRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Headers and credentials stay out of logs.
        f.debug_struct("OutboundRequest")
            .field("url", &self.url)
            .field("purpose", &self.purpose)
            .field("max_bytes", &self.max_bytes)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

/// Makes the requests a signature needs. Implementations refuse what the
/// outbound rules refuse, keep to `timeout` and `max_bytes`, and answer an
/// error for any status other than 200.
#[async_trait]
pub trait Outbound: Send + Sync + std::fmt::Debug {
    async fn fetch(&self, request: OutboundRequest) -> Result<Vec<u8>>;
}

/// Why a request a signature needed failed, as the client reads it: a 502,
/// since the service's upstream failed and nothing in the request can fix it.
pub fn upstream(purpose: &str, what: impl std::fmt::Display) -> Error {
    Error::Status(502, format!("{purpose} failed: {what}"))
}
