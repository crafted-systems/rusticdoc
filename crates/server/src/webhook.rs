//! Asynchronous delivery of results.
//!
//! A request that carries `X-Rusticdoc-Webhook-Url` (or Gotenberg's
//! `Gotenberg-Webhook-Url`) returns 204 at once; the result is then posted to
//! that URL, signed when `RUSTICDOC_WEBHOOK_SECRET` is set. Failures go to
//! the error URL as JSON. Deliveries retry with backoff.
//!
//! Targets are resolved when the request arrives, every resolved address is
//! checked against non-global ranges, the URL is rebuilt from the normalised
//! host, and the delivery is pinned to those addresses with redirects
//! disabled, so neither DNS rebinding nor a redirect can turn a webhook into
//! a request to something beside the service.
//!
//! The signature covers the timestamp and the body:
//! `X-Rusticdoc-Signature: v1=<hex HMAC-SHA256 of "<timestamp>.<body>">`,
//! with the timestamp in `X-Rusticdoc-Timestamp`. Receivers verify the
//! signature and reject timestamps outside their freshness window.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use regex::Regex;
use rusticdoc_core::Output;
use sha2::Sha256;
use tracing::Instrument;

use crate::auth::Principal;
use crate::error::ApiError;
use crate::jobs::JobPermit;
use crate::metrics::OutcomeLabels;
use crate::state::AppState;

pub const SIGNATURE: &str = "x-rusticdoc-signature";
pub const EVENT: &str = "x-rusticdoc-event";
pub const TIMESTAMP: &str = "x-rusticdoc-timestamp";

/// Headers a client may not set through the extra-headers field, because
/// the service owns them.
const RESERVED_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "content-type",
    "transfer-encoding",
    "connection",
    "x-request-id",
];

/// True for headers the service owns, which a client may not set on
/// deliveries or downloads.
pub fn is_reserved_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("x-rusticdoc-") || RESERVED_HEADERS.contains(&lower.as_str())
}

/// What a URL must get past before it is resolved. The lists add to the refusal
/// of private addresses and never relax it.
#[derive(Debug, Clone, Default)]
pub struct UrlPolicy {
    /// Allow addresses that are not global unicast.
    pub allow_private: bool,
    /// A URL must match one of these. Empty allows any.
    pub allow: Vec<Regex>,
    /// A URL may match none of these. Deny wins over allow.
    pub deny: Vec<Regex>,
}

impl UrlPolicy {
    /// The private-address rule alone.
    pub fn private(allow_private: bool) -> Self {
        Self {
            allow_private,
            ..Self::default()
        }
    }

    fn compile(allow_private: bool, allow: &[String], deny: &[&String]) -> Result<Self, String> {
        let compile = |pattern: &String| Regex::new(pattern).map_err(|e| e.to_string());
        Ok(Self {
            allow_private,
            allow: allow.iter().map(compile).collect::<Result<_, _>>()?,
            deny: deny
                .iter()
                .copied()
                .map(compile)
                .collect::<Result<_, _>>()?,
        })
    }

    fn permits(&self, url: &str) -> bool {
        !self.deny.iter().any(|r| r.is_match(url))
            && (self.allow.is_empty() || self.allow.iter().any(|r| r.is_match(url)))
    }
}

/// The policies of every outbound URL a request can name.
#[derive(Debug, Clone, Default)]
pub struct UrlPolicies {
    pub webhook: UrlPolicy,
    pub webhook_error: UrlPolicy,
    pub download: UrlPolicy,
}

impl UrlPolicies {
    /// From the settings. [`rusticdoc_core::Config::check`] has seen the patterns.
    pub fn from_config(config: &rusticdoc_core::Config) -> Result<Self, String> {
        let compat = &config.compat;
        let hook = config.webhook_allow_private;
        let error_allow = if compat.webhook_error_allow_list.is_empty() {
            &compat.webhook_allow_list
        } else {
            &compat.webhook_error_allow_list
        };
        let error_deny: Vec<&String> = compat
            .webhook_deny_list
            .iter()
            .chain(&compat.webhook_error_deny_list)
            .collect();
        Ok(Self {
            webhook: UrlPolicy::compile(
                hook,
                &compat.webhook_allow_list,
                &compat.webhook_deny_list.iter().collect::<Vec<_>>(),
            )?,
            webhook_error: UrlPolicy::compile(hook, error_allow, &error_deny)?,
            download: UrlPolicy::compile(
                config.download_from_allow_private,
                &compat.download_allow_list,
                &compat.download_deny_list.iter().collect::<Vec<_>>(),
            )?,
        })
    }

