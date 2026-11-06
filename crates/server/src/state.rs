use std::sync::Arc;
use std::time::Instant;

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
    /// The loaded fonts, scanned on first use and on reload.
    pub fonts: Arc<tokio::sync::RwLock<Option<Arc<Vec<rusticdoc_core::fonts::FontInfo>>>>>,
    /// The fonts directory's signature and when a reload last ran.
    pub fonts_reload: Arc<tokio::sync::Mutex<Option<(String, Instant)>>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let engines = Arc::new(Engines::from_config(&config));
        let pipeline = Pipeline::new(engines.clone(), config.api_timeout);
        let http = reqwest::Client::builder()
            .user_agent(format!("rusticdoc/{}", rusticdoc_core::VERSION))
            .timeout(config.api_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client");
        Self {
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
