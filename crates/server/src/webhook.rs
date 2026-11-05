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

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use rusticdoc_core::Output;
use sha2::Sha256;

use crate::error::ApiError;
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
    "content-disposition",
    "transfer-encoding",
    "connection",
    "x-request-id",
];

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
}

impl Webhook {
    /// Reads the webhook headers and resolves their hosts; `Ok(None)` when
    /// the request is synchronous.
    pub async fn from_headers(
        headers: &HeaderMap,
        allow_private: bool,
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
        let target = resolve(&url, allow_private).await?;
        let error_target = match get(&[
            "x-rusticdoc-webhook-error-url",
            "gotenberg-webhook-error-url",
        ]) {
            Some(e) => Some(resolve(&e, allow_private).await?),
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
        }))
    }
}

/// Checks the URL's shape, resolves its host, and refuses any address that
/// is not global unicast unless allowed. The returned URL carries the
/// normalised host, which is also the key the delivery client pins.
pub async fn resolve(url: &str, allow_private: bool) -> Result<Target, ApiError> {
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
fn client_for(target: &Target, timeout: Duration) -> reqwest::Result<reqwest::Client> {
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
    let (target, event, body, content_type, disposition) = match &result {
        Ok(out) => (
            hook.target.clone(),
            "render.completed",
            out.bytes.to_vec(),
            out.content_type.to_string(),
            Some(format!(
                "attachment; filename=\"{}\"",
                out.name.replace('"', "")
            )),
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
            return;
        }
    };

    let attempts = state.config.webhook_attempts.max(1);
    for attempt in 1..=attempts {
        let timestamp = unix_now();
        let mut req = client
            .request(hook.method.clone(), &url)
            .header("content-type", &content_type)
            .header(EVENT, event)
            .header("x-request-id", &request_id)
            .header(TIMESTAMP, timestamp.to_string())
            .header("x-rusticdoc-attempt", attempt.to_string());
        if let Some(d) = &disposition {
            req = req.header("content-disposition", d);
        }
        if let Some(secret) = &state.config.webhook_secret {
            req = req.header(SIGNATURE, sign(secret, timestamp, &body));
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
        assert!(resolve("http://127.0.0.1:9000/x", false).await.is_err());
        assert!(
            resolve("http://[::ffff:127.0.0.1]:9000/x", false)
                .await
                .is_err()
        );
        assert!(resolve("http://10.1.2.3/x", false).await.is_err());
        assert!(resolve("http://localhost/x", false).await.is_err());
        assert!(resolve("http://LOCALHOST./x", false).await.is_err());
        assert!(resolve("http://api.internal/x", false).await.is_err());
        assert!(resolve("ftp://hooks.example.com/x", false).await.is_err());
        assert!(
            resolve("http://user:pw@hooks.example.com/x", true)
                .await
                .is_err()
        );
        let t = resolve("http://127.0.0.1:9000/x", true).await.unwrap();
        assert_eq!(
            t.addrs,
            vec!["127.0.0.1:9000".parse::<SocketAddr>().unwrap()]
        );
        assert_eq!(t.host, "127.0.0.1");
    }

    #[tokio::test]
    async fn the_url_is_rebuilt_from_the_normalised_host() {
        let t = resolve("http://LocalHost.:9000/path?x=1", true)
            .await
            .unwrap();
        assert_eq!(t.host, "localhost");
        assert_eq!(t.url, "http://localhost:9000/path?x=1");
        let t = resolve("http://[::1]:9000/p", true).await.unwrap();
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
        let hook = Webhook::from_headers(&h, true).await.unwrap().unwrap();
        assert_eq!(hook.method, Method::PUT);
        assert_eq!(hook.extra_headers.get("authorization").unwrap(), "Bearer t");
        assert!(
            Webhook::from_headers(&HeaderMap::new(), false)
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
            assert!(Webhook::from_headers(&h, true).await.is_err(), "{bad}");
        }
    }
}
