//! The API key, sent in `X-Api-Key` or as a bearer token. Nothing is required
//! until the key is configured; health and the API description stay open.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::ApiError;
use crate::state::AppState;

/// Who a request is, for scoping what it may reuse and for deriving its webhook
/// signing key: a digest of the key, never the key itself. `public` when the
/// service requires no credentials.
#[derive(Debug, Clone)]
pub struct Principal(pub String);

impl Principal {
    /// What the logs say about this caller: its scheme and the start of its
    /// digest.
    pub fn short(&self) -> String {
        match self.0.split_once(':') {
            Some((scheme, digest)) => format!("{scheme}:{}", &digest[..digest.len().min(12)]),
            None => self.0.clone(),
        }
    }
}

fn digest(kind: &str, secret: &str) -> Principal {
    Principal(format!(
        "{kind}:{}",
        hex::encode(Sha256::digest(secret.as_bytes()))
    ))
}

/// The principal of an API key, as webhook signing keys are derived from it.
pub fn key_principal(key: &str) -> Principal {
    digest("key", key)
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
    is_the_key(key, presented).then(|| key_principal(presented))
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
        req.extensions_mut().insert(Principal("public".into()));
        return next.run(req).await;
    }
    if let Some(principal) = identify(&state, req.headers()) {
        let caller = LoggedCaller(principal.short());
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

/// True when `presented` is the key. Their digests are compared, so the time
/// says nothing about how far a wrong one got.
fn is_the_key(key: &str, presented: &str) -> bool {
    digest_eq(key.as_bytes(), presented.as_bytes()).into()
}

/// Compares digests rather than the values, so the lengths do not show.
fn digest_eq(a: &[u8], b: &[u8]) -> subtle::Choice {
    Sha256::digest(a).ct_eq(&Sha256::digest(b))
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
    fn principals_show_only_a_prefix_of_their_digest() {
        let short = key_principal("secret").short();
        assert!(short.starts_with("key:"), "{short}");
        assert_eq!(short.len(), "key:".len() + 12);
        assert_eq!(Principal("public".into()).short(), "public");
    }
}