    /// The private-address rule alone, for webhooks and downloads alike.
    pub fn private(allow_private: bool) -> Self {
        Self {
            webhook: UrlPolicy::private(allow_private),
            webhook_error: UrlPolicy::private(allow_private),
            download: UrlPolicy::private(allow_private),
        }
    }
}

/// A vetted destination: the URL rebuilt from its normalised host, and the
/// addresses that host resolved to when the request came in.
#[derive(Debug, Clone)]
pub struct Target {
    pub url: String,
    pub host: String,
    pub addrs: Vec<SocketAddr>,
}

#[derive(Debug, Clone)]
pub struct Webhook {
    pub target: Target,
    pub error_target: Option<Target>,
    pub method: Method,
    pub extra_headers: HeaderMap,
    /// Where events about the delivery go, as Gotenberg's clients ask for.
    pub events_target: Option<Target>,
    /// The method for the error URL, when it differs.
    pub error_method: Option<Method>,
    /// Failures only go to the error URL, as Gotenberg sends them, never to
    /// the result URL.
    pub errors_to_error_url_only: bool,
    /// A valid W3C trace context from the request, so a trace continues across
    /// the delivery.
    pub traceparent: Option<HeaderValue>,
    /// This job's place among the running ones, held through its delivery.
    pub permit: Option<Arc<JobPermit>>,
}

/// The webhook a `/v1/*` request asks for, with its place among the running
/// jobs, or nothing when it runs synchronously. As an extractor it refuses before
/// the body is read.
pub struct Hook(pub Option<Webhook>);

impl FromRequestParts<AppState> for Hook {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let caller = parts
            .extensions
            .get::<Principal>()
            .map(|p| p.0.as_str())
            .unwrap_or("public");
        // `WEBHOOK_DISABLE` means the headers are ignored, here as on `/forms/*`.
        if state.config.compat.webhook_disabled {
            return Ok(Self(None));
        }
        let Some(mut hook) = Webhook::from_headers(&parts.headers, &state.url_policies).await?
        else {
            return Ok(Self(None));
        };
        hook.permit = Some(Arc::new(state.jobs.try_acquire(caller)?));
        Ok(Self(Some(hook)))
    }
}

/// Runs `job` to `limit`, as one timeout over everything in it.
pub async fn within<F, T>(limit: Duration, job: F) -> Result<T, ApiError>
where
    F: Future<Output = Result<T, ApiError>>,
{
    match tokio::time::timeout(limit, job).await {
        Ok(result) => result,
        Err(_) => Err(rusticdoc_core::Error::Timeout(limit).into()),
    }
}

/// Runs `job` in the background and delivers what it gives, inside the request's
/// span. The hook's permit is released when the delivery is over.
pub fn spawn_job<F>(state: AppState, hook: Webhook, request_id: String, job: F)
where
    F: Future<Output = Result<Output, ApiError>> + Send + 'static,
{
    let span = tracing::Span::current();
    tokio::spawn(
        async move {
            let result = job.await;
            deliver(state, hook, request_id, result).await;
        }
        .instrument(span),
    );
}

