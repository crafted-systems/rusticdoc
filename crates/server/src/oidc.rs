//! Bearer tokens from an OpenID Connect provider, when `API_ENABLE_OIDC_AUTH`
//! is on.
//!
//! A token passes when its signature checks out against the provider's keys
//! and its issuer, audience and expiry are right. The keys are fetched when the
//! first token arrives, from `API_OIDC_JWKS_URL` or the issuer's discovery
//! document, kept for a while, and fetched again when a token names a key they
//! lack. Only asymmetric algorithms are accepted, so a token cannot pick a
//! weaker check for itself. The URLs are the operator's, never a client's.

use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use rusticdoc_core::config::OidcConfig;
use serde::Deserialize;
use tokio::sync::RwLock;

/// How long fetched keys are kept before a token makes them fresh again.
const KEYS_TTL: Duration = Duration::from_secs(300);
/// A token naming an unknown key refetches at most this often, so garbage
/// cannot turn into a stream of requests to the provider.
const REFETCH_AFTER: Duration = Duration::from_secs(30);
/// Largest key set or discovery document read.
const MAX_DOCUMENT: usize = 1 << 20;

#[derive(Default)]
struct Keys {
    set: Option<JwkSet>,
    fetched: Option<Instant>,
}

pub struct Verifier {
    config: OidcConfig,
    http: reqwest::Client,
    keys: RwLock<Keys>,
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
            .build()
            .expect("reqwest client");
        Self {
            config,
            http,
            keys: RwLock::default(),
        }
    }

    /// Checks a token. The reason it fails is for the log, not the client.
    pub async fn verify(&self, token: &str) -> Result<(), String> {
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
        decode::<serde_json::Value>(token, &key, &validation)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The key a token names, fetching the set when needed.
    async fn key(&self, kid: Option<&str>) -> Result<Jwk, String> {
        let find = |set: &JwkSet| match kid {
            Some(kid) => set.find(kid).cloned(),
            None => set.keys.first().cloned(),
        };
        {
            let keys = self.keys.read().await;
            let fresh = keys.fetched.is_some_and(|at| at.elapsed() < KEYS_TTL);
            if fresh && let Some(jwk) = keys.set.as_ref().and_then(find) {
                return Ok(jwk);
            }
            if keys.fetched.is_some_and(|at| at.elapsed() < REFETCH_AFTER) {
                return Err("the token names a key the provider does not list".into());
            }
        }
        let set = self.fetch().await?;
        let jwk = find(&set);
        *self.keys.write().await = Keys {
            set: Some(set),
            fetched: Some(Instant::now()),
        };
        jwk.ok_or_else(|| "the token names a key the provider does not list".to_string())
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
        let res = self.http.get(url).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("status {} from the provider", res.status()));
        }
        if res.content_length().unwrap_or(0) as usize > MAX_DOCUMENT {
            return Err("the provider's document is too large".into());
        }
        let body = res.bytes().await.map_err(|e| e.to_string())?;
        if body.len() > MAX_DOCUMENT {
            return Err("the provider's document is too large".into());
        }
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;
    use jsonwebtoken::{EncodingKey, Header, encode};

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
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("one".into());
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
