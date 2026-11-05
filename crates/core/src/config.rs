use std::path::PathBuf;
use std::time::Duration;

/// Service and engine settings, read from `RUSTICDOC_*` environment
/// variables. Every field has a default, so the binary starts with none set.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address the HTTP server listens on.
    pub bind: String,
    /// Time limit for one request, including conversion.
    pub api_timeout: Duration,
    /// Largest request body accepted, in bytes.
    pub body_limit: usize,
    /// LibreOffice conversions that may run at once.
    pub workers: usize,
    /// Path to `soffice` or `soffice.bin`. `None` disables conversion.
    pub libreoffice_bin: Option<PathBuf>,
    /// Path to `qpdf`. `None` disables the qpdf engine.
    pub qpdf_bin: Option<PathBuf>,
    /// Path to `pdfcpu`. `None` disables the pdfcpu engine.
    pub pdfcpu_bin: Option<PathBuf>,
    /// Directory of extra fonts, mounted at runtime.
    pub fonts_dir: PathBuf,
    /// Accepted API keys for `Authorization: Bearer` or `X-Api-Key`. Empty
    /// means no key is required.
    pub api_keys: Vec<String>,
    /// Basic auth credentials, when set.
    pub basic_auth: Option<(String, String)>,
    /// Secret for the HMAC signature on webhook deliveries.
    pub webhook_secret: Option<String>,
    /// Allow webhooks to private or loopback addresses. Off by default.
    pub webhook_allow_private: bool,
    /// Attempts per webhook delivery.
    pub webhook_attempts: u32,
    /// Origins allowed cross-origin access, or `*`. Empty means none.
    pub cors_origins: Vec<String>,
    /// `json` or `pretty`.
    pub log_format: String,
    /// A `tracing` filter such as `info` or `rusticdoc=debug`.
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:3000".into(),
            api_timeout: Duration::from_secs(30),
            body_limit: 64 * 1024 * 1024,
            workers: 2,
            libreoffice_bin: first_existing(&[
                "/usr/lib/libreoffice/program/soffice.bin",
                "/usr/bin/soffice",
                "/Applications/LibreOffice.app/Contents/MacOS/soffice",
            ]),
            qpdf_bin: first_existing(&[
                "/usr/bin/qpdf",
                "/opt/homebrew/bin/qpdf",
                "/usr/local/bin/qpdf",
            ]),
            pdfcpu_bin: first_existing(&[
                "/usr/bin/pdfcpu",
                "/opt/homebrew/bin/pdfcpu",
                "/usr/local/bin/pdfcpu",
            ]),
            fonts_dir: PathBuf::from("/fonts"),
            api_keys: Vec::new(),
            basic_auth: None,
            webhook_secret: None,
            webhook_allow_private: false,
            webhook_attempts: 3,
            cors_origins: Vec::new(),
            log_format: "json".into(),
            log_level: "info".into(),
        }
    }
}

impl Config {
    /// Reads `RUSTICDOC_*` variables over the defaults.
    pub fn from_env() -> Self {
        let mut c = Self::default();
        let get = |k: &str| {
            std::env::var(format!("RUSTICDOC_{k}"))
                .ok()
                .filter(|v| !v.is_empty())
        };

        if let Some(v) = get("BIND") {
            c.bind = v;
        }
        if let Some(v) = get("API_TIMEOUT").and_then(|v| parse_duration(&v)) {
            c.api_timeout = v;
        }
        if let Some(v) = get("BODY_LIMIT").and_then(|v| parse_bytes(&v)) {
            c.body_limit = v;
        }
        if let Some(v) = get("WORKERS").and_then(|v| v.parse().ok()) {
            c.workers = v;
        }
        if let Some(v) = get("LIBREOFFICE_BIN") {
            c.libreoffice_bin = if v == "none" {
                None
            } else {
                Some(PathBuf::from(v))
            };
        }
        if let Some(v) = get("QPDF_BIN") {
            c.qpdf_bin = if v == "none" {
                None
            } else {
                Some(PathBuf::from(v))
            };
        }
        if let Some(v) = get("PDFCPU_BIN") {
            c.pdfcpu_bin = if v == "none" {
                None
            } else {
                Some(PathBuf::from(v))
            };
        }
        if let Some(v) = get("FONTS_DIR") {
            c.fonts_dir = PathBuf::from(v);
        }
        if let Some(v) = get("API_KEYS") {
            c.api_keys = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let (Some(u), Some(p)) = (get("BASIC_AUTH_USERNAME"), get("BASIC_AUTH_PASSWORD")) {
            c.basic_auth = Some((u, p));
        }
        c.webhook_secret = get("WEBHOOK_SECRET");
        if let Some(v) = get("WEBHOOK_ALLOW_PRIVATE") {
            c.webhook_allow_private = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Some(v) = get("WEBHOOK_ATTEMPTS").and_then(|v| v.parse().ok()) {
            c.webhook_attempts = v;
        }
        if let Some(v) = get("CORS_ORIGINS") {
            c.cors_origins = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(v) = get("LOG_FORMAT") {
            c.log_format = v;
        }
        if let Some(v) = get("LOG_LEVEL") {
            c.log_level = v;
        }
        c
    }
}

fn first_existing(candidates: &[&str]) -> Option<PathBuf> {
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}

/// Parses `30s`, `2m`, `500ms` or a bare number of seconds.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        return ms.trim().parse().ok().map(Duration::from_millis);
    }
    if let Some(m) = s.strip_suffix('m') {
        return m
            .trim()
            .parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60));
    }
    if let Some(h) = s.strip_suffix('h') {
        return h
            .trim()
            .parse::<u64>()
            .ok()
            .map(|h| Duration::from_secs(h * 3600));
    }
    let secs = s.strip_suffix('s').unwrap_or(s);
    secs.trim().parse::<f64>().ok().map(Duration::from_secs_f64)
}

/// Parses `64MB`, `512KB`, `1GB` or a bare number of bytes.
pub fn parse_bytes(s: &str) -> Option<usize> {
    let s = s.trim().to_ascii_uppercase();
    let (num, mult) = if let Some(n) = s.strip_suffix("GB") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("MB") {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("KB") {
        (n, 1024)
    } else {
        (s.as_str(), 1)
    };
    num.trim().parse::<usize>().ok().map(|n| n * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration("1.5"), Some(Duration::from_secs_f64(1.5)));
        assert_eq!(parse_duration("soon"), None);
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_bytes("64MB"), Some(64 * 1024 * 1024));
        assert_eq!(parse_bytes("512kb"), Some(512 * 1024));
        assert_eq!(parse_bytes("1000"), Some(1000));
        assert_eq!(parse_bytes("lots"), None);
    }
}