impl Webhook {
    /// Reads the webhook headers and resolves their hosts; `Ok(None)` when
    /// the request is synchronous.
    pub async fn from_headers(
        headers: &HeaderMap,
        policies: &UrlPolicies,
    ) -> Result<Option<Self>, ApiError> {
        let get = |names: &[&str]| {
            names
                .iter()
                .find_map(|n| headers.get(*n))
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let Some(url) = get(&["x-rusticdoc-webhook-url", "gotenberg-webhook-url"]) else {
            return Ok(None);
        };
        let target = resolve(&url, &policies.webhook).await?;
        let error_target = match get(&[
            "x-rusticdoc-webhook-error-url",
            "gotenberg-webhook-error-url",
        ]) {
            Some(e) => Some(resolve(&e, &policies.webhook_error).await?),
            None => None,
        };
        let method = match get(&["x-rusticdoc-webhook-method", "gotenberg-webhook-method"]) {
            None => Method::POST,
            Some(m) => match m.to_ascii_uppercase().as_str() {
                "POST" => Method::POST,
                "PUT" => Method::PUT,
                "PATCH" => Method::PATCH,
                other => {
                    return Err(ApiError::bad_request(format!(
                        "webhook method '{other}' is not POST, PUT or PATCH"
                    )));
                }
            },
        };
        let mut extra_headers = HeaderMap::new();
        if let Some(raw) = get(&[
            "x-rusticdoc-webhook-extra-headers",
            "gotenberg-webhook-extra-http-headers",
        ]) {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&raw)
                .map_err(|e| {
                    ApiError::bad_request(format!(
                        "webhook extra headers are not a JSON object: {e}"
                    ))
                })?;
            for (k, v) in map {
                let value = match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                let name = HeaderName::from_bytes(k.as_bytes())
                    .map_err(|_| ApiError::bad_request(format!("bad webhook header name '{k}'")))?;
                let lower = name.as_str();
                if lower.starts_with("x-rusticdoc-") || RESERVED_HEADERS.contains(&lower) {
                    return Err(ApiError::bad_request(format!(
                        "webhook header '{k}' is set by the service and cannot be overridden"
                    )));
                }
                let value = HeaderValue::from_str(&value).map_err(|_| {
                    ApiError::bad_request(format!("bad webhook header value for '{k}'"))
                })?;
                extra_headers.insert(name, value);
            }
        }
        Ok(Some(Self {
            target,
            error_target,
            method,
            extra_headers,
            events_target: None,
            error_method: None,
            errors_to_error_url_only: false,
            traceparent: None,
            permit: None,
        }))
    }

    /// Reads Gotenberg's webhook headers: a result URL needs an error URL or an
    /// events URL beside it. Every URL goes through the same vetting as any
    /// other delivery, and the extra headers through the same reservations.
    pub async fn from_gotenberg_headers(
        headers: &HeaderMap,
        policies: &UrlPolicies,
    ) -> Result<Option<Self>, ApiError> {
        let get = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        if get("gotenberg-webhook-url").is_none() {
            // The service's own webhook headers work here as anywhere.
            return Self::from_headers(headers, policies).await;
        }
        let events_url = get("gotenberg-webhook-events-url");
        let error_url = get("gotenberg-webhook-error-url");
        if error_url.is_none() && events_url.is_none() {
            return Err(ApiError::bad_request(
                "At least one of 'Gotenberg-Webhook-Error-Url' or 'Gotenberg-Webhook-Events-Url' headers is required",
            ));
        }
        let method_of = |header: &str, name: &str| -> Result<Method, ApiError> {
            let Some(value) = get(name) else {
                return Ok(Method::POST);
            };
            match value.to_ascii_uppercase().as_str() {
                "POST" => Ok(Method::POST),
                "PATCH" => Ok(Method::PATCH),
                "PUT" => Ok(Method::PUT),
                other => Err(ApiError::bad_request(format!(
                    "Invalid '{header}' header value: expected 'POST', 'PATCH' or 'PUT', but got '{other}'"
                ))),
            }
        };
        let method = method_of("Gotenberg-Webhook-Method", "gotenberg-webhook-method")?;
        let error_method = method_of(
            "Gotenberg-Webhook-Error-Method",
            "gotenberg-webhook-error-method",
        )?;
        let Some(mut hook) = Self::from_headers(headers, policies).await? else {
            return Ok(None);
        };
        hook.method = method;
        hook.error_method = Some(error_method);
        hook.errors_to_error_url_only = true;
        if let Some(events_url) = events_url {
            hook.events_target = Some(resolve(&events_url, &policies.webhook).await?);
        }
        hook.traceparent = headers
            .get("traceparent")
            .filter(|v| v.to_str().is_ok_and(valid_traceparent))
            .cloned();
        Ok(Some(hook))
    }
}

/// `00-<32 hex>-<16 hex>-<2 hex>`, the only form that is forwarded.
fn valid_traceparent(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    let hex = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit());
    parts.len() == 4
        && hex(parts[0], 2)
        && hex(parts[1], 32)
        && hex(parts[2], 16)
        && hex(parts[3], 2)
}

