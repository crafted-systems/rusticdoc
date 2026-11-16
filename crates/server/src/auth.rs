//! The API key, sent in `X-Api-Key` or as a bearer token. Nothing is required
//! until the key is configured; health and the API description stay open.

use std::net::SocketAddr;
use std::sync::OnceLock;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::ApiError;
use crate::state::AppState;

/// Who a request is, for the logs and for what it may reuse: `key` when it
/// presented the API key, `public` when the service requires none. Nothing is
/// derived from the key, so no trace of it reaches the logs or the store.
#[derive(Debug, Clone)]
pub struct Principal(pub String);

impl Principal {
    /// A request that presented the API key.
    pub fn key() -> Self {
        Self("key".into())
    }

    /// A request to a service that requires no credentials.
    pub fn public() -> Self {
        Self("public".into())
    }
}

/// The key a request presents, as a bearer token or in `X-Api-Key`.
fn presented_key(headers: &HeaderMap) -> Option<&str> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(str::trim));
    let api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    bearer.or(api_key)
}

/// The caller the request's key proves, if any.
pub fn identify(state: &AppState, headers: &HeaderMap) -> Option<Principal> {
    let key = state.config.api_key.as_deref()?;
    let presented = presented_key(headers)?;
    is_the_key(key, presented).then(Principal::key)
}

/// Who a request said it was, for the log: where the key came from and a digest
/// prefix, never the key.
fn presented(headers: &HeaderMap) -> String {
    let prefix = |value: &str| hex::encode(Sha256::digest(value.as_bytes()))[..12].to_string();
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if let Some(value) = authorization {
        let (scheme, credential) = value.split_once(' ').unwrap_or((value, ""));
        if scheme.eq_ignore_ascii_case("bearer") {
            return format!("bearer:{}", prefix(credential.trim()));
        }
        return "other".to_string();
    }
    match headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        Some(key) => format!("key:{}", prefix(key.trim())),
        None => "none".to_string(),
    }
}

/// The caller a request ended up as, on its response for the access log.
#[derive(Debug, Clone)]
pub struct LoggedCaller(pub String);

pub async fn require(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    if !state.config.auth_configured() {
        req.extensions_mut().insert(Principal::public());
        return next.run(req).await;
    }
    if let Some(principal) = identify(&state, req.headers()) {
        let caller = LoggedCaller(principal.0.clone());
        req.extensions_mut().insert(principal);
        let mut res = next.run(req).await;
        res.extensions_mut().insert(caller);
        return res;
    }
    let caller = presented(req.headers());
    let remote_addr = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.to_string())
        .unwrap_or_else(|| "-".to_string());
    tracing::warn!(%remote_addr, %caller, "authentication failed");
    let mut res = ApiError::new(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "a valid API key is required",
    )
    .into_response();
    res.extensions_mut().insert(LoggedCaller(caller));
    res
}

/// True when `presented` is the key. Their tags are compared, not the values,
/// so the time says nothing about the key's length or how far a wrong one got.
fn is_the_key(key: &str, presented: &str) -> bool {
    tag(key.as_bytes()).ct_eq(&tag(presented.as_bytes())).into()
}

/// HMAC-SHA256 of a value under a key drawn when the first request is checked.
/// A tag is compared and dropped, never stored or sent, and no caller can
/// work one out, since the key never leaves the process.
fn tag(value: &[u8]) -> [u8; 32] {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = KEY.get_or_init(|| {
        let mut key = [0; 32];
        getrandom::fill(&mut key).expect("the system has random bytes");
        key
    });
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(value);
    mac.finalize().into_bytes().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_whole_key_matches() {
        assert!(is_the_key("first", "first"));
        assert!(!is_the_key("first", "second"));
        assert!(!is_the_key("first", ""));
        assert!(!is_the_key("first", "firs"));
        assert!(!is_the_key("first", "first "));
    }

    #[test]
    fn a_value_has_one_tag_and_another_value_has_another() {
        assert_eq!(tag(b"first"), tag(b"first"));
        assert_ne!(tag(b"first"), tag(b"second"));
        assert_ne!(tag(b"first"), [0; 32]);
    }
}
