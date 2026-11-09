//! Asynchronous delivery of results.
//!
//! A request that carries `X-Rusticdoc-Webhook-Url` returns 204 at once; the
//! result is then posted to
//! that URL, signed when `RUSTICDOC_WEBHOOK_SECRET` is set. Failures go to
//! the error URL as JSON. Deliveries retry with backoff.
//!
//! Targets are resolved when the request arrives, every resolved address is
//! checked against non-global ranges, the URL is rebuilt from the normalised
//! host, and the delivery is pinned to those addresses with redirects
//! disabled, so neither DNS rebinding nor a redirect can turn a webhook into
//! a request to something beside the service.
//!
//! Each caller signs with its own key, so one tenant cannot send another's
//! receiver something that verifies: the key is `HMAC-SHA256(secret, principal)`,
//! or the secret itself when the service requires no credentials and the
//! principal is `public`. `rusticdoc webhook-secret` prints a caller's key.
//!
//! The signature covers the timestamp, the event, the request id and the body:
//! `X-Rusticdoc-Signature: v1=<hex HMAC-SHA256 of
//! "<timestamp>.<event>.<request id>.<body>">`, with the timestamp in
//! `X-Rusticdoc-Timestamp` and the event in `X-Rusticdoc-Event`. Receivers verify
//! the signature and reject timestamps outside their freshness window.

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
use crate::jobs::{Delivery, JobPermit};
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
        let hook = config.webhook_allow_private;
        let error_allow = if config.webhook_error_allow_list.is_empty() {
            &config.webhook_allow_list
        } else {
            &config.webhook_error_allow_list
        };
        let error_deny: Vec<&String> = config
            .webhook_deny_list
            .iter()
            .chain(&config.webhook_error_deny_list)
            .collect();
        Ok(Self {
            webhook: UrlPolicy::compile(
                hook,
                &config.webhook_allow_list,
                &config.webhook_deny_list.iter().collect::<Vec<_>>(),
            )?,
            webhook_error: UrlPolicy::compile(hook, error_allow, &error_deny)?,
            download: UrlPolicy::compile(
                config.download_from_allow_private,
                &config.download_allow_list,
                &config.download_deny_list.iter().collect::<Vec<_>>(),
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
    /// This job's place among the running ones, held through its delivery.
    pub permit: Option<Arc<JobPermit>>,
    /// The caller, whose key signs the delivery.
    pub principal: String,
}

/// How a `/v1/*` request's result is to be delivered: in the response, to the
/// webhook it names, or into the store as a background job when it says
/// `Prefer: respond-async`. A webhook or a job comes with its place among the
/// running jobs. As an extractor it refuses before the body is read.
pub struct Hook(pub Delivery);

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
        let background = crate::jobs::prefers_async(&parts.headers);
        // `RUSTICDOC_WEBHOOK_DISABLED` means the headers are ignored.
        let webhook_asked =
            !state.config.webhook_disabled && parts.headers.contains_key("x-rusticdoc-webhook-url");
        // Before the URL is resolved: the request is wrong whatever it names.
        if background && webhook_asked {
            return Err(ApiError::bad_request(
                "ask for a webhook or for 'Prefer: respond-async', not both",
            ));
        }
        let hook = if webhook_asked {
            Webhook::from_headers(&parts.headers, &state.url_policies).await?
        } else {
            None
        };
        match (hook, background) {
            (Some(mut hook), _) => {
                hook.permit = Some(Arc::new(state.jobs.try_acquire(caller)?));
                hook.principal = caller.to_string();
                Ok(Self(Delivery::Webhook(Box::new(hook))))
            }
            (None, true) => Ok(Self(Delivery::Job(crate::jobs::ticket(state, caller)?))),
            (None, false) => Ok(Self(Delivery::Now)),
        }
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
        let get = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let Some(url) = get("x-rusticdoc-webhook-url") else {
            return Ok(None);
        };
        let target = resolve(&url, &policies.webhook).await?;
        let error_target = match get("x-rusticdoc-webhook-error-url") {
            Some(e) => Some(resolve(&e, &policies.webhook_error).await?),
            None => None,
        };
        let method = match get("x-rusticdoc-webhook-method") {
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
        if let Some(raw) = get("x-rusticdoc-webhook-extra-headers") {
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
            permit: None,
            principal: "public".to_string(),
        }))
    }
}

