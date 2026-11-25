use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// Service and engine settings, read from `RUSTICDOC_*` environment
/// variables. Every field has a default, so the binary starts with none set.
#[derive(Clone)]
pub struct Config {
    /// Address the HTTP server listens on.
    pub bind: String,
    /// Path prefix every route sits under, such as `/docs/`. `/` by default.
    pub root_path: String,
    /// Time limit for one request, including conversion.
    pub api_timeout: Duration,
    /// Largest request body accepted, in bytes.
    pub body_limit: usize,
    /// Request bodies held in memory at once, in bytes. Past it, requests wait.
    pub max_inflight_bytes: usize,
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
    /// Directory of images and stylesheets that templates and pages name,
    /// mounted at runtime. It need not exist.
    pub assets_dir: PathBuf,
    /// `<lang>.json` translation dictionaries for `t`, when set.
    pub translations_dir: Option<PathBuf>,
    /// Where uploaded templates and fonts live: `file:///path` or
    /// `s3://bucket/prefix`. Without it those routes answer 501.
    pub store: Option<String>,
    /// Largest template that can be stored.
    pub max_template_bytes: usize,
    /// Most templates one scope may hold.
    pub max_templates: usize,
    /// Most bytes of templates one scope may hold.
    pub max_template_storage: u64,
    /// Most bytes of background-job results one scope may hold at a time.
    pub max_job_storage: u64,
    /// Most fonts the store may hold.
    pub max_stored_fonts: usize,
    /// Most bytes of fonts the store may hold. Every replica copies them to disk.
    pub max_font_storage: u64,
    /// This replica's copy of the stored fonts, on its own disk.
    pub font_cache_dir: PathBuf,
    /// Most assets the store may hold.
    pub max_stored_assets: usize,
    /// Most bytes of assets the store may hold. Every replica copies them to disk.
    pub max_asset_storage: u64,
    /// Most disk the asset libraries sent with requests may take.
    pub max_asset_library_space: u64,
    /// This replica's copy of the stored assets, on its own disk.
    pub asset_cache_dir: PathBuf,
    /// How often a replica looks for fonts and assets added, changed or deleted
    /// elsewhere.
    pub store_sync_interval: Duration,
    /// The API key, for `Authorization: Bearer` or `X-Api-Key`. `None` means
    /// no key is required.
    pub api_key: Option<String>,
    /// How long a background job and its result are kept once it ends.
    pub job_ttl: Duration,
    /// Path to poppler's `pdftoppm`, which draws template previews. `None`
    /// makes previews answer 503.
    pub pdftoppm_bin: Option<PathBuf>,
    /// Secret for the HMAC signature on webhook deliveries.
    pub webhook_secret: Option<String>,
    /// PEM private key (PKCS#8, PKCS#1 or SEC1) for cryptographic PDF
    /// signatures. Set with [`Config::signing_cert`] or not at all.
    pub signing_key: Option<PathBuf>,
    /// PEM certificate for the signing key, then any chain certificates.
    pub signing_cert: Option<PathBuf>,
    /// The rest of signing: named keys, PKCS#11, the timestamp authority, the
    /// default level and the outbound rules for timestamps and revocation data.
    pub signing: crate::pdfops::sign::Settings,
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
    /// Most temporary files one LibreOffice conversion may write.
    pub libreoffice_max_disk: u64,
    /// Most file parts one form may hold.
    pub max_form_parts: usize,
    /// Webhook and background jobs that may run at once.
    pub max_async_jobs: usize,
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
    /// Memory cap for one external engine process (qpdf, pdfcpu, LibreOffice),
    /// in bytes. Native engines get a Linux `RLIMIT_AS`; pdfcpu, being Go, gets
    /// a `GOMEMLIMIT` soft cap instead. 0 leaves them uncapped, bounded only by
    /// the container's own memory limit. Off by default, because the cap can
    /// make a genuinely large conversion fail.
    pub engine_memory: u64,
    /// Render workers that may run at once.
    pub render_workers: usize,
    /// How long a client may take to send its request headers.
    pub header_timeout: Duration,
    /// Connections served at once. More wait in the listen queue.
    pub max_connections: usize,
    /// Connections served at once from one address. 0 is no limit.
    pub max_connections_per_peer: usize,
    /// How long a connection may stay blocked on its client.
    pub stall_timeout: Duration,
    /// How long writes may spend blocked in all, until the queue drains.
    pub response_write_timeout: Duration,
    /// A proxy for webhooks and downloads, named on purpose. Pinning to the vetted
    /// addresses no longer applies through it.
    pub outbound_proxy: Option<String>,
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
    /// Conversion routes answer 404, and conversion steps 503.
    pub convert_disabled: bool,
    /// The `/v1/pdf/*` routes answer 404.
    pub pdf_disabled: bool,
    /// Webhook headers are ignored on every route.
    pub webhook_disabled: bool,
    /// Engine lists by variable name, as set. Unset lists use the default order.
    pub engine_orders: BTreeMap<String, Vec<String>>,
    /// Regular expressions a webhook URL must match one of. Empty allows all,
    /// subject to the refusal of private addresses.
    pub webhook_allow_list: Vec<String>,
    /// Regular expressions no webhook URL may match.
    pub webhook_deny_list: Vec<String>,
    /// The allow list for error URLs, when set. The webhook list otherwise.
    pub webhook_error_allow_list: Vec<String>,
    /// More refusals for error URLs.
    pub webhook_error_deny_list: Vec<String>,
    /// As `webhook_allow_list`, for `downloadFrom`.
    pub download_allow_list: Vec<String>,
    /// As `webhook_deny_list`, for `downloadFrom`.
    pub download_deny_list: Vec<String>,
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

/// One operation's engine list, from a `RUSTICDOC_PDF_ENGINES_*` variable.
/// Settings that are gone, without their prefix, and what holds instead.
const REMOVED_VARIABLES: &[(&str, &str)] = &[
    ("API_KEYS", ONE_KEY),
    ("ADMIN_KEYS", ONE_KEY),
    ("TENANT_KEYS", ONE_KEY),
    ("BASIC_AUTH_USERNAME", ONE_KEY),
    ("BASIC_AUTH_PASSWORD", ONE_KEY),
    ("BASIC_AUTH_ADMIN", ONE_KEY),
    ("OIDC_ISSUER", ONE_KEY),
    ("OIDC_AUDIENCE", ONE_KEY),
    ("OIDC_JWKS_URL", ONE_KEY),
    ("OIDC_ADMIN_SUBJECTS", ONE_KEY),
    ("OIDC_ADMIN_CLAIM", ONE_KEY),
    (
        "STORE_SCOPE",
        "templates and aliases are one set for the whole store",
    ),
    (
        "MAX_ASYNC_JOBS_PER_CALLER",
        "RUSTICDOC_MAX_ASYNC_JOBS is the one limit",
    ),
];
const ONE_KEY: &str = "callers authenticate with the key in RUSTICDOC_API_KEY";

pub const ENGINE_VARIABLES: &[&str] = &[
    "RUSTICDOC_PDF_ENGINES_MERGE",
    "RUSTICDOC_PDF_ENGINES_SPLIT",
    "RUSTICDOC_PDF_ENGINES_FLATTEN",
    "RUSTICDOC_PDF_ENGINES_ROTATE",
    "RUSTICDOC_PDF_ENGINES_OPTIMIZE_IMAGES",
    "RUSTICDOC_PDF_ENGINES_ENCRYPT",
    "RUSTICDOC_PDF_ENGINES_WATERMARK",
    "RUSTICDOC_PDF_ENGINES_STAMP",
    "RUSTICDOC_PDF_ENGINES_EMBED",
    "RUSTICDOC_PDF_ENGINES_EMBED_METADATA",
    "RUSTICDOC_PDF_ENGINES_READ_BOOKMARKS",
    "RUSTICDOC_PDF_ENGINES_WRITE_BOOKMARKS",
    "RUSTICDOC_PDF_ENGINES_READ_METADATA",
    "RUSTICDOC_PDF_ENGINES_WRITE_METADATA",
    "RUSTICDOC_PDF_ENGINES_FACTUR_X",
];

/// The port the service listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 9027;

/// Not derived: the settings hold every key and the webhook secret, and one `{:?}` in a log line would print them. This says
/// what is configured and nothing of what it is.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("root_path", &self.root_path)
            .field("api_key", &self.api_key.is_some())
            .field("webhook_secret", &self.webhook_secret.is_some())
            .field("store", &self.store.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: format!("0.0.0.0:{DEFAULT_PORT}"),
            root_path: "/".into(),
            api_timeout: Duration::from_secs(30),
            body_limit: 64 * 1024 * 1024,
            max_inflight_bytes: 2 * 1024 * 1024 * 1024,
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
            assets_dir: PathBuf::from("/assets"),
            translations_dir: None,
            store: None,
            max_template_bytes: 50 * 1024 * 1024,
            max_templates: 10_000,
            max_template_storage: 5 * 1024 * 1024 * 1024,
            max_job_storage: 5 * 1024 * 1024 * 1024,
            max_stored_fonts: 2_000,
            max_font_storage: 1024 * 1024 * 1024,
            font_cache_dir: std::env::temp_dir().join("rusticdoc-stored-fonts"),
            max_stored_assets: 2_000,
            max_asset_storage: 1024 * 1024 * 1024,
            max_asset_library_space: 512 * 1024 * 1024,
            asset_cache_dir: std::env::temp_dir().join("rusticdoc-stored-assets"),
            store_sync_interval: Duration::from_secs(30),
            api_key: None,
            job_ttl: Duration::from_secs(3600),
            pdftoppm_bin: first_existing(&[
                "/usr/bin/pdftoppm",
                "/opt/homebrew/bin/pdftoppm",
                "/usr/local/bin/pdftoppm",
            ]),
            webhook_secret: None,
            signing_key: None,
            signing_cert: None,
            signing: Default::default(),
            webhook_allow_private: false,
            webhook_attempts: 3,
            cors_origins: Vec::new(),
            batch_limit: 100,
            max_render_bytes: crate::limits::DEFAULT_MAX_RENDER_BYTES,
            libreoffice_max_disk: crate::limits::DEFAULT_LIBREOFFICE_MAX_DISK,
            max_form_parts: crate::limits::DEFAULT_MAX_FORM_PARTS,
            max_async_jobs: 8,
            batch_timeout: Duration::from_secs(300),
            render_mode: RenderMode::InProcess,
            render_mode_set: false,
            render_worker_bin: None,
            render_memory: 1024 * 1024 * 1024,
            engine_memory: 0,
            render_workers: 4,
            header_timeout: Duration::from_secs(10),
            max_connections: 1024,
            max_connections_per_peer: 0,
            stall_timeout: Duration::from_secs(30),
            response_write_timeout: Duration::from_secs(300),
            outbound_proxy: None,
            metrics_public: false,
            enable_docs: false,
            download_from_disabled: false,
            download_from_max_entries: 10,
            download_from_allow_private: false,
            log_format: "json".into(),
            log_level: "info".into(),
            convert_disabled: false,
            pdf_disabled: false,
            webhook_disabled: false,
            engine_orders: BTreeMap::new(),
            webhook_allow_list: Vec::new(),
            webhook_deny_list: Vec::new(),
            webhook_error_allow_list: Vec::new(),
            webhook_error_deny_list: Vec::new(),
            download_allow_list: Vec::new(),
            download_deny_list: Vec::new(),
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
        let on = |k: &str| get(k).map(|v| matches!(v.as_str(), "1" | "true" | "yes"));
        let list = |k: &str| -> Vec<String> {
            get(k)
                .map(|v| {
                    v.split(',')
                        .map(|e| e.trim().to_string())
                        .filter(|e| !e.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };

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
        if let Some(v) = get("MAX_INFLIGHT_BYTES").and_then(|v| parse_bytes(&v)) {
            c.max_inflight_bytes = v;
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
        if let Some(v) = get("PDFTOPPM_BIN") {
            c.pdftoppm_bin = if v == "none" {
                None
            } else {
                Some(PathBuf::from(v))
            };
        }
        c.translations_dir = get("TRANSLATIONS_DIR").map(PathBuf::from);
        if let Some(v) = get("FONTS_DIR") {
            c.fonts_dir = PathBuf::from(v);
        }
        if let Some(v) = get("ASSETS_DIR") {
            c.assets_dir = PathBuf::from(v);
        }
        c.store = get("STORE");
        if let Some(v) = get("MAX_TEMPLATE_BYTES") {
            match parse_bytes(&v) {
                Some(n) if n > 0 => c.max_template_bytes = n,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_MAX_TEMPLATE_BYTES: bad size '{v}'")),
            }
        }
        if let Some(v) = get("MAX_TEMPLATES") {
            match v.parse::<usize>() {
                Ok(n) if n > 0 => c.max_templates = n,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_MAX_TEMPLATES: bad count '{v}'")),
            }
        }
        if let Some(v) = get("MAX_TEMPLATE_STORAGE") {
            match parse_bytes(&v) {
                Some(n) if n > 0 => c.max_template_storage = n as u64,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_MAX_TEMPLATE_STORAGE: bad size '{v}'")),
            }
        }
        if let Some(v) = get("MAX_JOB_STORAGE") {
            match parse_bytes(&v) {
                Some(n) if n > 0 => c.max_job_storage = n as u64,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_MAX_JOB_STORAGE: bad size '{v}'")),
            }
        }
        if let Some(v) = get("MAX_STORED_FONTS") {
            match v.parse::<usize>() {
                Ok(n) if n > 0 => c.max_stored_fonts = n,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_MAX_STORED_FONTS: bad count '{v}'")),
            }
        }
        if let Some(v) = get("MAX_FONT_STORAGE") {
            match parse_bytes(&v) {
                Some(n) if n > 0 => c.max_font_storage = n as u64,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_MAX_FONT_STORAGE: bad size '{v}'")),
            }
        }
        if let Some(v) = get("FONT_CACHE_DIR") {
            c.font_cache_dir = PathBuf::from(v);
        }
        if let Some(v) = get("MAX_STORED_ASSETS") {
            match v.parse::<usize>() {
                Ok(n) if n > 0 => c.max_stored_assets = n,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_MAX_STORED_ASSETS: bad count '{v}'")),
            }
        }
        if let Some(v) = get("MAX_ASSET_STORAGE") {
            match parse_bytes(&v) {
                Some(n) if n > 0 => c.max_asset_storage = n as u64,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_MAX_ASSET_STORAGE: bad size '{v}'")),
            }
        }
        if let Some(v) = get("MAX_ASSET_LIBRARY_SPACE") {
            match parse_bytes(&v) {
                Some(n) if n > 0 => c.max_asset_library_space = n as u64,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_MAX_ASSET_LIBRARY_SPACE: bad size '{v}'")),
            }
        }
        if let Some(v) = get("ASSET_CACHE_DIR") {
            c.asset_cache_dir = PathBuf::from(v);
        }
        if let Some(v) = get("STORE_SYNC_INTERVAL") {
            match parse_duration(&v) {
                Some(d) if !d.is_zero() => c.store_sync_interval = d,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_STORE_SYNC_INTERVAL: bad duration '{v}'")),
            }
        }
        // Left empty is left unset, as compose passes it; a value with no key
        // in it is a mistake that would leave the service open. A list is the
        // old setting's form, and as one key no caller would have it.
        if let Some(v) = get("API_KEY") {
            let key = v.trim();
            if key.is_empty() {
                c.problems
                    .push("RUSTICDOC_API_KEY is set but holds no key".into());
            } else if key.contains(',') {
                c.problems
                    .push("RUSTICDOC_API_KEY holds one key, with no comma in it".into());
            } else {
                c.api_key = Some(key.to_string());
            }
        }
        if let Some(v) = get("JOB_TTL") {
            match parse_duration(&v) {
                Some(d) if !d.is_zero() => c.job_ttl = d,
                _ => c
                    .problems
                    .push(format!("RUSTICDOC_JOB_TTL: bad duration '{v}'")),
            }
        }
        c.webhook_secret = get("WEBHOOK_SECRET");
        match (get("SIGNING_KEY"), get("SIGNING_CERT")) {
            (Some(key), Some(cert)) => {
                c.signing_key = Some(PathBuf::from(key));
                c.signing_cert = Some(PathBuf::from(cert));
            }
            (None, None) => {}
            // A key without its certificate, or the reverse, is a deployment
            // half done: refuse it rather than answer 503 to every signature.
            _ => c
                .problems
                .push("RUSTICDOC_SIGNING_KEY and RUSTICDOC_SIGNING_CERT go together".into()),
        }
        c.signing = crate::pdfops::sign::Settings::from_vars(vars, &mut c.problems);
        if let Some(v) = get("WEBHOOK_ALLOW_PRIVATE") {
            c.webhook_allow_private = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Some(v) = get("WEBHOOK_ATTEMPTS").and_then(|v| v.parse().ok()) {
            c.webhook_attempts = v;
        }
        if let Some(v) = get("HEADER_TIMEOUT").and_then(|v| parse_duration(&v)) {
            c.header_timeout = v;
        }
        if let Some(v) = get("MAX_CONNECTIONS").and_then(|v| v.parse().ok()) {
            c.max_connections = v;
        }
        if let Some(v) = get("MAX_CONNECTIONS_PER_PEER").and_then(|v| v.parse().ok()) {
            c.max_connections_per_peer = v;
        }
        if let Some(v) = get("STALL_TIMEOUT").and_then(|v| parse_duration(&v)) {
            c.stall_timeout = v;
        }
        if let Some(v) = get("RESPONSE_WRITE_TIMEOUT").and_then(|v| parse_duration(&v)) {
            c.response_write_timeout = v;
        }
        c.outbound_proxy = get("OUTBOUND_PROXY");
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
        if let Some(v) = get("LIBREOFFICE_MAX_DISK").and_then(|v| parse_bytes(&v)) {
            c.libreoffice_max_disk = v as u64;
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
        if let Some(v) = get("ENGINE_MEMORY").and_then(|v| parse_bytes(&v)) {
            c.engine_memory = v as u64;
        }
        if let Some(v) = get("RENDER_WORKERS").and_then(|v| v.parse().ok()) {
            c.render_workers = v;
        }
        if let Some(v) = get("MAX_ASYNC_JOBS").and_then(|v| v.parse().ok()) {
            c.max_async_jobs = v;
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
        if let Some(v) = on("CONVERT_DISABLED") {
            c.convert_disabled = v;
        }
        if let Some(v) = on("PDF_DISABLED") {
            c.pdf_disabled = v;
        }
        if let Some(v) = on("WEBHOOK_DISABLED") {
            c.webhook_disabled = v;
        }
        // Ignored, one of these would change what the service does without a
        // word: a service that relied on basic auth alone would start open.
        for (name, instead) in REMOVED_VARIABLES {
            if get(name).is_some() {
                c.problems.push(format!(
                    "RUSTICDOC_{name} is no longer supported; {instead}"
                ));
            }
        }
        c.webhook_allow_list = list("WEBHOOK_ALLOW_LIST");
        c.webhook_deny_list = list("WEBHOOK_DENY_LIST");
        c.webhook_error_allow_list = list("WEBHOOK_ERROR_ALLOW_LIST");
        c.webhook_error_deny_list = list("WEBHOOK_ERROR_DENY_LIST");
        c.download_allow_list = list("DOWNLOAD_FROM_ALLOW_LIST");
        c.download_deny_list = list("DOWNLOAD_FROM_DENY_LIST");
        for variable in ENGINE_VARIABLES {
            let name = variable.trim_start_matches("RUSTICDOC_");
            if get(name).is_some() {
                c.engine_orders.insert((*variable).to_string(), list(name));
            }
        }
        c
    }

    /// True when requests must carry credentials.
    pub fn auth_configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// True when `/metrics` needs credentials.
    pub fn metrics_guarded(&self) -> bool {
        self.auth_configured() && !self.metrics_public
    }

    /// The signing key and certificate, read and checked against each other,
    /// or `None` when signing is not configured. Errors name the variable and
    /// the file, never what the key file holds.
    pub fn signer(&self) -> Result<Option<crate::pdfops::sign::Signer>, String> {
        match (&self.signing_key, &self.signing_cert) {
            (Some(key), Some(cert)) => {
                crate::pdfops::sign::load_key(&key.to_string_lossy(), cert, &self.signing.pkcs11)
                    .map(Some)
                    .map_err(|e| format!("RUSTICDOC_SIGNING_KEY and RUSTICDOC_SIGNING_CERT: {e}"))
            }
            (None, None) => Ok(None),
            _ => Err("RUSTICDOC_SIGNING_KEY and RUSTICDOC_SIGNING_CERT go together".into()),
        }
    }

    /// Every signing key, read and checked, with the timestamp authority and
    /// the rest of [`Config::signing`]. Nothing configured is an empty service.
    pub fn signing(&self) -> Result<crate::pdfops::sign::Signing, String> {
        let key = self.signing_key.as_ref().map(|k| k.to_string_lossy());
        let default = match (&key, &self.signing_cert) {
            (Some(key), Some(cert)) => Some((key.as_ref(), cert.as_path())),
            (None, None) => None,
            _ => return Err("RUSTICDOC_SIGNING_KEY and RUSTICDOC_SIGNING_CERT go together".into()),
        };
        crate::pdfops::sign::Signing::load(default, &self.signing)
    }

    /// Where templates and pages find the assets they name: the mounted
    /// directory, then the copy of the stored ones when there is a store.
    pub fn asset_dirs(&self) -> rusticdoc_template::assets::AssetDirs {
        rusticdoc_template::assets::AssetDirs {
            sent: None,
            mounted: Some(self.assets_dir.clone()),
            stored: self.store.as_ref().map(|_| self.asset_cache_dir.clone()),
        }
    }

    /// Settings the service refuses to start with.
    pub fn check(&self) -> Result<(), String> {
        if let Some(problem) = self.problems.first() {
            return Err(problem.clone());
        }
        if let Some(url) = &self.store {
            crate::store::Store::open(url)?;
        }
        self.signing()?;
        // A dictionary that cannot be read stops the start, not the first render.
        crate::dictionaries::Dictionaries::open(self.translations_dir.clone())?;
        for (variable, engines) in &self.engine_orders {
            for engine in engines {
                if !matches!(
                    engine.as_str(),
                    "qpdf" | "pdfcpu" | "lopdf" | "qpdf-metadata"
                ) {
                    return Err(format!(
                        "{variable} names an unknown engine '{engine}'; use lopdf, qpdf, pdfcpu or qpdf-metadata"
                    ));
                }
            }
        }
        for (name, list) in [
            ("RUSTICDOC_WEBHOOK_ALLOW_LIST", &self.webhook_allow_list),
            ("RUSTICDOC_WEBHOOK_DENY_LIST", &self.webhook_deny_list),
            (
                "RUSTICDOC_WEBHOOK_ERROR_ALLOW_LIST",
                &self.webhook_error_allow_list,
            ),
            (
                "RUSTICDOC_WEBHOOK_ERROR_DENY_LIST",
                &self.webhook_error_deny_list,
            ),
            (
                "RUSTICDOC_DOWNLOAD_FROM_ALLOW_LIST",
                &self.download_allow_list,
            ),
            (
                "RUSTICDOC_DOWNLOAD_FROM_DENY_LIST",
                &self.download_deny_list,
            ),
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
    fn a_bad_key_or_a_removed_setting_stops_the_service() {
        for vars in [
            &[("RUSTICDOC_API_KEY", "  ")][..],
            &[("RUSTICDOC_API_KEY", " , ")],
            &[("RUSTICDOC_API_KEY", "first-key,second-key")],
            &[("RUSTICDOC_API_KEYS", "first-key")],
            &[("RUSTICDOC_ADMIN_KEYS", "ops-key")],
            &[("RUSTICDOC_TENANT_KEYS", "acme:k1")],
            &[("RUSTICDOC_BASIC_AUTH_USERNAME", "u")],
            &[("RUSTICDOC_BASIC_AUTH_PASSWORD", "p")],
            &[("RUSTICDOC_BASIC_AUTH_ADMIN", "true")],
            &[("RUSTICDOC_OIDC_ISSUER", "https://idp.example.com")],
            &[("RUSTICDOC_OIDC_AUDIENCE", "a")],
            &[("RUSTICDOC_OIDC_JWKS_URL", "https://idp.example.com/keys")],
            &[("RUSTICDOC_OIDC_ADMIN_SUBJECTS", "alice")],
            &[("RUSTICDOC_OIDC_ADMIN_CLAIM", "groups=admins")],
            &[("RUSTICDOC_STORE_SCOPE", "shared")],
            &[("RUSTICDOC_STORE_SCOPE", "caller")],
            &[("RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER", "4")],
            &[("RUSTICDOC_SIGNING_ALLOW", "*")],
            // Beside the key too: the setting no longer does what it says.
            &[
                ("RUSTICDOC_API_KEY", "a-key"),
                ("RUSTICDOC_API_KEYS", "a-key,another-key"),
            ],
        ] {
            assert!(from(vars).check().is_err(), "{vars:?}");
        }
        let err = from(&[("RUSTICDOC_API_KEYS", "first-key")])
            .check()
            .unwrap_err();
        assert!(
            err.contains("RUSTICDOC_API_KEYS is no longer supported"),
            "{err}"
        );
        // The message never repeats what may be a key.
        let err = from(&[("RUSTICDOC_API_KEY", "sekrit-one,sekrit-two")])
            .check()
            .unwrap_err();
        assert!(!err.contains("sekrit"), "{err}");
    }

    #[test]
    fn the_key_is_read() {
        // Left empty is left unset, as compose passes it.
        let empty = from(&[("RUSTICDOC_API_KEY", "")]);
        assert!(empty.check().is_ok());
        assert!(!empty.auth_configured());
        let key = from(&[("RUSTICDOC_API_KEY", " a-key ")]);
        assert!(key.check().is_ok());
        assert_eq!(key.api_key.as_deref(), Some("a-key"));
        assert!(key.auth_configured());
        assert!(from(&[]).check().is_ok());
        assert!(!from(&[]).auth_configured());
    }

    #[test]
    fn invalid_url_patterns_stop_the_service() {
        for name in [
            "RUSTICDOC_WEBHOOK_ALLOW_LIST",
            "RUSTICDOC_WEBHOOK_DENY_LIST",
            "RUSTICDOC_WEBHOOK_ERROR_ALLOW_LIST",
            "RUSTICDOC_WEBHOOK_ERROR_DENY_LIST",
            "RUSTICDOC_DOWNLOAD_FROM_ALLOW_LIST",
            "RUSTICDOC_DOWNLOAD_FROM_DENY_LIST",
        ] {
            assert!(from(&[(name, "(")]).check().is_err(), "{name}");
            assert!(from(&[(name, "^https://a,^https://b")]).check().is_ok());
        }
    }

    #[test]
    fn route_switches_are_rusticdoc_booleans() {
        let c = from(&[
            ("RUSTICDOC_CONVERT_DISABLED", "true"),
            ("RUSTICDOC_PDF_DISABLED", "1"),
            ("RUSTICDOC_WEBHOOK_DISABLED", "yes"),
        ]);
        assert!(c.convert_disabled && c.pdf_disabled && c.webhook_disabled);
        let c = from(&[("RUSTICDOC_CONVERT_DISABLED", "TRUE")]);
        assert!(!c.convert_disabled);
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
    fn engine_memory_is_off_by_default_and_parses_sizes() {
        assert_eq!(from(&[]).engine_memory, 0);
        assert_eq!(
            from(&[("RUSTICDOC_ENGINE_MEMORY", "256MB")]).engine_memory,
            256 * 1024 * 1024
        );
        // An unparseable size leaves the default in place.
        assert_eq!(
            from(&[("RUSTICDOC_ENGINE_MEMORY", "lots")]).engine_memory,
            0
        );
    }

    #[test]
    fn root_paths() {
        assert_eq!(normalise_root(""), "/");
        assert_eq!(normalise_root("/"), "/");
        assert_eq!(normalise_root("foo"), "/foo/");
        assert_eq!(normalise_root("/foo/bar/"), "/foo/bar/");
    }

    #[test]
    fn engine_lists_are_read_and_unknown_engines_stop_the_service() {
        assert!(Config::default().check().is_ok());
        let c = from(&[("RUSTICDOC_PDF_ENGINES_MERGE", "qpdf, lopdf")]);
        assert!(c.check().is_ok());
        assert_eq!(
            c.engine_orders["RUSTICDOC_PDF_ENGINES_MERGE"],
            ["qpdf", "lopdf"]
        );
        for bad in ["pdftk", "exiftool", "nonsense"] {
            let err = from(&[("RUSTICDOC_PDF_ENGINES_STAMP", bad)])
                .check()
                .unwrap_err();
            assert!(err.contains(bad), "{err}");
        }
    }

    #[test]
    fn the_port_defaults_to_9027() {
        assert_eq!(Config::default().bind, "0.0.0.0:9027");
        assert_eq!(
            from(&[("RUSTICDOC_BIND", "127.0.0.1:8000")]).bind,
            "127.0.0.1:8000"
        );
    }

    #[test]
    fn store_settings_are_read_and_bad_ones_stop_the_service() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("file://{}/store", dir.path().display());
        let c = from(&[
            ("RUSTICDOC_STORE", url.as_str()),
            ("RUSTICDOC_MAX_TEMPLATES", "7"),
            ("RUSTICDOC_MAX_TEMPLATE_BYTES", "1MB"),
            ("RUSTICDOC_STORE_SYNC_INTERVAL", "5s"),
        ]);
        assert!(c.check().is_ok());
        assert_eq!(c.max_templates, 7);
        assert_eq!(c.max_template_bytes, 1024 * 1024);
        assert_eq!(c.store_sync_interval, Duration::from_secs(5));
        assert!(dir.path().join("store").is_dir());

        for bad in [
            &[("RUSTICDOC_STORE", "https://example.com")][..],
            &[("RUSTICDOC_STORE", "s3://key:secret@bucket")],
            &[("RUSTICDOC_MAX_TEMPLATES", "0")],
            &[("RUSTICDOC_MAX_TEMPLATE_BYTES", "lots")],
            &[("RUSTICDOC_STORE_SYNC_INTERVAL", "0s")],
        ] {
            assert!(from(bad).check().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn signing_needs_both_files_and_a_key_that_fits() {
        let dir = tempfile::tempdir().unwrap();
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let cert = rcgen::CertificateParams::new(vec!["signer.test".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let other = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let key_path = dir.path().join("key.pem");
        let other_path = dir.path().join("other.pem");
        let cert_path = dir.path().join("cert.pem");
        std::fs::write(&key_path, key.serialize_pem()).unwrap();
        std::fs::write(&other_path, other.serialize_pem()).unwrap();
        std::fs::write(&cert_path, cert.pem()).unwrap();
        let (key_path, other_path, cert_path) = (
            key_path.to_str().unwrap(),
            other_path.to_str().unwrap(),
            cert_path.to_str().unwrap(),
        );

        assert!(from(&[]).signer().unwrap().is_none());
        let c = from(&[
            ("RUSTICDOC_SIGNING_KEY", key_path),
            ("RUSTICDOC_SIGNING_CERT", cert_path),
        ]);
        assert!(c.check().is_ok(), "{:?}", c.check());
        assert!(c.signer().unwrap().is_some());

        for (bad, says) in [
            (&[("RUSTICDOC_SIGNING_KEY", key_path)][..], "go together"),
            (&[("RUSTICDOC_SIGNING_CERT", cert_path)], "go together"),
            (
                &[
                    ("RUSTICDOC_SIGNING_KEY", other_path),
                    ("RUSTICDOC_SIGNING_CERT", cert_path),
                ],
                "does not belong",
            ),
            (
                &[
                    ("RUSTICDOC_SIGNING_KEY", "/nonexistent/key.pem"),
                    ("RUSTICDOC_SIGNING_CERT", cert_path),
                ],
                "cannot read",
            ),
        ] {
            let err = from(bad).check().unwrap_err();
            assert!(err.contains(says), "{bad:?}: {err}");
        }
    }

    #[test]
    fn jobs_and_previews_are_read() {
        let c = from(&[
            ("RUSTICDOC_JOB_TTL", "10m"),
            ("RUSTICDOC_PDFTOPPM_BIN", "none"),
        ]);
        assert!(c.check().is_ok(), "{:?}", c.check());
        assert_eq!(c.job_ttl, Duration::from_secs(600));
        assert!(c.pdftoppm_bin.is_none());

        for bad in [
            &[("RUSTICDOC_JOB_TTL", "0s")][..],
            &[("RUSTICDOC_JOB_TTL", "soon")],
        ] {
            assert!(from(bad).check().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_bytes("64MB"), Some(64 * 1024 * 1024));
        assert_eq!(parse_bytes("512kb"), Some(512 * 1024));
        assert_eq!(parse_bytes("1000"), Some(1000));
        assert_eq!(parse_bytes("lots"), None);
    }
}
