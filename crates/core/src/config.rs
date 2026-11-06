use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// Service and engine settings, read from `RUSTICDOC_*` environment
/// variables. Every field has a default, so the binary starts with none set.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address the HTTP server listens on.
    pub bind: String,
    /// Path prefix every route sits under, such as `/docs/`. `/` by default.
    pub root_path: String,
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
    /// Most elements one batch render may hold.
    pub batch_limit: usize,
    /// Most bytes a text template, `batchName` or PDF form tag may write.
    pub max_render_bytes: usize,
    /// Most file parts one form may hold.
    pub max_form_parts: usize,
    /// Webhook jobs that may run at once across the service.
    pub max_async_jobs: usize,
    /// Webhook jobs that may run at once for one caller.
    pub max_async_jobs_per_caller: usize,
    /// Time limit for a batch render or a multi-file conversion.
    pub batch_timeout: Duration,
    /// Where template code runs. `serve` chooses `Isolated` unless told otherwise.
    pub render_mode: RenderMode,
    /// Whether `RUSTICDOC_RENDER_MODE` chose the mode.
    pub render_mode_set: bool,
    /// The binary with the worker in it. The running one, when unset.
    pub render_worker_bin: Option<PathBuf>,
    /// Address space of one render worker, in bytes.
    pub render_memory: u64,
    /// Render workers that may run at once.
    pub render_workers: usize,
    /// `/metrics` stays open when other routes need credentials.
    pub metrics_public: bool,
    /// Serve `/docs`, which loads a third-party script.
    pub enable_docs: bool,
    /// Refuse the `downloadFrom` form field entirely.
    pub download_from_disabled: bool,
    /// Most URLs one request may fetch through `downloadFrom`.
    pub download_from_max_entries: usize,
    /// Allow `downloadFrom` to fetch from private or loopback addresses.
    /// Separate from the webhook flag. Off by default.
    pub download_from_allow_private: bool,
    /// `json` or `pretty`.
    pub log_format: String,
    /// A `tracing` filter such as `info` or `rusticdoc=debug`.
    pub log_level: String,
    /// Gotenberg's settings, read from its variable names.
    pub compat: CompatConfig,
    /// What the environment got wrong, for [`Config::check`] to refuse.
    pub problems: Vec<String>,
}

/// Where a service evaluates client template code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// In this process. For a library, tests, or an operator's explicit choice.
    InProcess,
    /// In a `__render-worker` child with its own limits.
    Isolated,
}

/// One operation's engine list, from a `PDFENGINES_*_ENGINES` variable.
pub const ENGINE_VARIABLES: &[&str] = &[
    "PDFENGINES_MERGE_ENGINES",
    "PDFENGINES_SPLIT_ENGINES",
    "PDFENGINES_FLATTEN_ENGINES",
    "PDFENGINES_CONVERT_ENGINES",
    "PDFENGINES_OPTIMIZE_IMAGES_ENGINES",
    "PDFENGINES_READ_METADATA_ENGINES",
    "PDFENGINES_WRITE_METADATA_ENGINES",
    "PDFENGINES_ENCRYPT_ENGINES",
    "PDFENGINES_EMBED_ENGINES",
    "PDFENGINES_EMBED_METADATA_ENGINES",
    "PDFENGINES_READ_BOOKMARKS_ENGINES",
    "PDFENGINES_WRITE_BOOKMARKS_ENGINES",
    "PDFENGINES_WATERMARK_ENGINES",
    "PDFENGINES_STAMP_ENGINES",
    "PDFENGINES_ROTATE_ENGINES",
    "PDFENGINES_FACTUR_X_ENGINES",
];