/// Checks the URL's shape, resolves its host, and refuses any address that
/// is not global unicast unless allowed. The returned URL carries the
/// normalised host, which is also the key the delivery client pins.
pub async fn resolve(url: &str, policy: &UrlPolicy) -> Result<Target, ApiError> {
    let allow_private = policy.allow_private;
    let mut parsed = reqwest::Url::parse(url)
        .map_err(|e| ApiError::bad_request(format!("webhook URL '{url}': {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(format!(
            "webhook URL '{url}' must be http or https"
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ApiError::bad_request(format!(
            "webhook URL '{url}' must not carry credentials; use extra headers"
        )));
    }
    if !policy.permits(parsed.as_str()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "url_not_allowed",
            format!("URL '{}' is not allowed on this service", redact(url)),
        ));
    }
    let raw_host = parsed
        .host_str()
        .ok_or_else(|| ApiError::bad_request(format!("webhook URL '{url}' has no host")))?
        .to_string();
    let bare = raw_host
        .trim_matches(|c| c == '[' || c == ']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| ApiError::bad_request(format!("webhook URL '{url}' has no port")))?;

    if !allow_private && is_private_name(&bare) {
        return Err(private(url));
    }
    let addrs: Vec<SocketAddr> = match bare.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::net::lookup_host((bare.as_str(), port))
            .await
            .map_err(|e| {
                ApiError::bad_request(format!("webhook host '{bare}' does not resolve: {e}"))
            })?
            .collect(),
    };
    if addrs.is_empty() {
        return Err(ApiError::bad_request(format!(
            "webhook host '{bare}' resolves to no address"
        )));
    }
    if !allow_private && addrs.iter().any(|a| !is_global_ip(a.ip())) {
        return Err(private(url));
    }

    // The URL the client sends and the key it pins must be one string.
    let host_for_url = if bare.contains(':') {
        format!("[{bare}]")
    } else {
        bare.clone()
    };
    parsed
        .set_host(Some(&host_for_url))
        .map_err(|_| ApiError::bad_request(format!("webhook host '{bare}' is not valid")))?;
    let host = parsed
        .host_str()
        .unwrap_or(&bare)
        .trim_matches(|c| c == '[' || c == ']')
        .to_string();
    Ok(Target {
        url: parsed.to_string(),
        host,
        addrs,
    })
}

fn private(url: &str) -> ApiError {
    ApiError::bad_request(format!(
        "webhook URL '{}' points at a private address; set RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true to allow it",
        redact(url)
    ))
}

fn is_private_name(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".home.arpa")
}

/// Global unicast only: everything loopback, private, link-local, carrier
/// NAT, multicast, reserved, documentation or benchmark is out, including
/// IPv4 addresses embedded in IPv6 forms.
pub fn is_global_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_global_v4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_global_v4(v4);
            }
            let s = ip.segments();
            // IPv4-compatible (::a.b.c.d), NAT64 (64:ff9b::/96) and 6to4 (2002::/16)
            // all carry an IPv4 address that decides it.
            let v4_at = |hi: u16, lo: u16| {
                Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8)
            };
            let compatible = s[..6] == [0, 0, 0, 0, 0, 0] && (s[6] != 0 || s[7] != 0);
            let nat64 = s[..6] == [0x64, 0xff9b, 0, 0, 0, 0];
            let embedded = if compatible || nat64 {
                Some(v4_at(s[6], s[7]))
            } else if s[0] == 0x2002 {
                Some(v4_at(s[1], s[2]))
            } else {
                None
            };
            if let Some(v4) = embedded {
                return is_global_v4(v4);
            }
            !(ip.is_loopback()
                || ip.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // link local fe80::/10
                || (s[0] & 0xffc0) == 0xfec0 // site local fec0::/10
                || (s[0] & 0xff00) == 0xff00 // multicast ff00::/8
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x2001 && s[1] == 0x0002 && s[2] == 0)) // benchmark
        }
    }
}

fn is_global_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0
        || o[0] >= 240 // reserved 240/4
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // carrier NAT 100.64/10
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
        || (o[0] == 198 && (o[1] & 0xfe) == 18)) // benchmark 198.18/15
}

/// The URL without query or credentials, for logs.
pub fn redact(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut u) => {
            u.set_query(None);
            u.set_fragment(None);
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.to_string()
        }
        Err(_) => "<invalid url>".to_string(),
    }
}

