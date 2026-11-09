//! The signing settings beyond the default key, read from the environment.
//!
//! Named keys: `RUSTICDOC_SIGNING_KEYS=acme,globex` lists them, and each name
//! has `RUSTICDOC_SIGNING_KEY_<NAME>`, `RUSTICDOC_SIGNING_CERT_<NAME>` and
//! `RUSTICDOC_SIGNING_ALLOW_<NAME>`, where `<NAME>` is the name in capitals
//! with `-` as `_`. A key variable holds a PEM file's path, or `pkcs11:` and
//! the label of a key on the token `RUSTICDOC_PKCS11_*` describes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::Level;
use super::keys::Signer;
use super::pkcs11::{Pkcs11Settings, TokenKeySpec};
use super::revocation::Limits;
use super::tsa::Tsa;
use crate::config::{parse_bytes, parse_duration};

/// A key besides the default one, chosen with `signingKey=<name>`.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedKey {
    pub name: String,
    /// A PEM file's path, or `pkcs11:...`.
    pub key: String,
    pub cert: PathBuf,
    /// Principals that may use it: exact, `prefix*`, or `*` for every caller.
    pub allow: Vec<String>,
}

/// Everything about signing but the default key's two paths, which
/// [`crate::Config`] keeps as `signing_key` and `signing_cert`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    /// `RUSTICDOC_SIGNING_ALLOW`: who may use the default key. Every caller
    /// when unset.
    pub default_allow: Option<Vec<String>>,
    pub named: Vec<NamedKey>,
    pub pkcs11: Pkcs11Settings,
    pub tsa: Option<Tsa>,
    /// The level a request gets when it names none.
    pub level: Level,
    pub revocation: Limits,
    /// Patterns a timestamp, OCSP, CRL or issuer URL must match one of.
    pub outbound_allow_list: Vec<String>,
    /// Patterns none of those URLs may match.
    pub outbound_deny_list: Vec<String>,
    /// Allow those URLs to reach private and loopback addresses.
    pub outbound_allow_private: bool,
}