/// The behaviour of Gotenberg's routes and of the service around them.
#[derive(Debug, Clone)]
pub struct CompatConfig {
    /// `LIBREOFFICE_DISABLE_ROUTES`: `/forms/libreoffice/*` answers 404.
    pub libreoffice_disable_routes: bool,
    /// `PDFENGINES_DISABLE_ROUTES`: `/forms/pdfengines/*` answers 404.
    pub pdfengines_disable_routes: bool,
    /// `API_ENABLE_DEBUG_ROUTE`: serve `/debug`.
    pub enable_debug_route: bool,
    /// `GOTENBERG_BUILD_DEBUG_DATA`: fill `/debug` in.
    pub build_debug_data: bool,
    /// The header that carries a request's correlation id.
    pub trace_header: String,
    /// Routes left out of the access log.
    pub health_telemetry_off: bool,
    pub root_telemetry_off: bool,
    pub debug_telemetry_off: bool,
    pub version_telemetry_off: bool,
    pub prometheus_telemetry_off: bool,
    /// `PROMETHEUS_DISABLE_COLLECT`: no Gotenberg metrics route at all.
    pub prometheus_disabled: bool,
    pub prometheus_namespace: String,
    /// `/prometheus/metrics` by default.
    pub prometheus_path: String,
    /// `WEBHOOK_ENABLE_SYNC_MODE`: answer only after the delivery.
    pub webhook_sync: bool,
    /// `WEBHOOK_DISABLE`: `/forms/*` ignores the webhook headers.
    pub webhook_disabled: bool,
    /// `API_ENABLE_OIDC_AUTH` and friends.
    pub oidc: Option<OidcConfig>,
    /// Engine lists by variable name, as set. Unset lists use the defaults.
    pub engine_orders: BTreeMap<String, Vec<String>>,
    /// `WEBHOOK_ALLOW_LIST`: regular expressions a webhook URL must match one of.
    /// Empty allows all, subject to the refusal of private addresses.
    pub webhook_allow_list: Vec<String>,
    /// `WEBHOOK_DENY_LIST`: regular expressions no webhook URL may match.
    pub webhook_deny_list: Vec<String>,
    /// `WEBHOOK_ERROR_ALLOW_LIST`: the allow list for error URLs, when set.
    pub webhook_error_allow_list: Vec<String>,
    /// `WEBHOOK_ERROR_DENY_LIST`: more refusals for error URLs.
    pub webhook_error_deny_list: Vec<String>,
    /// `API_DOWNLOAD_FROM_ALLOW_LIST`, as `webhook_allow_list` for `downloadFrom`.
    pub download_allow_list: Vec<String>,
    /// `API_DOWNLOAD_FROM_DENY_LIST`, as `webhook_deny_list` for `downloadFrom`.
    pub download_deny_list: Vec<String>,
}

impl Default for CompatConfig {
    fn default() -> Self {
        Self {
            libreoffice_disable_routes: false,
            pdfengines_disable_routes: false,
            enable_debug_route: false,
            build_debug_data: true,
            trace_header: "Gotenberg-Trace".into(),
            health_telemetry_off: true,
            root_telemetry_off: true,
            debug_telemetry_off: true,
            version_telemetry_off: true,
            prometheus_telemetry_off: true,
            prometheus_disabled: false,
            prometheus_namespace: "gotenberg".into(),
            prometheus_path: "/prometheus/metrics".into(),
            webhook_sync: false,
            webhook_disabled: false,
            oidc: None,
            engine_orders: BTreeMap::new(),
            webhook_allow_list: Vec::new(),
            webhook_deny_list: Vec::new(),
            webhook_error_allow_list: Vec::new(),
            webhook_error_deny_list: Vec::new(),
            download_allow_list: Vec::new(),
            download_deny_list: Vec::new(),
        }
    }
}

/// Bearer tokens accepted on the protected routes.
#[derive(Debug, Clone, Default)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    /// Where the signing keys live. Discovered from the issuer when empty.
    pub jwks_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:3000".into(),
            root_path: "/".into(),
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
            batch_limit: 100,
            max_render_bytes: crate::limits::DEFAULT_MAX_RENDER_BYTES,
            max_form_parts: crate::limits::DEFAULT_MAX_FORM_PARTS,
            max_async_jobs: 32,
            max_async_jobs_per_caller: 8,
            batch_timeout: Duration::from_secs(300),
            render_mode: RenderMode::InProcess,
            render_mode_set: false,
            render_worker_bin: None,
            render_memory: 1024 * 1024 * 1024,
            render_workers: 4,
            metrics_public: false,
            enable_docs: false,
            download_from_disabled: false,
            download_from_max_entries: 10,
            download_from_allow_private: false,
            log_format: "json".into(),
            log_level: "info".into(),
            compat: CompatConfig::default(),
            problems: Vec::new(),
        }
    }
}