/// `v1=<hex hmac>` over `"<timestamp>.<body>"`.
pub fn sign(secret: &str, timestamp: u64, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("v1={}", hex::encode(mac.finalize().into_bytes()))
}

/// A client pinned to the target's vetted addresses, following no redirects.
pub fn client_for(target: &Target, timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("rusticdoc/{}", rusticdoc_core::VERSION))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&target.host, &target.addrs)
        .build()
}

/// Posts the result, or the error, and records the outcome.
pub async fn deliver(
    state: AppState,
    hook: Webhook,
    request_id: String,
    result: Result<Output, ApiError>,
) {
    // The whole error goes to the log; what is delivered is what a client may
    // read.
    let result = result.map_err(|e| {
        e.log();
        e.public()
    });
    let outcome = match &result {
        Ok(_) => serde_json::json!({
            "event": "webhook.success",
            "correlationId": request_id,
            "timestamp": rfc3339(unix_now()),
        }),
        Err(e) => serde_json::json!({
            "event": "webhook.error",
            "correlationId": request_id,
            "timestamp": rfc3339(unix_now()),
            "error": { "status": e.status.as_u16(), "message": e.body.message },
        }),
    };
    if result.is_err() && hook.errors_to_error_url_only && hook.error_target.is_none() {
        send_event(&state, &hook, &request_id, &outcome).await;
        return;
    }
    let method = match (&result, &hook.error_method) {
        (Err(_), Some(method)) => method.clone(),
        _ => hook.method.clone(),
    };
    let (target, event, body, content_type, disposition) = match &result {
        Ok(out) => (
            hook.target.clone(),
            "render.completed",
            out.bytes.to_vec(),
            out.content_type.to_string(),
            Some(crate::routes::form::disposition(&out.name)),
        ),
        Err(e) => (
            hook.error_target
                .clone()
                .unwrap_or_else(|| hook.target.clone()),
            "render.failed",
            serde_json::to_vec(&serde_json::json!({
                "error": e.body.error,
                "message": e.body.message,
                "status": e.status.as_u16(),
                "requestId": request_id,
            }))
            .unwrap_or_default(),
            "application/json".to_string(),
            None,
        ),
    };
    let url = target.url.clone();
    let shown = redact(&url);
    let client = match client_for(&target, state.config.api_timeout) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(url = %shown, error = %e, "webhook client could not be built");
            count(&state, "gave_up");
            send_event(&state, &hook, &request_id, &outcome).await;
            return;
        }
    };
    let trace_header = &state.config.compat.trace_header;

    let attempts = state.config.webhook_attempts.max(1);
    for attempt in 1..=attempts {
        let timestamp = unix_now();
        let mut req = client
            .request(method.clone(), &url)
            .header("content-type", &content_type)
            .header(EVENT, event)
            .header("x-request-id", &request_id)
            .header(trace_header.as_str(), &request_id)
            .header("gotenberg-version", rusticdoc_core::VERSION)
            .header(TIMESTAMP, timestamp.to_string())
            .header("x-rusticdoc-attempt", attempt.to_string());
        // The caller chose this destination, so it may name the file for it.
        if let Some(d) = &disposition
            && !hook.extra_headers.contains_key("content-disposition")
        {
            req = req.header("content-disposition", d);
        }
        if let Some(secret) = &state.config.webhook_secret {
            req = req.header(SIGNATURE, sign(secret, timestamp, &body));
        }
        if let Some(traceparent) = &hook.traceparent {
            req = req.header("traceparent", traceparent);
        }
        for (k, v) in hook.extra_headers.iter() {
            // Set by the service above, so an extra header cannot misreport them.
            if k.as_str() == "gotenberg-version"
                || k.as_str().eq_ignore_ascii_case(trace_header)
                || k.as_str() == "traceparent"
            {
                continue;
            }
            req = req.header(k, v);
        }
        match req.body(body.clone()).send().await {
            Ok(res) if res.status().is_success() => {
                tracing::info!(url = %shown, event, attempt, status = res.status().as_u16(), "webhook delivered");
                count(&state, "delivered");
                send_event(&state, &hook, &request_id, &outcome).await;
                return;
            }
            Ok(res) => {
                tracing::warn!(url = %shown, event, attempt, status = res.status().as_u16(), "webhook rejected");
            }
            Err(e) => {
                tracing::warn!(url = %shown, event, attempt, error = %e, "webhook failed");
            }
        }
        count(&state, "failed");
        if attempt < attempts {
            tokio::time::sleep(Duration::from_secs(3u64.pow(attempt - 1))).await;
        }
    }
    tracing::error!(url = %shown, event, attempts, "webhook gave up");
    count(&state, "gave_up");
    send_event(&state, &hook, &request_id, &outcome).await;
}

