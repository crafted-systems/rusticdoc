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
/// never the secret itself, or `tenant:<name>` for a key named in
/// `RUSTICDOC_TENANT_KEYS`, which every key of that tenant shares. `public` when
/// the service requires no credentials.
#[derive(Debug, Clone)]
pub struct Principal(pub String);

/// Whether the request's credential may change what every caller renders
/// with, as the authentication middleware found it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Admin(pub bool);

/// A caller its credentials prove: who, and whether it is an admin.
#[derive(Debug, Clone)]
pub struct Caller {
    pub principal: Principal,
    pub admin: bool,
}

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

/// The principal of a tenant, whichever of its keys was presented. Not a
/// digest: the name is the operator's, not a secret, and it has to stay the
/// same when a key is replaced.
pub fn tenant_principal(tenant: &str) -> Principal {
    Principal(format!("tenant:{tenant}"))
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

/// The caller the request's credentials prove, if any.
pub async fn identify(state: &AppState, headers: &HeaderMap) -> Option<Principal> {
    identify_caller(state, headers).await.map(|c| c.principal)
}

/// The caller the request's credentials prove, if any, and whether it is an
/// admin: an admin key, the basic-auth user when `RUSTICDOC_BASIC_AUTH_ADMIN` is
/// on, or a token whose subject or claim the operator named.
pub async fn identify_caller(state: &AppState, headers: &HeaderMap) -> Option<Caller> {
    let config = &state.config;
    let presented = presented_key(headers);
    let is_admin_key = presented.is_some_and(|key| key_is_listed(&config.admin_keys, key));
    let caller = |principal: Principal, admin: bool| {
        Some(Caller {
            principal,
            admin: admin || is_admin_key,
        })
    };
    if let Some(presented) = presented.filter(|_| !config.api_keys.is_empty())
        && key_is_listed(&config.api_keys, presented)
    {
        return caller(key_principal(presented), false);
    }
    if let Some(presented) = presented
        && let Some(tenant) = tenant_of(&config.tenant_keys, presented)
    {
        return caller(tenant_principal(tenant), false);
    }
    // An admin key is a credential wherever credentials are asked for.
    if let Some(presented) = presented
        && config.auth_configured()
        && is_admin_key
    {
        return caller(key_principal(presented), true);
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
            return caller(basic_principal(user), config.basic_auth_admin);
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
            match verifier.verify_claims(token).await {
                Ok((subject, claims)) => {
                    let admin = oidc_admin(config, &subject, &claims);
                    return caller(oidc_principal(&subject), admin);
                }
                Err(reason) => tracing::warn!(%reason, "bearer token refused"),
            }
        }
    }
    None
}

/// Refuses a request that may not change what every caller renders with. With
/// credentials configured that takes an admin: one of `RUSTICDOC_ADMIN_KEYS`, the
/// basic-auth user when `RUSTICDOC_BASIC_AUTH_ADMIN` is on, or an OpenID Connect
/// user named by `RUSTICDOC_OIDC_ADMIN_SUBJECTS` or `RUSTICDOC_OIDC_ADMIN_CLAIM`.
/// No other credential will do, however valid: a font decides what digits look
/// like in everyone's documents. A service that requires no credentials has
/// already told everyone they may do anything, unless it names admin keys.
pub fn require_admin(state: &AppState, admin: Admin) -> Result<(), ApiError> {
    let config = &state.config;
    if admin.0 {
        return Ok(());
    }
    if config.admin_keys.is_empty() && !config.admin_users_configured() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "admin_required",
            "changing fonts takes an admin, and none is configured: set RUSTICDOC_ADMIN_KEYS, RUSTICDOC_BASIC_AUTH_ADMIN or an OpenID Connect admin",
        ));
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "admin_required",
        "changing fonts takes an admin",
    ))
}

/// Whether an open service treats this request as an admin: always, unless it
/// names admin keys, and then only with one of them.
fn open_admin(state: &AppState, headers: &HeaderMap) -> bool {
    let keys = &state.config.admin_keys;
    keys.is_empty() || presented_key(headers).is_some_and(|key| key_is_listed(keys, key))
}

