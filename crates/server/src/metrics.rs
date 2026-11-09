//! Prometheus metrics: requests by route and status, stage durations,
//! in-flight work, webhook outcomes, and, read at each scrape, the LibreOffice
//! queue and worker restarts and the compiled-template cache.

use std::sync::Mutex;

use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use prometheus_client::collector::Collector;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::{DescriptorEncoder, EncodeLabelSet, EncodeMetric};
use prometheus_client::metrics::counter::{ConstCounter, Counter};
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::{ConstGauge, Gauge};
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

use crate::state::AppState;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct RouteLabels {
    pub route: String,
    pub status: u16,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct StageLabels {
    pub stage: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct OutcomeLabels {
    pub outcome: String,
}

pub struct Metrics {
    registry: Mutex<Registry>,
    pub requests: Family<RouteLabels, Counter>,
    pub request_seconds: Family<StageLabels, Histogram>,
    pub stage_seconds: Family<StageLabels, Histogram>,
    pub inflight: Family<StageLabels, Gauge>,
    pub webhooks: Family<OutcomeLabels, Counter>,
    /// Runs of the stored-font sync: `ok`, `changed`, `failed`.
    pub font_syncs: Family<OutcomeLabels, Counter>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        let mut registry = Registry::with_prefix("rusticdoc");
        let requests = Family::<RouteLabels, Counter>::default();
        fn seconds() -> Histogram {
            Histogram::new(exponential_buckets(0.01, 2.0, 14))
        }
        let request_seconds = Family::<StageLabels, Histogram>::new_with_constructor(seconds);
        let stage_seconds = Family::<StageLabels, Histogram>::new_with_constructor(seconds);
        let inflight = Family::<StageLabels, Gauge>::default();
        let webhooks = Family::<OutcomeLabels, Counter>::default();
        let font_syncs = Family::<OutcomeLabels, Counter>::default();
        registry.register(
            "store_font_syncs",
            "Runs of the stored-font sync by outcome",
            font_syncs.clone(),
        );
        registry.register(
            "requests",
            "HTTP requests by route and status",
            requests.clone(),
        );
        registry.register(
            "request_seconds",
            "Request latency by route",
            request_seconds.clone(),
        );
        registry.register(
            "stage_seconds",
            "Pipeline stage duration",
            stage_seconds.clone(),
        );
        registry.register("inflight", "Work in progress by stage", inflight.clone());
        registry.register(
            "webhook_deliveries",
            "Webhook delivery attempts by outcome",
            webhooks.clone(),
        );
        Self {
            registry: Mutex::new(registry),
            requests,
            request_seconds,
            stage_seconds,
            inflight,
            webhooks,
            font_syncs,
        }
    }

    /// Reports the engines' own counters at every scrape: the LibreOffice
    /// queue and restarts live in the pool, and copying them into metrics as
    /// they change would need hooks in the pool for no gain.
    pub fn watch(&self, libreoffice: Option<rusticdoc_core::convert::LibreOffice>) {
        self.registry
            .lock()
            .expect("metrics registry")
            .register_collector(Box::new(EngineCollector { libreoffice }));
    }

    /// The text exposition format.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let registry = self.registry.lock().expect("metrics registry");
        encode(&mut out, &registry).expect("encode metrics");
        out
    }

    pub fn record_timings(&self, timings: &[(String, u128)]) {
        for (stage, ms) in timings {
            self.stage_seconds
                .get_or_create(&StageLabels {
                    stage: stage.clone(),
                })
                .observe(*ms as f64 / 1000.0);
        }
    }
}

/// Counts every request by matched route and status, and times it.
pub async fn observe(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let start = std::time::Instant::now();
    let res = next.run(req).await;
    state
        .metrics
        .requests
        .get_or_create(&RouteLabels {
            route: route.clone(),
            status: res.status().as_u16(),
        })
        .inc();
    state
        .metrics
        .request_seconds
        .get_or_create(&StageLabels { stage: route })
        .observe(start.elapsed().as_secs_f64());
    res
}

/// Counters the engines keep themselves, encoded on each scrape.
struct EngineCollector {
    libreoffice: Option<rusticdoc_core::convert::LibreOffice>,
}

impl std::fmt::Debug for EngineCollector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineCollector").finish()
    }
}

impl Collector for EngineCollector {
    fn encode(&self, mut encoder: DescriptorEncoder) -> Result<(), std::fmt::Error> {
        if let Some(pool) = &self.libreoffice {
            let (queued, restarts) = pool.stats();
            let gauge = ConstGauge::new(queued as i64);
            gauge.encode(encoder.encode_descriptor(
                "libreoffice_queue",
                "Conversions waiting for a LibreOffice worker",
                None,
                gauge.metric_type(),
            )?)?;
            let counter = ConstCounter::new(restarts);
            counter.encode(encoder.encode_descriptor(
                "libreoffice_worker_restarts",
                "LibreOffice workers replaced after a crash, a timeout, their job limit or a font reload",
                None,
                counter.metric_type(),
            )?)?;
        }
        let (hits, misses) = rusticdoc_core::template::cache_stats();
        for (name, help, value) in [
            (
                "template_cache_hits",
                "Renders that found their compiled template in the cache",
                hits,
            ),
            (
                "template_cache_misses",
                "Renders that had to compile their template",
                misses,
            ),
        ] {
            let counter = ConstCounter::new(value);
            counter.encode(encoder.encode_descriptor(
                name,
                help,
                None,
                counter.metric_type(),
            )?)?;
        }
        Ok(())
    }
}