/// Tells the events URL how the delivery went. One attempt: an event that does
/// not arrive changes nothing about the result.
async fn send_event(state: &AppState, hook: &Webhook, request_id: &str, event: &serde_json::Value) {
    let Some(target) = &hook.events_target else {
        return;
    };
    let shown = redact(&target.url);
    let client = match client_for(target, state.config.api_timeout) {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(url = %shown, error = %e, "webhook events client could not be built");
            return;
        }
    };
    let mut req = client
        .post(&target.url)
        .header("content-type", "application/json")
        .header(state.config.compat.trace_header.as_str(), request_id)
        .header("gotenberg-version", rusticdoc_core::VERSION);
    if let Some(traceparent) = &hook.traceparent {
        req = req.header("traceparent", traceparent);
    }
    if let Err(e) = req.body(event.to_string()).send().await {
        tracing::warn!(url = %shown, error = %e, "webhook event was not delivered");
    }
}

fn count(state: &AppState, outcome: &str) {
    state
        .metrics
        .webhooks
        .get_or_create(&OutcomeLabels {
            outcome: outcome.into(),
        })
        .inc();
}

/// The 204 a webhook request gets.
pub fn accepted(request_id: &str) -> axum::response::Response {
    let mut res = axum::response::Response::new(axum::body::Body::empty());
    *res.status_mut() = StatusCode::NO_CONTENT;
    if let Ok(v) = HeaderValue::from_str(request_id) {
        res.headers_mut().insert("x-request-id", v);
    }
    res
}

