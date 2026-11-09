//! The server's side of PDF signing: the outbound client that timestamps and
//! validation data go through, and who may sign with which key.

use std::time::Duration;

use rusticdoc_core::Config;
use rusticdoc_core::pdfops::sign::net::upstream;
use rusticdoc_core::pdfops::sign::{Outbound, OutboundRequest, SignOptions, Signing};

use crate::auth::Principal;
use crate::error::ApiError;
use crate::webhook::{UrlPolicy, client_for, redact, resolve};

/// Requests to the timestamp authority, OCSP responders, CRL distribution
/// points and certificate issuers, under the same rules as webhooks: the URL
/// must pass `RUSTICDOC_SIGNING_OUTBOUND_ALLOW_LIST` and `_DENY_LIST`, private
/// addresses are refused unless `RUSTICDOC_SIGNING_OUTBOUND_ALLOW_PRIVATE` is
/// set, the connection is pinned to the addresses checked, redirects are not
/// followed, and `RUSTICDOC_OUTBOUND_PROXY` is used when it is set.
#[derive(Debug)]
pub struct PolicyOutbound {
    policy: UrlPolicy,
    proxy: Option<String>,
}

impl PolicyOutbound {
    /// From the settings. [`Config::check`] has seen the patterns.
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let signing = &config.signing;
        Ok(Self {
            policy: UrlPolicy::compile(
                signing.outbound_allow_private,
                &signing.outbound_allow_list,
                &signing.outbound_deny_list.iter().collect::<Vec<_>>(),
            )?,
            proxy: config.outbound_proxy.clone(),
        })
    }
}

#[async_trait::async_trait]
impl Outbound for PolicyOutbound {
    async fn fetch(&self, request: OutboundRequest) -> rusticdoc_core::Result<Vec<u8>> {
        let purpose = request.purpose;
        let shown = redact(&request.url);
        let failed = |what: String| upstream(purpose, format!("{shown}: {what}"));
        let target = resolve(&request.url, &self.policy)
            .await
            .map_err(|e| failed(e.body.message.replace("webhook ", "")))?;
        let client = client_for(&target, request.timeout, self.proxy.as_deref())
            .map_err(|e| failed(e.without_url().to_string()))?;
        let mut builder = match &request.body {
            Some(body) => client.post(&target.url).body(body.clone()),
            None => client.get(&target.url),
        };
        if let Some(content_type) = request.content_type {
            builder = builder.header("content-type", content_type);
        }
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some((user, password)) = &request.basic_auth {
            builder = builder.basic_auth(user, Some(password));
        }
        let work = async {
            let mut res = builder
                .send()
                .await
                .map_err(|e| failed(e.without_url().to_string()))?;
            if !res.status().is_success() {
                return Err(failed(format!("it answered {}", res.status().as_u16())));
            }
            let limit = request.max_bytes;
            if res.content_length().unwrap_or(0) as usize > limit {
                return Err(failed(format!("the answer is larger than {limit} bytes")));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = res
                .chunk()
                .await
                .map_err(|e| failed(e.without_url().to_string()))?
            {
                if bytes.len() + chunk.len() > limit {
                    return Err(failed(format!("the answer is larger than {limit} bytes")));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        };
        // The client's timeout covers each read; this covers the whole answer.
        tokio::time::timeout(request.timeout + Duration::from_millis(100), work)
            .await
            .map_err(|_| failed(format!("no answer within {:?}", request.timeout)))?
    }
}

/// True when the caller `principal` may sign with a key whose allow list is
/// `allow`. An entry matches the principal string exactly (`key:<sha256>`,
/// `basic:<sha256>`, `oidc:<sha256>`, `tenant:<name>`, `public`), `prefix*`
/// matches every principal that starts with the prefix, and `*` matches every
/// caller.
pub fn may_use_signing_key(principal: &str, allow: &[String]) -> bool {
    allow.iter().any(|entry| match entry.strip_suffix('*') {
        Some(prefix) => principal.starts_with(prefix),
        None => entry == principal,
    })
}

/// Refuses, with 403, a caller that may not use the key `options` names. A
/// key that is not configured is left to [`Signing::check`], which runs after
/// the step order is checked.
pub fn authorize(
    signing: &Signing,
    principal: &Principal,
    options: &SignOptions,
) -> Result<(), ApiError> {
    let Ok(allow) = signing.allow_list(options.key.as_deref()) else {
        return Ok(());
    };
    if let Some(allow) = allow
        && !may_use_signing_key(&principal.0, allow)
    {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "signing_key_forbidden",
            match &options.key {
                Some(name) => format!("this caller may not sign with the key '{name}'"),
                None => "this caller may not sign with the default key".to_string(),
            },
        ));
    }
    Ok(())
}

/// [`authorize`] for every `sign` step of a step list.
pub fn authorize_steps(
    signing: &Signing,
    principal: &Principal,
    steps: &[rusticdoc_core::Step],
) -> Result<(), ApiError> {
    for step in steps {
        if let rusticdoc_core::Step::Sign { options } = step {
            authorize(signing, principal, options)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::may_use_signing_key;

    #[test]
    fn allow_lists_match_exactly_by_prefix_or_everyone() {
        let allow = |entries: &[&str]| entries.iter().map(|e| e.to_string()).collect::<Vec<_>>();
        assert!(may_use_signing_key("tenant:acme", &allow(&["tenant:acme"])));
        assert!(!may_use_signing_key(
            "tenant:acme2",
            &allow(&["tenant:acme"])
        ));
        assert!(may_use_signing_key("tenant:acme2", &allow(&["tenant:*"])));
        assert!(!may_use_signing_key("key:abc", &allow(&["tenant:*"])));
        assert!(may_use_signing_key("public", &allow(&["*"])));
        assert!(!may_use_signing_key("public", &allow(&[])));
    }
}