/// A key name: letters, digits, `-` and `_`, up to 64.
pub(super) fn valid_key_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn list(value: Option<String>) -> Vec<String> {
    value
        .map(|v| {
            v.split(',')
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

impl Settings {
    /// Reads the `RUSTICDOC_SIGNING_*` and `RUSTICDOC_PKCS11_*` variables
    /// through `vars`, which looks one up by its full name, and adds what is
    /// wrong to `problems`.
    pub fn from_vars(vars: &dyn Fn(&str) -> Option<String>, problems: &mut Vec<String>) -> Self {
        let get = |name: &str| vars(name).filter(|v| !v.trim().is_empty());
        let mut s = Settings::default();

        if let Some(v) = get("RUSTICDOC_SIGNING_ALLOW") {
            let allow = list(Some(v));
            if allow.is_empty() {
                problems.push("RUSTICDOC_SIGNING_ALLOW is set but names no caller".into());
            }
            s.default_allow = Some(allow);
        }
        let mut suffixes = Vec::new();
        for name in list(get("RUSTICDOC_SIGNING_KEYS")) {
            if !valid_key_name(&name) {
                problems.push(format!(
                    "RUSTICDOC_SIGNING_KEYS names '{name}'; a key name is letters, digits, '-' and '_', up to 64"
                ));
                continue;
            }
            let suffix = name.to_ascii_uppercase().replace('-', "_");
            if suffixes.contains(&suffix) {
                problems.push(format!(
                    "RUSTICDOC_SIGNING_KEYS names two keys that both read RUSTICDOC_SIGNING_KEY_{suffix}"
                ));
                continue;
            }
            suffixes.push(suffix.clone());
            let key = get(&format!("RUSTICDOC_SIGNING_KEY_{suffix}"));
            let cert = get(&format!("RUSTICDOC_SIGNING_CERT_{suffix}"));
            let allow = get(&format!("RUSTICDOC_SIGNING_ALLOW_{suffix}"));
            match (key, cert, allow) {
                (Some(key), Some(cert), Some(allow)) => {
                    let allow = list(Some(allow));
                    if allow.is_empty() {
                        problems.push(format!(
                            "RUSTICDOC_SIGNING_ALLOW_{suffix} names no caller"
                        ));
                    }
                    s.named.push(NamedKey {
                        name,
                        key,
                        cert: PathBuf::from(cert),
                        allow,
                    });
                }
                // A named key nobody may use is a mistake, and one everybody
                // may use has to say so with `*`.
                _ => problems.push(format!(
                    "signing key '{name}' needs RUSTICDOC_SIGNING_KEY_{suffix}, RUSTICDOC_SIGNING_CERT_{suffix} and RUSTICDOC_SIGNING_ALLOW_{suffix}"
                )),
            }
        }

        s.pkcs11 = Pkcs11Settings {
            module: get("RUSTICDOC_PKCS11_MODULE").map(PathBuf::from),
            token: get("RUSTICDOC_PKCS11_TOKEN"),
            slot: None,
            pin: vars("RUSTICDOC_PKCS11_PIN").filter(|v| !v.is_empty()),
            pin_file: get("RUSTICDOC_PKCS11_PIN_FILE").map(PathBuf::from),
        };
        if let Some(v) = get("RUSTICDOC_PKCS11_SLOT") {
            match v.trim().parse() {
                Ok(slot) => s.pkcs11.slot = Some(slot),
                Err(_) => problems.push(format!("RUSTICDOC_PKCS11_SLOT: '{v}' is not a slot id")),
            }
        }

        if let Some(url) = get("RUSTICDOC_SIGNING_TSA_URL") {
            let mut tsa = Tsa::new(url.trim(), Duration::from_secs(10));
            match url::Url::parse(url.trim()) {
                Ok(u) if matches!(u.scheme(), "http" | "https") => {}
                _ => problems.push(format!(
                    "RUSTICDOC_SIGNING_TSA_URL must be an http or https URL, not '{url}'"
                )),
            }
            match get("RUSTICDOC_SIGNING_TSA_CERT") {
                Some(path) => tsa.cert_file = Some(PathBuf::from(path)),
                // A token whose signer nothing vouches for proves nothing.
                None => problems.push(
                    "RUSTICDOC_SIGNING_TSA_URL needs RUSTICDOC_SIGNING_TSA_CERT: the certificate the timestamp authority signs with, or the CA it chains to"
                        .into(),
                ),
            }
            tsa.username = get("RUSTICDOC_SIGNING_TSA_USERNAME");
            tsa.password = vars("RUSTICDOC_SIGNING_TSA_PASSWORD").filter(|v| !v.is_empty());
            if tsa.password.is_some() && tsa.username.is_none() {
                problems.push(
                    "RUSTICDOC_SIGNING_TSA_PASSWORD needs RUSTICDOC_SIGNING_TSA_USERNAME".into(),
                );
            }
            if let Some(v) = get("RUSTICDOC_SIGNING_TSA_HEADERS") {
                match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&v) {
                    Ok(map) => {
                        for (name, value) in map {
                            match value {
                                serde_json::Value::String(value) => tsa.headers.push((name, value)),
                                _ => problems.push(format!(
                                    "RUSTICDOC_SIGNING_TSA_HEADERS: the value of '{name}' must be a string"
                                )),
                            }
                        }
                    }
                    Err(_) => problems.push(
                        "RUSTICDOC_SIGNING_TSA_HEADERS must be a JSON object of header names to values"
                            .into(),
                    ),
                }
            }
            if let Some(v) = get("RUSTICDOC_SIGNING_TSA_TIMEOUT") {
                match parse_duration(&v) {
                    Some(d) if !d.is_zero() => tsa.timeout = d,
                    _ => {
                        problems.push(format!("RUSTICDOC_SIGNING_TSA_TIMEOUT: bad duration '{v}'"))
                    }
                }
            }
            s.tsa = Some(tsa);
        } else {
            for name in [
                "RUSTICDOC_SIGNING_TSA_CERT",
                "RUSTICDOC_SIGNING_TSA_USERNAME",
                "RUSTICDOC_SIGNING_TSA_PASSWORD",
                "RUSTICDOC_SIGNING_TSA_HEADERS",
                "RUSTICDOC_SIGNING_TSA_TIMEOUT",
            ] {
                if get(name).is_some() {
                    problems.push(format!("{name} is set without RUSTICDOC_SIGNING_TSA_URL"));
                }
            }
        }
        if let Some(v) = get("RUSTICDOC_SIGNING_LEVEL") {
            match Level::parse(&v) {
                Ok(level) => s.level = level,
                Err(_) => problems.push(format!(
                    "RUSTICDOC_SIGNING_LEVEL is '{v}'; use b-b, b-t, b-lt or b-lta"
                )),
            }
        }
        if s.level >= Level::BT && s.tsa.is_none() {
            problems.push(format!(
                "RUSTICDOC_SIGNING_LEVEL is {}, which needs RUSTICDOC_SIGNING_TSA_URL",
                s.level.name()
            ));
        }
        if let Some(v) = get("RUSTICDOC_SIGNING_REVOCATION_TIMEOUT") {
            match parse_duration(&v) {
                Some(d) if !d.is_zero() => s.revocation.timeout = d,
                _ => problems.push(format!(
                    "RUSTICDOC_SIGNING_REVOCATION_TIMEOUT: bad duration '{v}'"
                )),
            }
        }
        if let Some(v) = get("RUSTICDOC_SIGNING_REVOCATION_MAX_BYTES") {
            match parse_bytes(&v) {
                Some(n) if n > 0 => s.revocation.max_bytes = n,
                _ => problems.push(format!(
                    "RUSTICDOC_SIGNING_REVOCATION_MAX_BYTES: bad size '{v}'"
                )),
            }
        }
        s.outbound_allow_list = list(get("RUSTICDOC_SIGNING_OUTBOUND_ALLOW_LIST"));
        s.outbound_deny_list = list(get("RUSTICDOC_SIGNING_OUTBOUND_DENY_LIST"));
        for (name, patterns) in [
            (
                "RUSTICDOC_SIGNING_OUTBOUND_ALLOW_LIST",
                &s.outbound_allow_list,
            ),
            (
                "RUSTICDOC_SIGNING_OUTBOUND_DENY_LIST",
                &s.outbound_deny_list,
            ),
        ] {
            for pattern in patterns {
                if let Err(e) = regex::Regex::new(pattern) {
                    problems.push(format!("{name} holds an invalid pattern '{pattern}': {e}"));
                }
            }
        }
        s.outbound_allow_private = get("RUSTICDOC_SIGNING_OUTBOUND_ALLOW_PRIVATE")
            .is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes"));
        s
    }
}

/// Loads a key from a PEM file or a PKCS#11 token, with its certificate file.
pub fn load_key(key: &str, cert: &Path, pkcs11: &Pkcs11Settings) -> Result<Signer, String> {
    match key.strip_prefix("pkcs11:") {
        Some(spec) => Signer::from_pkcs11(pkcs11, &TokenKeySpec::parse(spec)?, cert),
        None => Signer::from_files(Path::new(key), cert),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(vars: &[(&str, &str)]) -> (Settings, Vec<String>) {
        let mut problems = Vec::new();
        let lookup = |name: &str| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        };
        let s = Settings::from_vars(&lookup, &mut problems);
        (s, problems)
    }

    #[test]
    fn named_keys_need_a_key_a_certificate_and_an_allow_list() {
        // Built, so the configuration check does not take them for settings.
        let var = |kind: &str, name: &str| ["RUSTICDOC_SIGNING_", kind, "_", name].concat();
        let vars = [
            ("RUSTICDOC_SIGNING_KEYS".to_string(), "acme, globex-eu"),
            (var("KEY", "ACME"), "/k/acme.pem"),
            (var("CERT", "ACME"), "/k/acme.crt"),
            (var("ALLOW", "ACME"), "tenant:acme, key:abc"),
            (var("KEY", "GLOBEX_EU"), "pkcs11:globex"),
            (var("CERT", "GLOBEX_EU"), "/k/globex.crt"),
        ];
        let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        let (s, problems) = read(&vars);
        assert_eq!(s.named.len(), 1);
        assert_eq!(s.named[0].name, "acme");
        assert_eq!(s.named[0].allow, ["tenant:acme", "key:abc"]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains(&var("ALLOW", "GLOBEX_EU")));

        let (_, problems) = read(&[("RUSTICDOC_SIGNING_KEYS", "bad name")]);
        assert!(problems[0].contains("letters, digits"), "{problems:?}");
    }

    #[test]
    fn timestamp_authority_and_level() {
        let (s, problems) = read(&[
            ("RUSTICDOC_SIGNING_TSA_URL", "https://tsa.example/stamp"),
            ("RUSTICDOC_SIGNING_TSA_CERT", "/k/tsa-root.pem"),
            ("RUSTICDOC_SIGNING_TSA_USERNAME", "u"),
            ("RUSTICDOC_SIGNING_TSA_PASSWORD", "p"),
            ("RUSTICDOC_SIGNING_TSA_HEADERS", r#"{"X-Api-Key": "k"}"#),
            ("RUSTICDOC_SIGNING_TSA_TIMEOUT", "5s"),
            ("RUSTICDOC_SIGNING_LEVEL", "B-LTA"),
        ]);
        assert!(problems.is_empty(), "{problems:?}");
        let tsa = s.tsa.unwrap();
        assert_eq!(tsa.url, "https://tsa.example/stamp");
        assert_eq!(tsa.headers, [("X-Api-Key".to_string(), "k".to_string())]);
        assert_eq!(tsa.timeout, Duration::from_secs(5));
        assert_eq!(s.level, Level::BLta);
        // Debug output never shows the password or headers.
        assert!(!format!("{tsa:?}").contains("\"p\""));

        let (_, problems) = read(&[("RUSTICDOC_SIGNING_LEVEL", "b-t")]);
        assert!(
            problems[0].contains("RUSTICDOC_SIGNING_TSA_URL"),
            "{problems:?}"
        );
        let (_, problems) = read(&[("RUSTICDOC_SIGNING_TSA_TIMEOUT", "5s")]);
        assert!(problems[0].contains("without"), "{problems:?}");
        let (_, problems) = read(&[
            ("RUSTICDOC_SIGNING_TSA_URL", "ftp://x"),
            ("RUSTICDOC_SIGNING_TSA_CERT", "/k/tsa-root.pem"),
        ]);
        assert!(problems[0].contains("http"), "{problems:?}");
        // A timestamp authority without an anchor to check its tokens against.
        let (_, problems) = read(&[("RUSTICDOC_SIGNING_TSA_URL", "https://tsa.example/")]);
        assert!(
            problems[0].contains("RUSTICDOC_SIGNING_TSA_CERT"),
            "{problems:?}"
        );
    }
}
