//! Bearer tokens from an OpenID Connect provider, when `API_ENABLE_OIDC_AUTH`
//! is on.
//!
//! A token passes when its signature checks out against the provider's keys
//! and its issuer, audience and expiry are right. The keys are fetched when the
//! first token arrives, from `API_OIDC_JWKS_URL` or the issuer's discovery
//! document, kept for a while, and fetched again when a token names a key they
//! lack. Anyone can send a token, so fetches are rationed: one at a time, and at
//! most one per [`REFETCH_AFTER`] whether it worked or not. Only asymmetric
//! algorithms are accepted, so a token cannot pick a weaker check for itself.
//! The URLs are the operator's, never a client's.

use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{Jwk, JwkSet, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use rusticdoc_core::config::{OidcConfig, is_secure_url};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

/// How long fetched keys are kept before a token makes them fresh again.
const KEYS_TTL: Duration = Duration::from_secs(300);
/// The provider is asked at most this often, however many tokens name keys it
/// has not listed and whether or not it answers, so garbage cannot turn into a
/// stream of requests to it.
const REFETCH_AFTER: Duration = Duration::from_secs(30);
/// Largest key set or discovery document read.
const MAX_DOCUMENT: usize = 1 << 20;

#[derive(Default)]
struct Keys {
    set: Option<JwkSet>,
    /// When the set was fetched.
    fetched: Option<Instant>,
    /// When a fetch was last started, successful or not.
    attempted: Option<Instant>,
}

pub struct Verifier {
    config: OidcConfig,
    http: reqwest::Client,
    keys: RwLock<Keys>,
    /// Held across a fetch, so concurrent tokens share one.
    fetching: Mutex<()>,
}

#[derive(Deserialize)]
struct Discovery {
    jwks_uri: String,
}

impl Verifier {
    pub fn new(config: OidcConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .expect("reqwest client");
        Self {
            config,
            http,
            keys: RwLock::default(),
            fetching: Mutex::new(()),
        }
    }

    /// Checks a token and returns its subject, or the token's own digest when it
    /// names none. The reason it fails is for the log, not the client.
    pub async fn verify(&self, token: &str) -> Result<String, String> {
        let header = decode_header(token).map_err(|e| e.to_string())?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
            return Err(format!("algorithm {:?} is not accepted", header.alg));
        }
        let jwk = self.key(header.kid.as_deref()).await?;
        let key = DecodingKey::from_jwk(&jwk).map_err(|e| e.to_string())?;
        let mut validation = Validation::new(header.alg);
        validation.set_audience(&[&self.config.audience]);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.validate_nbf = true;
        let claims = decode::<serde_json::Value>(token, &key, &validation)
            .map_err(|e| e.to_string())?
            .claims;
        Ok(match claims.get("sub").and_then(|sub| sub.as_str()) {
            Some(subject) => subject.to_string(),
            None => token.to_string(),
        })
    }

    /// The key a token names, fetching the set when needed and allowed.
    async fn key(&self, kid: Option<&str>) -> Result<Jwk, String> {
        if let Some(jwk) = self.cached(kid).await {
            return Ok(jwk);
        }
        let _fetching = self.fetching.lock().await;
        // Someone else may have fetched while this one waited.
        if let Some(jwk) = self.cached(kid).await {
            return Ok(jwk);
        }
        {
            let mut keys = self.keys.write().await;
            if keys
                .attempted
                .is_some_and(|at| at.elapsed() < REFETCH_AFTER)
            {
                return Err("the token names a key the provider does not list".into());
            }
            keys.attempted = Some(Instant::now());
        }
        let set = self.fetch().await?;
        let jwk = find_key(&set, kid);
        let mut keys = self.keys.write().await;
        keys.set = Some(set);
        keys.fetched = Some(Instant::now());
        jwk.ok_or_else(|| "the token names a key the provider does not list".to_string())
    }

    /// The key, from a set that is still fresh.
    async fn cached(&self, kid: Option<&str>) -> Option<Jwk> {
        let keys = self.keys.read().await;
        if !keys.fetched.is_some_and(|at| at.elapsed() < KEYS_TTL) {
            return None;
        }
        find_key(keys.set.as_ref()?, kid)
    }

    async fn fetch(&self) -> Result<JwkSet, String> {
        let url = if self.config.jwks_url.is_empty() {
            let discovery = format!(
                "{}/.well-known/openid-configuration",
                self.config.issuer.trim_end_matches('/')
            );
            let document: Discovery = self.get_json(&discovery).await?;
            document.jwks_uri
        } else {
            self.config.jwks_url.clone()
        };
        self.get_json(&url).await
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, String> {
        // Keys fetched in the clear could be anyone's. A discovery document
        // names its own key set URL, so this is checked here as well as at start.
        if !is_secure_url(url) {
            return Err(format!("'{url}' is not https, or http on a loopback host"));
        }
        let mut res = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if !res.status().is_success() {
            return Err(format!("status {} from the provider", res.status()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = res.chunk().await.map_err(|e| e.without_url().to_string())? {
            if body.len() + chunk.len() > MAX_DOCUMENT {
                return Err("the provider's document is too large".into());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    }
}

/// The signing key a token names. A key marked for anything but signatures is
/// not one; a key with no `use` may be, as the JOSE specifications leave it out.
fn find_key(set: &JwkSet, kid: Option<&str>) -> Option<Jwk> {
    let signs = |jwk: &&Jwk| {
        matches!(
            jwk.common.public_key_use,
            None | Some(PublicKeyUse::Signature)
        )
    };
    match kid {
        Some(kid) => set.find(kid).filter(signs).cloned(),
        None => set.keys.iter().find(signs).cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgmJ9nJkaP9Rzg5Wma
6ZS+eBkD12USL+y5CHfhMXkuEcahRANCAASsdQ+QOJ1w4B4l7IUIUui+zWM2m4Zj
Yfo0bRmNQhP1WmUPaBpWQtZEqxmMlQWeJzftl7o5GzJCX1oJHh9aL/HF
-----END PRIVATE KEY-----
";
    const JWKS: &str = r#"{"keys":[{"kty":"EC","crv":"P-256","kid":"one","use":"sig","alg":"ES256",
        "x":"rHUPkDidcOAeJeyFCFLovs1jNpuGY2H6NG0ZjUIT9Vo",
        "y":"ZQ9oGlZC1kSrGYyVBZ4nN-2XujkbMkJfWgkeH1ov8cU"}]}"#;

    fn token(aud: &str, iss: &str, exp_from_now: i64) -> String {
        token_with_kid("one", aud, iss, exp_from_now)
    }

    fn token_with_kid(kid: &str, aud: &str, iss: &str, exp_from_now: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(kid.into());
        encode(
            &header,
            &serde_json::json!({"aud": aud, "iss": iss, "exp": now + exp_from_now}),
            &EncodingKey::from_ec_pem(PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn tokens_are_checked_against_the_providers_keys() {
        let provider = Router::new().route("/jwks", get(|| async { JWKS }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, provider).await });

        let verifier = Verifier::new(OidcConfig {
            issuer: "https://issuer.test/".into(),
            audience: "rusticdoc".into(),
            jwks_url: format!("http://127.0.0.1:{port}/jwks"),
        });
        verifier
            .verify(&token("rusticdoc", "https://issuer.test/", 600))
            .await
            .unwrap();
        for bad in [
            token("someone-else", "https://issuer.test/", 600),
            token("rusticdoc", "https://elsewhere.test/", 600),
            token("rusticdoc", "https://issuer.test/", -600),
            "not.a.token".to_string(),
        ] {
            assert!(verifier.verify(&bad).await.is_err(), "{bad}");
        }
    }

    /// A provider on a local port that counts its requests.
    async fn provider(status: StatusCode, body: &'static str) -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let router = Router::new().route(
            "/jwks",
            get(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    (status, body)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await });
        (format!("http://127.0.0.1:{port}/jwks"), hits)
    }

    fn verifier_at(jwks_url: String) -> Verifier {
        Verifier::new(OidcConfig {
            issuer: "https://issuer.test/".into(),
            audience: "rusticdoc".into(),
            jwks_url,
        })
    }

    fn unknown(i: usize) -> String {
        token_with_kid(
            &format!("kid-{i}"),
            "rusticdoc",
            "https://issuer.test/",
            600,
        )
    }

    #[tokio::test]
    async fn unknown_keys_cannot_hammer_a_failing_provider() {
        let (url, hits) = provider(StatusCode::INTERNAL_SERVER_ERROR, "down").await;
        let verifier = verifier_at(url);
        for i in 0..5 {
            assert!(verifier.verify(&unknown(i)).await.is_err());
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unknown_keys_cannot_hammer_a_working_provider() {
        let (url, hits) = provider(StatusCode::OK, JWKS).await;
        let verifier = verifier_at(url);
        for i in 0..5 {
            assert!(verifier.verify(&unknown(i)).await.is_err());
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // The keys it did list stay usable without another request.
        verifier
            .verify(&token("rusticdoc", "https://issuer.test/", 600))
            .await
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_tokens_share_one_fetch() {
        let (url, hits) = provider(StatusCode::INTERNAL_SERVER_ERROR, "down").await;
        let verifier = Arc::new(verifier_at(url));
        let mut tasks = Vec::new();
        for i in 0..16 {
            let verifier = verifier.clone();
            tasks.push(tokio::spawn(async move {
                verifier.verify(&unknown(i)).await.is_err()
            }));
        }
        for task in tasks {
            assert!(task.await.unwrap());
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_document_that_never_ends_is_cut_off() {
        // Sent in chunks with no length up front, so only the running count can
        // catch it.
        let router = Router::new().route(
            "/jwks",
            get(|| async {
                let chunks = (0..64).map(|_| {
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(vec![b' '; 64 << 10]))
                });
                axum::body::Body::from_stream(futures_util::stream::iter(chunks))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await });
        let verifier = verifier_at(format!("http://127.0.0.1:{port}/jwks"));
        let err = verifier.verify(&unknown(0)).await.unwrap_err();
        assert!(err.contains("too large"), "{err}");
    }

    fn token_with(claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("one".into());
        encode(
            &header,
            &claims,
            &EncodingKey::from_ec_pem(PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[tokio::test]
    async fn a_token_not_yet_valid_is_refused_and_a_subject_comes_back() {
        let (url, _) = provider(StatusCode::OK, JWKS).await;
        let verifier = verifier_at(url);
        let claims = |nbf: i64| {
            serde_json::json!({
                "aud": "rusticdoc", "iss": "https://issuer.test/",
                "exp": now() + 600, "nbf": nbf, "sub": "ada",
            })
        };
        let early = token_with(claims(now() + 500));
        assert!(verifier.verify(&early).await.is_err());
        assert_eq!(
            verifier.verify(&token_with(claims(now()))).await.unwrap(),
            "ada"
        );
    }

    #[tokio::test]
    async fn keys_marked_for_encryption_do_not_verify() {
        const ENC: &str = r#"{"keys":[{"kty":"EC","crv":"P-256","kid":"one","use":"enc",
            "x":"rHUPkDidcOAeJeyFCFLovs1jNpuGY2H6NG0ZjUIT9Vo",
            "y":"ZQ9oGlZC1kSrGYyVBZ4nN-2XujkbMkJfWgkeH1ov8cU"}]}"#;
        let (url, _) = provider(StatusCode::OK, ENC).await;
        let verifier = verifier_at(url);
        assert!(
            verifier
                .verify(&token("rusticdoc", "https://issuer.test/", 600))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn keys_are_not_fetched_in_the_clear_from_elsewhere() {
        let verifier = verifier_at("http://keys.example.com/jwks".into());
        let err = verifier
            .verify(&token("rusticdoc", "https://issuer.test/", 600))
            .await
            .unwrap_err();
        assert!(err.contains("loopback"), "{err}");
    }

    #[tokio::test]
    async fn symmetric_tokens_are_refused_before_any_fetch() {
        let verifier = Verifier::new(OidcConfig {
            issuer: "https://issuer.test/".into(),
            audience: "rusticdoc".into(),
            jwks_url: "http://127.0.0.1:1/unreachable".into(),
        });
        let hs256 = encode(
            &Header::new(Algorithm::HS256),
            &serde_json::json!({"aud": "rusticdoc"}),
            &EncodingKey::from_secret(b"guess"),
        )
        .unwrap();
        let err = verifier.verify(&hs256).await.unwrap_err();
        assert!(err.contains("not accepted"), "{err}");
    }
}