/// Whether a verified token's holder is an admin: its subject is listed, or the
/// named claim equals the value or is a list that holds it.
fn oidc_admin(config: &rusticdoc_core::Config, subject: &str, claims: &serde_json::Value) -> bool {
    if config.oidc_admin_subjects.iter().any(|s| s == subject) {
        return true;
    }
    let Some((path, wanted)) = &config.oidc_admin_claim else {
        return false;
    };
    // A claim's own name may hold dots, as URL-style claim names do; try it
    // whole first, then as a path into nested objects.
    let found = claims.get(path.as_str()).or_else(|| {
        path.split('.')
            .try_fold(claims, |value, key| value.get(key))
    });
    match found {
        Some(serde_json::Value::String(value)) => value == wanted,
        Some(serde_json::Value::Array(values)) => values.iter().any(|v| v.as_str() == Some(wanted)),
        Some(serde_json::Value::Bool(value)) => wanted == if *value { "true" } else { "false" },
        _ => false,
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
        let admin = Admin(open_admin(&state, req.headers()));
        req.extensions_mut().insert(Principal("public".into()));
        req.extensions_mut().insert(admin);
        return next.run(req).await;
    }
    if let Some(Caller { principal, admin }) = identify_caller(&state, req.headers()).await {
        let caller = LoggedCaller(principal.short());
        req.extensions_mut().insert(principal);
        req.extensions_mut().insert(Admin(admin));
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

/// The tenant whose key `presented` is. Every key is compared, as above.
fn tenant_of<'a>(keys: &'a [(String, String)], presented: &str) -> Option<&'a str> {
    let mut found = None;
    for (tenant, key) in keys {
        if bool::from(digest_eq(key.as_bytes(), presented.as_bytes())) {
            found = Some(tenant.as_str());
        }
    }
    found
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
    fn an_admin_claim_matches_a_value_a_list_or_a_nested_path() {
        let config = |claim: &str, value: &str| rusticdoc_core::Config {
            oidc_admin_claim: Some((claim.into(), value.into())),
            oidc_admin_subjects: vec!["root".into()],
            ..rusticdoc_core::Config::default()
        };
        let claims = serde_json::json!({
            "role": "admin",
            "groups": ["staff", "admins"],
            "realm_access": {"roles": ["admins"]},
            "https://example.com/roles": ["admins"],
            "verified": true,
        });
        assert!(oidc_admin(&config("role", "admin"), "x", &claims));
        assert!(oidc_admin(&config("groups", "admins"), "x", &claims));
        assert!(oidc_admin(
            &config("realm_access.roles", "admins"),
            "x",
            &claims
        ));
        assert!(oidc_admin(
            &config("https://example.com/roles", "admins"),
            "x",
            &claims
        ));
        assert!(oidc_admin(&config("verified", "true"), "x", &claims));
        assert!(
            !oidc_admin(&config("groups", "admin"), "x", &claims),
            "whole values only"
        );
        assert!(!oidc_admin(&config("missing", "admins"), "x", &claims));
        assert!(!oidc_admin(&config("realm_access", "admins"), "x", &claims));
        assert!(
            oidc_admin(&config("missing", "admins"), "root", &claims),
            "a listed subject"
        );
    }

    #[test]
    fn tenants_are_found_whichever_of_their_keys_is_shown() {
        let keys = vec![
            ("acme".to_string(), "one".to_string()),
            ("acme".to_string(), "two".to_string()),
            ("beta".to_string(), "three".to_string()),
        ];
        assert_eq!(tenant_of(&keys, "one"), Some("acme"));
        assert_eq!(tenant_of(&keys, "two"), Some("acme"));
        assert_eq!(tenant_of(&keys, "three"), Some("beta"));
        assert_eq!(tenant_of(&keys, "acme"), None);
        assert_eq!(tenant_principal("acme").short(), "tenant:acme");
    }

    #[test]
    fn principals_show_only_a_prefix_of_their_digest() {
        let short = key_principal("secret").short();
        assert!(short.starts_with("key:"), "{short}");
        assert_eq!(short.len(), "key:".len() + 12);
        assert_eq!(Principal("public".into()).short(), "public");
    }
}