/// A Unix time in UTC as `2026-11-06T08:39:00Z`.
fn rfc3339(secs: u64) -> String {
    // Days to civil date, after Howard Hinnant's algorithm.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn timestamps_and_trace_contexts() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_709_210_096), "2024-02-29T12:34:56Z");
        assert!(valid_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        ));
        assert!(!valid_traceparent("00-short-b7ad6b7169203331-01"));
        assert!(!valid_traceparent("not a trace"));
    }

    #[test]
    fn non_global_ranges_are_refused() {
        for p in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "0.0.0.0",
            "100.64.0.1",
            "100.127.255.254",
            "192.0.0.8",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::10.0.0.1",
            "64:ff9b::7f00:1",
            "2002:0a00:0001::",
        ] {
            assert!(!is_global_ip(ip(p)), "{p} should not be global");
        }
        for p in [
            "8.8.8.8",
            "100.128.0.1",
            "172.32.0.1",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
        ] {
            assert!(is_global_ip(ip(p)), "{p} should be global");
        }
    }

    #[tokio::test]
    async fn literal_and_named_private_targets_are_refused_unless_allowed() {
        assert!(
            resolve("http://127.0.0.1:9000/x", &UrlPolicy::private(false))
                .await
                .is_err()
        );
        assert!(
            resolve(
                "http://[::ffff:127.0.0.1]:9000/x",
                &UrlPolicy::private(false)
            )
            .await
            .is_err()
        );
        assert!(
            resolve("http://10.1.2.3/x", &UrlPolicy::private(false))
                .await
                .is_err()
        );
        assert!(
            resolve("http://localhost/x", &UrlPolicy::private(false))
                .await
                .is_err()
        );
        assert!(
            resolve("http://LOCALHOST./x", &UrlPolicy::private(false))
                .await
                .is_err()
        );
        assert!(
            resolve("http://api.internal/x", &UrlPolicy::private(false))
                .await
                .is_err()
        );
        assert!(
            resolve("ftp://hooks.example.com/x", &UrlPolicy::private(false))
                .await
                .is_err()
        );
        assert!(
            resolve(
                "http://user:pw@hooks.example.com/x",
                &UrlPolicy::private(true)
            )
            .await
            .is_err()
        );
        let t = resolve("http://127.0.0.1:9000/x", &UrlPolicy::private(true))
            .await
            .unwrap();
        assert_eq!(
            t.addrs,
            vec!["127.0.0.1:9000".parse::<SocketAddr>().unwrap()]
        );
        assert_eq!(t.host, "127.0.0.1");
    }

    fn lists(allow: &[&str], deny: &[&str], allow_private: bool) -> UrlPolicy {
        let list = |patterns: &[&str]| patterns.iter().map(|p| Regex::new(p).unwrap()).collect();
        UrlPolicy {
            allow_private,
            allow: list(allow),
            deny: list(deny),
        }
    }

    #[tokio::test]
    async fn allow_and_deny_lists_add_to_the_private_rule() {
        let forbidden = |policy: UrlPolicy, url: &'static str| async move {
            resolve(url, &policy).await.unwrap_err().status == StatusCode::FORBIDDEN
        };
        let only_a = lists(&["^http://1\\.1\\.1\\.1/a"], &[], false);
        assert!(forbidden(only_a.clone(), "http://1.1.1.1/b").await);
        assert!(resolve("http://1.1.1.1/a", &only_a).await.is_ok());
        // Deny wins over allow.
        let both = lists(&[".+"], &["/secret"], false);
        assert!(forbidden(both.clone(), "http://1.1.1.1/secret").await);
        assert!(resolve("http://1.1.1.1/open", &both).await.is_ok());
        // An allow list never lets a private address through.
        let wide = lists(&[".+"], &[], false);
        let err = resolve("http://127.0.0.1/x", &wide).await.unwrap_err();
        assert!(err.body.message.contains("private"), "{}", err.body.message);
    }

    #[tokio::test]
    async fn the_url_is_rebuilt_from_the_normalised_host() {
        let t = resolve("http://LocalHost.:9000/path?x=1", &UrlPolicy::private(true))
            .await
            .unwrap();
        assert_eq!(t.host, "localhost");
        assert_eq!(t.url, "http://localhost:9000/path?x=1");
        let t = resolve("http://[::1]:9000/p", &UrlPolicy::private(true))
            .await
            .unwrap();
        assert_eq!(t.host, "::1");
        assert_eq!(t.url, "http://[::1]:9000/p");
    }

    #[test]
    fn signature_covers_timestamp_and_body() {
        let s = sign("secret", 1700000000, b"body");
        assert!(s.starts_with("v1="));
        assert_eq!(s, sign("secret", 1700000000, b"body"));
        assert_ne!(s, sign("secret", 1700000001, b"body"));
        assert_ne!(s, sign("other", 1700000000, b"body"));
    }

    #[test]
    fn logs_do_not_carry_query_or_credentials() {
        assert_eq!(
            redact("https://u:p@hooks.example.com/done?token=abc#f"),
            "https://hooks.example.com/done"
        );
    }

    #[tokio::test]
    async fn headers_parse_and_reserved_ones_are_refused() {
        let mut h = HeaderMap::new();
        h.insert(
            "gotenberg-webhook-url",
            "http://127.0.0.1:1/done".parse().unwrap(),
        );
        h.insert("x-rusticdoc-webhook-method", "put".parse().unwrap());
        h.insert(
            "gotenberg-webhook-extra-http-headers",
            r#"{"Authorization":"Bearer t"}"#.parse().unwrap(),
        );
        let hook = Webhook::from_headers(&h, &UrlPolicies::private(true))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hook.method, Method::PUT);
        assert_eq!(hook.extra_headers.get("authorization").unwrap(), "Bearer t");
        assert!(
            Webhook::from_headers(&HeaderMap::new(), &UrlPolicies::private(false))
                .await
                .unwrap()
                .is_none()
        );
        for bad in [
            r#"{"Host":"evil"}"#,
            r#"{"X-Rusticdoc-Signature":"v1=0"}"#,
            r#"{"Content-Length":"1"}"#,
        ] {
            h.insert("gotenberg-webhook-extra-http-headers", bad.parse().unwrap());
            assert!(
                Webhook::from_headers(&h, &UrlPolicies::private(true))
                    .await
                    .is_err(),
                "{bad}"
            );
        }
    }
}
