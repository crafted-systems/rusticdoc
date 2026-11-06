//! API keys, basic auth and OpenID Connect tokens. Nothing is required until
//! one of them is configured; health, `/metrics` and the API description stay
//! open.

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::state::AppState;

/// Who a request is, for scoping what it may reuse: a digest of the key or
/// the basic-auth user, never the secret itself. `public` when the service
/// requires no credentials.
#[derive(Debug, Clone)]
pub struct Principal(pub String);

fn digest(kind: &str, secret: &str) -> Principal {
    Principal(format!(
        "{kind}:{}",
        hex::encode(Sha256::digest(secret.as_bytes()))
    ))
}

pub async fn require(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let config = &state.config;
    let keys_set = !config.api_keys.is_empty();
    let basic_set = config.basic_auth.is_some();
    if !keys_set && !basic_set && state.oidc.is_none() {
        req.extensions_mut().insert(Principal("public".into()));
        return next.run(req).await;
    }

    let headers = req.headers();
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(str::trim));
    let api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    if let Some(presented) = bearer.or(api_key).filter(|_| keys_set)
        && config
            .api_keys
            .iter()
            .any(|k| constant_time_eq(k.as_bytes(), presented.as_bytes()))
    {
        let principal = digest("key", presented);
        req.extensions_mut().insert(principal);
        return next.run(req).await;
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
        if let Some(pair) = basic {
            let expected = format!("{user}:{pass}");
            if constant_time_eq(expected.as_bytes(), pair.as_bytes()) {
                let principal = digest("basic", user);
                req.extensions_mut().insert(principal);
                return next.run(req).await;
            }
        }
    }

    if let Some(verifier) = &state.oidc {
        // An authentication scheme is case-insensitive.
        let token = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim().to_string());
        if let Some(token) = token {
            match verifier.verify(&token).await {
                Ok(()) => {
                    let principal = digest("oidc", &token);
                    req.extensions_mut().insert(principal);
                    return next.run(req).await;
                }
                Err(reason) => tracing::debug!(%reason, "bearer token refused"),
            }
        }
    }

    let mut res = ApiError::new(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "a valid API key or credentials are required",
    )
    .into_response();
    if basic_set {
        res.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"rusticdoc\"".parse().unwrap(),
        );
    }
    res
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