/// Checks the URL's shape, resolves its host, and refuses any address that
/// is not global unicast unless allowed. The returned URL carries the
/// normalised host, which is also the key the delivery client pins.
pub async fn resolve(url: &str, policy: &UrlPolicy) -> Result<Target, ApiError> {
    let allow_private = policy.allow_private;
    // Messages are logged, and a URL can carry what the logs should not: every
    // one of them shows it redacted.
    let shown = redact(url);
    let mut parsed = reqwest::Url::parse(url)
        .map_err(|e| ApiError::bad_request(format!("webhook URL {shown}: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(format!(
            "webhook URL '{shown}' must be http or https"
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ApiError::bad_request(format!(
            "webhook URL '{shown}' must not carry credentials; use extra headers"
        )));
    }
    let raw_host = parsed
        .host_str()
        .ok_or_else(|| ApiError::bad_request(format!("webhook URL '{shown}' has no host")))?
        .to_string();
    let bare = raw_host
        .trim_matches(|c| c == '[' || c == ']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| ApiError::bad_request(format!("webhook URL '{shown}' has no port")))?;

    // The URL the client sends and the key it pins must be one string, and the
    // lists judge that string too: what they see is exactly what gets fetched, so
    // `evil.com.` cannot pass as something other than `evil.com`.
    let canonical = match bare.parse::<IpAddr>() {
        Ok(ip) => ip.to_string(),
        Err(_) => bare.clone(),
    };
    let host_for_url = if canonical.contains(':') {
        format!("[{canonical}]")
    } else {
        canonical
    };
    parsed
        .set_host(Some(&host_for_url))
        .map_err(|_| ApiError::bad_request(format!("webhook host '{bare}' is not valid")))?;
    if !policy.permits(parsed.as_str()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "url_not_allowed",
            format!("URL '{}' is not allowed on this service", redact(url)),
        ));
    }
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
            // IPv4-translated (::ffff:0:0/96), local-use NAT64 (64:ff9b:1::/48) and
            // discard (100::/64) lead nowhere public.
            if s[..4] == [0, 0, 0, 0] && s[4] == 0xffff && s[5] == 0
                || s[..3] == [0x64, 0xff9b, 1]
                || s[..4] == [0x100, 0, 0, 0]
            {
                return false;
            }
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

/// The key `principal`'s deliveries are signed with: `HMAC-SHA256(secret,
/// principal)`, or the secret itself for `public`, where no credentials are
/// configured and there is one tenant.
pub fn signing_key(secret: &str, principal: &str) -> Vec<u8> {
    if principal == "public" {
        return secret.as_bytes().to_vec();
    }
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(principal.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// `v1=<hex hmac>` over `"<timestamp>.<event>.<request id>.<body>"`.
pub fn sign(key: &[u8], timestamp: u64, event: &str, request_id: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(format!("{timestamp}.{event}.{request_id}.").as_bytes());
    mac.update(body);
    format!("v1={}", hex::encode(mac.finalize().into_bytes()))
}

/// A client pinned to the target's vetted addresses, following no redirects.
///
/// Proxy variables in the environment are ignored: a proxy would resolve the name
/// itself and undo the pinning. `RUSTICDOC_OUTBOUND_PROXY` asks for one explicitly,
/// and then the URL checks still apply but the pinning does not.
pub fn client_for(
    target: &Target,
    timeout: Duration,
    proxy: Option<&str>,
) -> reqwest::Result<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .user_agent(format!("rusticdoc/{}", rusticdoc_core::VERSION))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&target.host, &target.addrs)
        .no_proxy();
    match proxy {
        Some(proxy) => builder.proxy(reqwest::Proxy::all(proxy)?).build(),
        None => builder.build(),
    }
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
    let method = hook.method.clone();
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
    let client = match client_for(
        &target,
        state.config.api_timeout,
        state.config.outbound_proxy.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(url = %shown, error = %e, "webhook client could not be built");
            count(&state, "gave_up");
            return;
        }
    };
    let key = state
        .config
        .webhook_secret
        .as_ref()
        .map(|secret| signing_key(secret, &hook.principal));

    let attempts = state.config.webhook_attempts.max(1);
    for attempt in 1..=attempts {
        let timestamp = unix_now();
        let mut req = client
            .request(method.clone(), &url)
            .header("content-type", &content_type)
            .header(EVENT, event)
            .header("x-request-id", &request_id)
            .header(TIMESTAMP, timestamp.to_string())
            .header("x-rusticdoc-attempt", attempt.to_string());
        // The caller chose this destination, so it may name the file for it.
        if let Some(d) = &disposition
            && !hook.extra_headers.contains_key("content-disposition")
        {
            req = req.header("content-disposition", d);
        }
        if let Some(key) = &key {
            req = req.header(SIGNATURE, sign(key, timestamp, event, &request_id, &body));
        }
        for (k, v) in hook.extra_headers.iter() {
            req = req.header(k, v);
        }
        match req.body(body.clone()).send().await {
            Ok(res) if res.status().is_success() => {
                tracing::info!(url = %shown, event, attempt, status = res.status().as_u16(), "webhook delivered");
                count(&state, "delivered");
                return;
            }
            Ok(res) => {
                tracing::warn!(url = %shown, event, attempt, status = res.status().as_u16(), "webhook rejected");
            }
            Err(e) => {
                let e = e.without_url();
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
            "::ffff:0:7f00:1",
            "::ffff:0:808:808",
            "64:ff9b:1::1",
            "64:ff9b:1:abcd::808:808",
            "100::1",
            "100::ffff:ffff:ffff:ffff",
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
    async fn the_lists_judge_the_url_as_it_will_be_fetched() {
        let deny = lists(&[], &["^https://evil\\.example/"], true);
        for url in [
            "https://evil.example./x",
            "https://EVIL.example/x",
            "https://Evil.Example./x",
            "https://evil.example../x",
        ] {
            let err = resolve(url, &deny).await.unwrap_err();
            assert_eq!(err.status, StatusCode::FORBIDDEN, "{url}");
        }
        // IP literals are judged in their canonical form.
        let deny = lists(&[], &["^http://\\[2606:4700::1111\\]/"], true);
        let err = resolve("http://[2606:4700:0:0:0:0:0:1111]/x", &deny)
            .await
            .unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        // An allow list reads the same string, and that string is what is stored.
        let allow = lists(&["^http://1\\.1\\.1\\.1/"], &[], false);
        let target = resolve("http://1.1.1.1./x", &allow).await.unwrap();
        assert_eq!(target.url, "http://1.1.1.1/x");
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
    fn signature_covers_timestamp_event_request_id_and_body() {
        // Computed apart from this code, with Python's hmac module.
        assert_eq!(
            sign(b"k", 1700000000, "render.completed", "rid-1", b"body"),
            "v1=e05cfcf77b34600c9118e4239910dece12b05df5186b8c3b25cf6b8e0286195e"
        );
        let s = sign(b"k", 1700000000, "e", "r", b"b");
        assert_ne!(s, sign(b"k", 1700000001, "e", "r", b"b"));
        assert_ne!(s, sign(b"k", 1700000000, "x", "r", b"b"));
        assert_ne!(s, sign(b"k", 1700000000, "e", "x", b"b"));
        assert_ne!(s, sign(b"k", 1700000000, "e", "r", b"x"));
        assert_ne!(s, sign(b"x", 1700000000, "e", "r", b"b"));
    }

    #[test]
    fn each_caller_has_its_own_signing_key() {
        assert_eq!(
            hex::encode(signing_key("master", "key:abc")),
            "fed23b5bf025ca341ad9baafe0fa4f6da51c5db6e789ba6be0377bc09e42f448"
        );
        assert_ne!(signing_key("m", "key:a"), signing_key("m", "key:b"));
        assert_ne!(signing_key("m", "key:a"), b"m".to_vec());
        assert_eq!(signing_key("m", "public"), b"m".to_vec());
    }

    #[tokio::test]
    async fn refusals_do_not_echo_query_or_credentials() {
        for url in [
            "http://user:pw@hooks.example.com/x?token=SECRET",
            "ftp://user:pw@hooks.example.com/x?token=SECRET",
            "http://127.0.0.1/x?token=SECRET",
        ] {
            let err = resolve(url, &UrlPolicy::private(false)).await.unwrap_err();
            let message = err.body.message;
            assert!(!message.contains("SECRET"), "{message}");
            assert!(!message.contains("pw"), "{message}");
        }
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
            "x-rusticdoc-webhook-url",
            "http://127.0.0.1:1/done".parse().unwrap(),
        );
        h.insert("x-rusticdoc-webhook-method", "put".parse().unwrap());
        h.insert(
            "x-rusticdoc-webhook-extra-headers",
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
            h.insert("x-rusticdoc-webhook-extra-headers", bad.parse().unwrap());
            assert!(
                Webhook::from_headers(&h, &UrlPolicies::private(true))
                    .await
                    .is_err(),
                "{bad}"
            );
        }
    }
}
