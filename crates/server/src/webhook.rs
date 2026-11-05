//! Asynchronous delivery of results.
//!
//! A request that carries `X-Rusticdoc-Webhook-Url` (or Gotenberg's
//! `Gotenberg-Webhook-Url`) returns 204 at once; the result is then posted to
//! that URL, with an HMAC-SHA256 signature when `RUSTICDOC_WEBHOOK_SECRET` is
//! set. Failures go to the error URL as JSON. Deliveries retry with backoff.
//!
//! Targets are resolved when the request arrives, every resolved address is
//! checked against private ranges, and the delivery is pinned to those
//! addresses with redirects disabled, so neither DNS rebinding nor a redirect
//! can turn a webhook into a request to something beside the service.

use std::net::{IpAddr, SocketAddr};
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

/// A vetted destination: the URL, and the addresses it resolved to when the
/// request came in.
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

/// Checks the URL's shape, resolves its host, and refuses any private,
/// loopback, link-local or similar address unless allowed.
pub async fn resolve(url: &str, allow_private: bool) -> Result<Target, ApiError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| ApiError::bad_request(format!("webhook URL '{url}': {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(format!(
            "webhook URL '{url}' must be http or https"
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ApiError::bad_request(format!("webhook URL '{url}' has no host")))?
        .trim_matches(|c| c == '[' || c == ']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| ApiError::bad_request(format!("webhook URL '{url}' has no port")))?;

    if !allow_private && is_private_name(&host) {
        return Err(private(url));
    }
    let addrs: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| {
                ApiError::bad_request(format!("webhook host '{host}' does not resolve: {e}"))
            })?
            .collect(),
    };
    if addrs.is_empty() {
        return Err(ApiError::bad_request(format!(
            "webhook host '{host}' resolves to no address"
        )));
    }
    if !allow_private && addrs.iter().any(|a| is_private_ip(a.ip())) {
        return Err(private(url));
    }
    Ok(Target {
        url: url.to_string(),
        host,
        addrs,
    })
}

fn private(url: &str) -> ApiError {
    ApiError::bad_request(format!(
        "webhook URL '{url}' points at a private address; set RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true to allow it"
    ))
}

fn is_private_name(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".home.arpa")
}

/// Loopback, private, link-local, carrier-grade NAT, unspecified, broadcast,
/// unique-local and IPv4-mapped forms of any of those.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let s = ip.segments();
            ip.is_loopback()
                || ip.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] == 0x2002
                    && is_private_ip(IpAddr::V4(std::net::Ipv4Addr::new(
                        (s[1] >> 8) as u8,
                        s[1] as u8,
                        (s[2] >> 8) as u8,
                        s[2] as u8,
                    ))))
        }
    }
}

/// `sha256=<hex hmac>` over the body.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
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
    let client = match client_for(&target, state.config.api_timeout) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(%url, error = %e, "webhook client could not be built");
            count(&state, "gave_up");
            return;
        }
    };

    let attempts = state.config.webhook_attempts.max(1);
    for attempt in 1..=attempts {
        let mut req = client
            .request(hook.method.clone(), &url)
            .header("content-type", &content_type)
            .header(EVENT, event)
            .header("x-request-id", &request_id)
            .header(TIMESTAMP, unix_now().to_string())
            .header("x-rusticdoc-attempt", attempt.to_string());
        if let Some(d) = &disposition {
            req = req.header("content-disposition", d);
        }
        if let Some(secret) = &state.config.webhook_secret {
            req = req.header(SIGNATURE, sign(secret, &body));
        }
        for (k, v) in hook.extra_headers.iter() {
            req = req.header(k, v);
        }
        match req.body(body.clone()).send().await {
            Ok(res) if res.status().is_success() => {
                tracing::info!(%url, event, attempt, status = res.status().as_u16(), "webhook delivered");
                count(&state, "delivered");
                return;
            }
            Ok(res) => {
                tracing::warn!(%url, event, attempt, status = res.status().as_u16(), "webhook rejected");
            }
            Err(e) => {
                tracing::warn!(%url, event, attempt, error = %e, "webhook failed");
            }
        }
        count(&state, "failed");
        if attempt < attempts {
            tokio::time::sleep(Duration::from_secs(3u64.pow(attempt - 1))).await;
        }
    }
    tracing::error!(%url, event, attempts, "webhook gave up");
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
    fn private_ranges_include_cgnat_mapped_and_ula() {
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
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "2002:0a00:0001::",
        ] {
            assert!(is_private_ip(ip(p)), "{p} should be private");
        }
        for p in [
            "8.8.8.8",
            "100.128.0.1",
            "172.32.0.1",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_private_ip(ip(p)), "{p} should be public");
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
        let t = resolve("http://127.0.0.1:9000/x", true).await.unwrap();
        assert_eq!(
            t.addrs,
            vec!["127.0.0.1:9000".parse::<SocketAddr>().unwrap()]
        );
        assert_eq!(t.host, "127.0.0.1");
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_loopback_is_refused() {
        // `localhost` is caught by name; this resolves it as a name would be.
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host(("localhost", 80))
            .await
            .map(|a| a.collect())
            .unwrap_or_default();
        for a in addrs {
            assert!(
                is_private_ip(a.ip()),
                "{a} resolved from localhost is private"
            );
        }
    }

    #[test]
    fn signature_is_deterministic_and_prefixed() {
        let s = sign("secret", b"body");
        assert!(s.starts_with("sha256="));
        assert_eq!(s, sign("secret", b"body"));
        assert_ne!(s, sign("other", b"body"));
    }

    #[tokio::test]
    async fn headers_parse() {
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
    }
}
