use std::sync::Arc;
use std::time::Instant;

use rusticdoc_core::config::RenderMode;
use rusticdoc_core::render::Renderer;
use rusticdoc_core::{Config, Engines, Pipeline};

use crate::metrics::Metrics;

/// Shared by every handler. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub engines: Arc<Engines>,
    pub pipeline: Pipeline,
    pub metrics: Arc<Metrics>,
    pub started: Instant,
    /// What an outbound URL must get past.
    pub url_policies: crate::webhook::UrlPolicies,
    /// Places for webhook jobs.
    pub jobs: crate::jobs::JobLimiter,
    /// Room for request bodies held in memory.
    pub admission: crate::admission::Admission,
    /// Verifies bearer tokens when OpenID Connect is on.
    pub oidc: Option<Arc<crate::oidc::Verifier>>,
    /// The loaded fonts, scanned on first use and on reload.
    pub fonts: Arc<tokio::sync::RwLock<Option<Arc<Vec<rusticdoc_core::fonts::FontInfo>>>>>,
    /// The fonts directory's signature and when a reload last ran.
    pub fonts_reload: Arc<tokio::sync::Mutex<Option<(String, Instant)>>>,
    /// Uploaded templates and fonts, when `RUSTICDOC_STORE` names a place.
    pub store: Option<Arc<rusticdoc_core::store::Store>>,
    /// True once this replica's copy of the stored fonts is complete, which is
    /// what `/ready` waits for: documents rendered before it would silently use
    /// a fallback font.
    pub fonts_synced: Arc<std::sync::atomic::AtomicBool>,
    /// One font upload at a time: unpacking holds a whole package in memory.
    pub font_uploads: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let engines = Arc::new(Engines::from_config(&config));
        let libreoffice = engines.libreoffice.clone();
        let renderer = match config.render_mode {
            RenderMode::InProcess => Renderer::in_process(),
            RenderMode::Isolated => Renderer::isolated(
                config
                    .render_worker_bin
                    .clone()
                    .unwrap_or_else(|| "rusticdoc".into()),
                config.render_memory,
                config.render_workers,
            ),
        };
        // HTML layout in the workers sees the mounted fonts and the stored ones,
        // as LibreOffice does.
        let mut font_dirs = vec![config.fonts_dir.clone()];
        if config.store.is_some() {
            font_dirs.push(config.font_cache_dir.clone());
        }
        let renderer = match renderer.clone().with_font_dirs(&font_dirs) {
            Ok(renderer) => renderer,
            Err(e) => {
                tracing::warn!(error = %e, "HTML layout uses the system fonts only");
                renderer
            }
        };
        let pipeline = Pipeline::new(engines.clone(), config.api_timeout)
            .with_max_output(config.max_render_bytes)
            .with_renderer(renderer);
        let oidc = config
            .oidc
            .clone()
            .map(|oidc| Arc::new(crate::oidc::Verifier::new(oidc)));
        Self {
            url_policies: crate::webhook::UrlPolicies::from_config(&config)
                .expect("patterns are checked before the service starts"),
            jobs: crate::jobs::JobLimiter::new(
                config.max_async_jobs,
                config.max_async_jobs_per_caller,
            ),
            admission: crate::admission::Admission::new(config.max_inflight_bytes),
            oidc,
            store: config.store.as_deref().map(|url| {
                Arc::new(
                    rusticdoc_core::store::Store::open(url)
                        .expect("the store is checked before the service starts"),
                )
            }),
            fonts_synced: Default::default(),
            font_uploads: Arc::new(tokio::sync::Semaphore::new(1)),
            config: Arc::new(config),
            engines,
            pipeline,
            metrics: Arc::new({
                let metrics = Metrics::new();
                metrics.watch(libreoffice);
                metrics
            }),
            started: Instant::now(),
            fonts: Default::default(),
            fonts_reload: Default::default(),
        }
    }
}