impl Config {
    /// Reads `RUSTICDOC_*` variables over the defaults.
    pub fn from_env() -> Self {
        Self::from_vars(&|name| std::env::var(name).ok())
    }

    /// Reads the settings from `vars`, which looks up one variable.
    pub fn from_vars(vars: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut c = Self::default();
        let get = |k: &str| vars(&format!("RUSTICDOC_{k}")).filter(|v| !v.is_empty());

        c.apply_gotenberg_names(vars);
        if let Some(v) = get("BIND") {
            c.bind = v;
        }
        if let Some(v) = get("ROOT_PATH") {
            c.root_path = normalise_root(&v);
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
            // Left empty is left unset, as compose passes it; a value with no
            // key in it is a mistake that would leave the service open.
            if c.api_keys.is_empty() {
                c.problems
                    .push("RUSTICDOC_API_KEYS is set but holds no key".into());
            }
        }
        match (get("BASIC_AUTH_USERNAME"), get("BASIC_AUTH_PASSWORD")) {
            (Some(u), Some(p)) => c.basic_auth = Some((u, p)),
            (None, None) => {}
            _ => c.problems.push(
                "RUSTICDOC_BASIC_AUTH_USERNAME and RUSTICDOC_BASIC_AUTH_PASSWORD go together"
                    .into(),
            ),
        }
        c.webhook_secret = get("WEBHOOK_SECRET");
        if let Some(v) = get("WEBHOOK_ALLOW_PRIVATE") {
            c.webhook_allow_private = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Some(v) = get("WEBHOOK_ATTEMPTS").and_then(|v| v.parse().ok()) {
            c.webhook_attempts = v;
        }
        if let Some(v) = get("METRICS_PUBLIC") {
            c.metrics_public = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Some(v) = get("ENABLE_DOCS") {
            c.enable_docs = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Some(v) = get("DOWNLOAD_FROM_DISABLED") {
            c.download_from_disabled = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Some(v) = get("DOWNLOAD_FROM_MAX_ENTRIES").and_then(|v| v.parse().ok()) {
            c.download_from_max_entries = v;
        }
        if let Some(v) = get("DOWNLOAD_FROM_ALLOW_PRIVATE") {
            c.download_from_allow_private = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Some(v) = get("BATCH_LIMIT").and_then(|v| v.parse().ok()) {
            c.batch_limit = v;
        }
        if let Some(v) = get("MAX_RENDER_BYTES").and_then(|v| parse_bytes(&v)) {
            c.max_render_bytes = v;
        }
        if let Some(v) = get("RENDER_MODE") {
            c.render_mode_set = true;
            c.render_mode = match v.as_str() {
                "isolated" => RenderMode::Isolated,
                "in-process" => RenderMode::InProcess,
                other => {
                    c.problems.push(format!(
                        "RUSTICDOC_RENDER_MODE is '{other}', not 'isolated' or 'in-process'"
                    ));
                    RenderMode::Isolated
                }
            };
        }
        if let Some(v) = get("RENDER_WORKER_BIN") {
            c.render_worker_bin = Some(PathBuf::from(v));
        }
        if let Some(v) = get("RENDER_MEMORY").and_then(|v| parse_bytes(&v)) {
            c.render_memory = v as u64;
        }
        if let Some(v) = get("RENDER_WORKERS").and_then(|v| v.parse().ok()) {
            c.render_workers = v;
        }
        if let Some(v) = get("MAX_ASYNC_JOBS").and_then(|v| v.parse().ok()) {
            c.max_async_jobs = v;
        }
        if let Some(v) = get("MAX_ASYNC_JOBS_PER_CALLER").and_then(|v| v.parse().ok()) {
            c.max_async_jobs_per_caller = v;
        }
        if let Some(v) = get("BATCH_TIMEOUT").and_then(|v| parse_duration(&v)) {
            c.batch_timeout = v;
        }
        if let Some(v) = get("MAX_FORM_PARTS").and_then(|v| v.parse().ok()) {
            c.max_form_parts = v;
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

    /// Gotenberg's variable names are accepted too, so a deployment moves
    /// over by changing the image. `RUSTICDOC_*` wins when both are set.
    fn apply_gotenberg_names(&mut self, vars: &dyn Fn(&str) -> Option<String>) {
        let get = |k: &str| vars(k).filter(|v| !v.is_empty());
        let port = get("API_PORT");
        let ip = get("API_BIND_IP");
        if port.is_some() || ip.is_some() {
            self.bind = format!(
                "{}:{}",
                ip.unwrap_or_else(|| "0.0.0.0".into()),
                port.unwrap_or_else(|| "3000".into())
            );
        }
        if let Some(v) = get("API_ROOT_PATH") {
            self.root_path = normalise_root(&v);
        }
        if let Some(v) = get("API_TIMEOUT").and_then(|v| parse_duration(&v)) {
            self.api_timeout = v;
        }
        if let Some(v) = get("API_BODY_LIMIT").and_then(|v| parse_bytes(&v)) {
            self.body_limit = v;
        }
        if let Some(flag) = get("API_ENABLE_BASIC_AUTH") {
            match parse_go_bool(&flag) {
                Some(true) => match (
                    get("GOTENBERG_API_BASIC_AUTH_USERNAME"),
                    get("GOTENBERG_API_BASIC_AUTH_PASSWORD"),
                ) {
                    (Some(u), Some(p)) => self.basic_auth = Some((u, p)),
                    _ => self.problems.push(
                        "API_ENABLE_BASIC_AUTH needs GOTENBERG_API_BASIC_AUTH_USERNAME and GOTENBERG_API_BASIC_AUTH_PASSWORD".into(),
                    ),
                },
                Some(false) => {}
                None => self
                    .problems
                    .push(format!("API_ENABLE_BASIC_AUTH is '{flag}', not a boolean")),
            }
        }
        if let Some(v) = get("LIBREOFFICE_BIN_PATH") {
            self.libreoffice_bin = Some(PathBuf::from(v));
        }
        if let Some(v) = get("QPDF_BIN_PATH") {
            self.qpdf_bin = Some(PathBuf::from(v));
        }
        if let Some(v) = get("PDFCPU_BIN_PATH") {
            self.pdfcpu_bin = Some(PathBuf::from(v));
        }
        if let Some(v) = get("LOG_LEVEL") {
            self.log_level = v;
        }
        if let Some(v) = get("LOG_FORMAT") {
            self.log_format = match v.as_str() {
                "text" | "pretty" => "pretty".into(),
                _ => "json".into(),
            };
        }
        if matches!(get("WEBHOOK_ALLOW_PRIVATE").as_deref(), Some("true" | "1")) {
            self.webhook_allow_private = true;
        }
        if matches!(
            get("API_DISABLE_DOWNLOAD_FROM").as_deref(),
            Some("true" | "1")
        ) {
            self.download_from_disabled = true;
        }
        if let Some(v) = get("API_DOWNLOAD_FROM_MAX_ENTRIES").and_then(|v| v.parse().ok()) {
            self.download_from_max_entries = v;
        }
        self.apply_compat_names(vars);
    }

    fn apply_compat_names(&mut self, vars: &dyn Fn(&str) -> Option<String>) {
        let get = |k: &str| vars(k).filter(|v| !v.is_empty());
        let on = |k: &str| get(k).and_then(|v| parse_go_bool(&v));
        let compat = &mut self.compat;
        if let Some(v) = on("LIBREOFFICE_DISABLE_ROUTES") {
            compat.libreoffice_disable_routes = v;
        }
        if let Some(v) = on("PDFENGINES_DISABLE_ROUTES") {
            compat.pdfengines_disable_routes = v;
        }
        if let Some(v) = on("API_ENABLE_DEBUG_ROUTE") {
            compat.enable_debug_route = v;
        }
        if let Some(v) = on("GOTENBERG_BUILD_DEBUG_DATA") {
            compat.build_debug_data = v;
        }
        if let Some(v) = get("API_TRACE_HEADER").or_else(|| get("API_CORRELATION_ID_HEADER")) {
            compat.trace_header = v;
        }
        // The older `*_LOGGING` names still work; the newer ones win.
        for (slot, names) in [
            (
                &mut compat.health_telemetry_off,
                [
                    "API_DISABLE_HEALTH_CHECK_LOGGING",
                    "API_DISABLE_HEALTH_CHECK_ROUTE_TELEMETRY",
                ],
            ),
            (
                &mut compat.root_telemetry_off,
                [
                    "API_DISABLE_ROOT_ROUTE_LOGGING",
                    "API_DISABLE_ROOT_ROUTE_TELEMETRY",
                ],
            ),
            (
                &mut compat.debug_telemetry_off,
                [
                    "API_DISABLE_DEBUG_ROUTE_LOGGING",
                    "API_DISABLE_DEBUG_ROUTE_TELEMETRY",
                ],
            ),
            (
                &mut compat.version_telemetry_off,
                [
                    "API_DISABLE_VERSION_ROUTE_LOGGING",
                    "API_DISABLE_VERSION_ROUTE_TELEMETRY",
                ],
            ),
            (
                &mut compat.prometheus_telemetry_off,
                [
                    "PROMETHEUS_DISABLE_ROUTE_LOGGING",
                    "PROMETHEUS_DISABLE_ROUTE_TELEMETRY",
                ],
            ),
        ] {
            for name in names {
                if let Some(v) = on(name) {
                    *slot = v;
                }
            }
        }
        if let Some(v) = on("PROMETHEUS_DISABLE_COLLECT") {
            compat.prometheus_disabled = v;
        }
        if let Some(v) = get("PROMETHEUS_NAMESPACE") {
            compat.prometheus_namespace = v;
        }
        if let Some(v) = get("PROMETHEUS_METRICS_PATH") {
            compat.prometheus_path = format!("/{}", v.trim_matches('/'));
        }
        if let Some(v) = on("WEBHOOK_ENABLE_SYNC_MODE") {
            compat.webhook_sync = v;
        }
        if let Some(v) = on("WEBHOOK_DISABLE") {
            compat.webhook_disabled = v;
        }
        if let Some(flag) = get("API_ENABLE_OIDC_AUTH") {
            match parse_go_bool(&flag) {
                Some(true) => {
                    compat.oidc = Some(OidcConfig {
                        issuer: get("API_OIDC_ISSUER").unwrap_or_default(),
                        audience: get("API_OIDC_AUDIENCE").unwrap_or_default(),
                        jwks_url: get("API_OIDC_JWKS_URL").unwrap_or_default(),
                    });
                }
                Some(false) => {}
                None => self
                    .problems
                    .push(format!("API_ENABLE_OIDC_AUTH is '{flag}', not a boolean")),
            }
        }
        let list = |name: &str| -> Vec<String> {
            get(name)
                .map(|v| {
                    v.split(',')
                        .map(|e| e.trim().to_string())
                        .filter(|e| !e.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        compat.webhook_allow_list = list("WEBHOOK_ALLOW_LIST");
        compat.webhook_deny_list = list("WEBHOOK_DENY_LIST");
        compat.webhook_error_allow_list = list("WEBHOOK_ERROR_ALLOW_LIST");
        compat.webhook_error_deny_list = list("WEBHOOK_ERROR_DENY_LIST");
        compat.download_allow_list = list("API_DOWNLOAD_FROM_ALLOW_LIST");
        compat.download_deny_list = list("API_DOWNLOAD_FROM_DENY_LIST");
        let compat = &mut self.compat;
        for name in ENGINE_VARIABLES.iter().chain(["PDFENGINES_ENGINES"].iter()) {
            if let Some(list) = get(name) {
                compat.engine_orders.insert(
                    (*name).to_string(),
                    list.split(',')
                        .map(|e| e.trim().to_string())
                        .filter(|e| !e.is_empty())
                        .collect(),
                );
            }
        }
    }

    /// True when requests must carry credentials.
    pub fn auth_configured(&self) -> bool {
        !self.api_keys.is_empty() || self.basic_auth.is_some() || self.compat.oidc.is_some()
    }

    /// True when `/metrics` needs credentials.
    pub fn metrics_guarded(&self) -> bool {
        self.auth_configured() && !self.metrics_public
    }

    /// Settings the service refuses to start with. PDFtk and ExifTool are
    /// not part of this build, so a list that names them cannot be honoured.
    pub fn check(&self) -> Result<(), String> {
        if let Some(problem) = self.problems.first() {
            return Err(problem.clone());
        }
        for (variable, engines) in &self.compat.engine_orders {
            for engine in engines {
                match engine.as_str() {
                    "qpdf" | "pdfcpu" | "lopdf" | "qpdf-metadata" | "libreoffice-pdfengine" => {}
                    "pdftk" | "exiftool" => {
                        return Err(format!(
                            "{variable} names '{engine}', which this build does not include; use qpdf, pdfcpu, lopdf or qpdf-metadata"
                        ));
                    }
                    other => {
                        return Err(format!("{variable} names an unknown engine '{other}'"));
                    }
                }
            }
        }
        let trace_header = &self.compat.trace_header;
        if trace_header.is_empty()
            || !trace_header
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(format!("'{trace_header}' cannot be a header name"));
        }
        if let Some(oidc) = &self.compat.oidc {
            if oidc.issuer.is_empty() || oidc.audience.is_empty() {
                return Err(
                    "API_ENABLE_OIDC_AUTH needs API_OIDC_ISSUER and API_OIDC_AUDIENCE".into(),
                );
            }
            for (name, url) in [
                ("API_OIDC_ISSUER", oidc.issuer.as_str()),
                ("API_OIDC_JWKS_URL", oidc.jwks_url.as_str()),
            ] {
                if !url.is_empty() && !is_secure_url(url) {
                    return Err(format!(
                        "{name} must be https, or http on a loopback host, not '{url}'"
                    ));
                }
            }
        }
        let compat = &self.compat;
        for (name, list) in [
            ("WEBHOOK_ALLOW_LIST", &compat.webhook_allow_list),
            ("WEBHOOK_DENY_LIST", &compat.webhook_deny_list),
            ("WEBHOOK_ERROR_ALLOW_LIST", &compat.webhook_error_allow_list),
            ("WEBHOOK_ERROR_DENY_LIST", &compat.webhook_error_deny_list),
            ("API_DOWNLOAD_FROM_ALLOW_LIST", &compat.download_allow_list),
            ("API_DOWNLOAD_FROM_DENY_LIST", &compat.download_deny_list),
        ] {
            for pattern in list {
                if let Err(e) = regex::Regex::new(pattern) {
                    return Err(format!("{name} holds an invalid pattern '{pattern}': {e}"));
                }
            }
        }
        Ok(())
    }
}

/// True for an `https` URL, or an `http` one whose host is this machine.
pub fn is_secure_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => match parsed.host() {
            Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// Go's `strconv.ParseBool`, which Gotenberg's flags go through.
pub fn parse_go_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

/// `/foo` and `/foo/` both become `/foo/`; empty becomes `/`.
pub fn normalise_root(s: &str) -> String {
    let trimmed = s.trim().trim_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        format!("/{trimmed}/")
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

    fn from(vars: &[(&str, &str)]) -> Config {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_vars(&move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    #[test]
    fn half_configured_authentication_stops_the_service() {
        for vars in [
            &[("API_ENABLE_BASIC_AUTH", "true")][..],
            &[("API_ENABLE_BASIC_AUTH", "yes")],
            &[
                ("API_ENABLE_BASIC_AUTH", "true"),
                ("GOTENBERG_API_BASIC_AUTH_USERNAME", "u"),
            ],
            &[("RUSTICDOC_BASIC_AUTH_USERNAME", "u")],
            &[("RUSTICDOC_BASIC_AUTH_PASSWORD", "p")],
            &[("RUSTICDOC_API_KEYS", " , ")],
            &[("API_ENABLE_OIDC_AUTH", "yes")],
            &[("API_ENABLE_OIDC_AUTH", "true")],
            &[
                ("API_ENABLE_OIDC_AUTH", "true"),
                ("API_OIDC_ISSUER", "http://idp.example.com"),
                ("API_OIDC_AUDIENCE", "a"),
            ],
            &[
                ("API_ENABLE_OIDC_AUTH", "true"),
                ("API_OIDC_ISSUER", "https://idp.example.com"),
                ("API_OIDC_AUDIENCE", "a"),
                ("API_OIDC_JWKS_URL", "http://idp.example.com/keys"),
            ],
        ] {
            assert!(from(vars).check().is_err(), "{vars:?}");
        }
    }

    #[test]
    fn authentication_is_read_as_gotenberg_reads_it() {
        let basic = [
            ("API_ENABLE_BASIC_AUTH", "True"),
            ("GOTENBERG_API_BASIC_AUTH_USERNAME", "u"),
            ("GOTENBERG_API_BASIC_AUTH_PASSWORD", "p"),
        ];
        let config = from(&basic);
        assert!(config.check().is_ok());
        assert!(config.basic_auth.is_some());
        let off = [
            ("API_ENABLE_BASIC_AUTH", "false"),
            ("GOTENBERG_API_BASIC_AUTH_USERNAME", "u"),
        ];
        assert!(from(&off).check().is_ok());
        assert!(from(&off).basic_auth.is_none());
        // Left empty is left unset, as compose passes it.
        assert!(from(&[("RUSTICDOC_API_KEYS", "")]).check().is_ok());
        assert_eq!(from(&[("RUSTICDOC_API_KEYS", "a, b")]).api_keys.len(), 2);
        let local = [
            ("API_ENABLE_OIDC_AUTH", "1"),
            ("API_OIDC_ISSUER", "http://127.0.0.1:8080"),
            ("API_OIDC_AUDIENCE", "a"),
        ];
        assert!(from(&local).check().is_ok());
        assert!(from(&[]).check().is_ok());
    }

    #[test]
    fn invalid_url_patterns_stop_the_service() {
        for name in [
            "WEBHOOK_ALLOW_LIST",
            "WEBHOOK_DENY_LIST",
            "API_DOWNLOAD_FROM_ALLOW_LIST",
            "API_DOWNLOAD_FROM_DENY_LIST",
        ] {
            assert!(from(&[(name, "(")]).check().is_err(), "{name}");
            assert!(from(&[(name, "^https://a,^https://b")]).check().is_ok());
        }
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration("1.5"), Some(Duration::from_secs_f64(1.5)));
        assert_eq!(parse_duration("soon"), None);
    }

    #[test]
    fn root_paths() {
        assert_eq!(normalise_root(""), "/");
        assert_eq!(normalise_root("/"), "/");
        assert_eq!(normalise_root("foo"), "/foo/");
        assert_eq!(normalise_root("/foo/bar/"), "/foo/bar/");
    }

    #[test]
    fn removed_engines_stop_the_service() {
        let mut config = Config::default();
        assert!(config.check().is_ok());
        config
            .compat
            .engine_orders
            .insert("PDFENGINES_MERGE_ENGINES".into(), vec!["qpdf".into()]);
        assert!(config.check().is_ok());
        for bad in ["pdftk", "exiftool", "nonsense"] {
            config
                .compat
                .engine_orders
                .insert("PDFENGINES_STAMP_ENGINES".into(), vec![bad.into()]);
            assert!(config.check().unwrap_err().contains(bad));
        }
    }

    #[test]
    fn trace_headers_must_be_header_names() {
        let mut config = Config::default();
        config.compat.trace_header = "bad header".into();
        assert!(config.check().is_err());
    }

    #[test]
    fn go_booleans() {
        assert_eq!(parse_go_bool("True"), Some(true));
        assert_eq!(parse_go_bool("0"), Some(false));
        assert_eq!(parse_go_bool("yes"), None);
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_bytes("64MB"), Some(64 * 1024 * 1024));
        assert_eq!(parse_bytes("512kb"), Some(512 * 1024));
        assert_eq!(parse_bytes("1000"), Some(1000));
        assert_eq!(parse_bytes("lots"), None);
    }
}
