//! API keys, basic auth and OpenID Connect tokens. Nothing is required until
//! one of them is configured; health and the API description stay open.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::ApiError;
use crate::state::AppState;

/// Who a request is, for scoping what it may reuse and for deriving its webhook
/// signing key: a digest of the key, the basic-auth user or the token's subject,
/// never the secret itself. `public` when the service requires no credentials.
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

/// The principal of a basic-auth user.
pub fn basic_principal(user: &str) -> Principal {
    digest("basic", user)
}

/// The principal of an OpenID Connect token's subject.
pub fn oidc_principal(subject: &str) -> Principal {
    digest("oidc", subject)
}

/// The caller the request's credentials prove, if any.
pub async fn identify(state: &AppState, headers: &HeaderMap) -> Option<Principal> {
    let config = &state.config;
    let keys_set = !config.api_keys.is_empty();
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(str::trim));
    let api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    if let Some(presented) = bearer.or(api_key).filter(|_| keys_set)
        && key_is_listed(&config.api_keys, presented)
    {
        return Some(key_principal(presented));
    }
    // An admin key is a credential wherever credentials are asked for.
    if let Some(presented) = bearer.or(api_key)
        && config.auth_configured()
        && key_is_listed(&config.admin_keys, presented)
    {
        return Some(key_principal(presented));
    }

    if let Some((user, pass)) = &config.basic_auth {
        let basic = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|v| {
                base64::engine::general_purpose::STANDARD
                    .decode(v.trim())
                    .ok()
            })
            .and_then(|v| String::from_utf8(v).ok());
        if let Some(pair) = basic
            && digests_match(format!("{user}:{pass}").as_bytes(), pair.as_bytes())
        {
            return Some(basic_principal(user));
        }
    }

    if let Some(verifier) = &state.oidc {
        // An authentication scheme is case-insensitive.
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim());
        if let Some(token) = token {
            match verifier.verify(token).await {
                Ok(subject) => return Some(oidc_principal(&subject)),
                Err(reason) => tracing::warn!(%reason, "bearer token refused"),
            }
        }
    }
    None
}

/// Refuses a request that may not change what every caller renders with. With
/// credentials configured that takes one of `RUSTICDOC_ADMIN_KEYS`, and no other
/// credential will do, however valid: a font decides what digits look like in
/// everyone's documents. A service that requires no credentials has already told
/// everyone they may do anything, unless it names admin keys.
pub fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let config = &state.config;
    if config.admin_keys.is_empty() {
        if !config.auth_configured() {
            return Ok(());
        }
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "admin_required",
            "changing fonts takes an admin key, and RUSTICDOC_ADMIN_KEYS is not set",
        ));
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(str::trim));
    let api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    match bearer.or(api_key) {
        Some(key) if key_is_listed(&config.admin_keys, key) => Ok(()),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "admin_required",
            "changing fonts takes an admin key",
        )),
    }
}

/// Who a request said it was, for the log: the scheme and a digest prefix. A
/// password or token never appears, and for basic auth only the user is hashed.
fn presented(headers: &HeaderMap) -> String {
    let prefix = |value: &str| hex::encode(Sha256::digest(value.as_bytes()))[..12].to_string();
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if let Some(value) = authorization {
        let (scheme, credential) = value.split_once(' ').unwrap_or((value, ""));
        let scheme = scheme.to_ascii_lowercase();
        if scheme == "basic" {
            let user = base64::engine::general_purpose::STANDARD
                .decode(credential.trim())
                .ok()
                .and_then(|v| String::from_utf8(v).ok())
                .and_then(|pair| pair.split(':').next().map(str::to_string))
                .unwrap_or_default();
            return format!("basic:{}", prefix(&user));
        }
        if scheme == "bearer" {
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
    if let Some(principal) = identify(&state, req.headers()).await {
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
        "a valid API key or credentials are required",
    )
    .into_response();
    if state.config.basic_auth.is_some() {
        res.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"rusticdoc\"".parse().unwrap(),
        );
    }
    res.extensions_mut().insert(LoggedCaller(caller));
    res
}

/// True when `presented` is one of `keys`. Every key is compared, whichever
/// matches, so the time says nothing about which one or how far it got.
fn key_is_listed(keys: &[String], presented: &str) -> bool {
    let mut listed = subtle::Choice::from(0);
    for key in keys {
        listed |= digest_eq(key.as_bytes(), presented.as_bytes());
    }
    listed.into()
}

fn digests_match(a: &[u8], b: &[u8]) -> bool {
    digest_eq(a, b).into()
}

/// Compares digests rather than the values, so the lengths do not show.
fn digest_eq(a: &[u8], b: &[u8]) -> subtle::Choice {
    Sha256::digest(a).ct_eq(&Sha256::digest(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_compared_by_digest_without_stopping_early() {
        let keys = vec!["first".to_string(), "second".to_string()];
        assert!(key_is_listed(&keys, "first"));
        assert!(key_is_listed(&keys, "second"));
        assert!(!key_is_listed(&keys, "third"));
        assert!(!key_is_listed(&keys, ""));
        assert!(!key_is_listed(&keys, "firs"));
    }

    #[test]
    fn principals_show_only_a_prefix_of_their_digest() {
        let short = key_principal("secret").short();
        assert!(short.starts_with("key:"), "{short}");
        assert_eq!(short.len(), "key:".len() + 12);
        assert_eq!(Principal("public".into()).short(), "public");
    }
}
