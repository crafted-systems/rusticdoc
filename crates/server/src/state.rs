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
    pub http: reqwest::Client,
    pub started: Instant,
    /// Places for webhook jobs.
    pub jobs: crate::jobs::JobLimiter,
    /// Verifies bearer tokens when OpenID Connect is on.
    pub oidc: Option<Arc<crate::oidc::Verifier>>,
    /// The loaded fonts, scanned on first use and on reload.
    pub fonts: Arc<tokio::sync::RwLock<Option<Arc<Vec<rusticdoc_core::fonts::FontInfo>>>>>,
    /// The fonts directory's signature and when a reload last ran.
    pub fonts_reload: Arc<tokio::sync::Mutex<Option<(String, Instant)>>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let engines = Arc::new(Engines::from_config(&config));
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
        let pipeline = Pipeline::new(engines.clone(), config.api_timeout)
            .with_max_output(config.max_render_bytes)
            .with_renderer(renderer);
        let http = reqwest::Client::builder()
            .user_agent(format!("rusticdoc/{}", rusticdoc_core::VERSION))
            .timeout(config.api_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client");
        let oidc = config
            .compat
            .oidc
            .clone()
            .map(|oidc| Arc::new(crate::oidc::Verifier::new(oidc)));
        Self {
            jobs: crate::jobs::JobLimiter::new(
                config.max_async_jobs,
                config.max_async_jobs_per_caller,
            ),
            oidc,
            config: Arc::new(config),
            engines,
            pipeline,
            metrics: Arc::new(Metrics::new()),
            http,
            started: Instant::now(),
            fonts: Default::default(),
            fonts_reload: Default::default(),
        }
    }
}
