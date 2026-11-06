//! Gotenberg's routes around the forms: `/`, `/favicon.ico`, `/version`,
//! `/debug` and its metrics.

use std::collections::BTreeMap;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use rusticdoc_core::Config;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::state::AppState;

/// Everything here sits behind authentication.
pub fn routes(config: &Config) -> Router<AppState> {
    let mut router = Router::new()
        .route("/", get(root))
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .route("/version", get(version));
    if config.compat.enable_debug_route {
        router = router.route("/debug", get(debug));
    }
    if !config.compat.prometheus_disabled {
        router = router.route(&config.compat.prometheus_path, get(metrics));
    }
    router
}

async fn root() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=UTF-8")],
        Html(
            "Hey, Gotenberg has no UI, it's an API. Head to the <a href=\"https://gotenberg.dev\">documentation</a> to learn how to interact with it 🚀",
        ),
    )
        .into_response()
}

async fn version() -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=UTF-8")],
        rusticdoc_core::VERSION,
    )
        .into_response()
}

/// The two gauges Gotenberg reports for LibreOffice, under its names.
async fn metrics(State(state): State<AppState>) -> Response {
    let namespace = &state.config.compat.prometheus_namespace;
    let (queue, restarts) = match &state.engines.libreoffice {
        Some(office) => office.stats(),
        None => (0, 0),
    };
    let body = format!(
        "# HELP {namespace}_libreoffice_requests_queue_size Current number of LibreOffice conversion requests waiting to be treated.\n\
         # TYPE {namespace}_libreoffice_requests_queue_size gauge\n\
         {namespace}_libreoffice_requests_queue_size {queue}\n\
         # HELP {namespace}_libreoffice_restarts_count Current number of LibreOffice restarts.\n\
         # TYPE {namespace}_libreoffice_restarts_count gauge\n\
         {namespace}_libreoffice_restarts_count {restarts}\n"
    );
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8; escaping=underscores",
        )],
        body,
    )
        .into_response()
}

async fn tool_version(bin: &std::path::Path, arg: &str) -> Option<String> {
    let output = tokio::process::Command::new(bin)
        .arg(arg)
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
}

/// What the engines report, once: asking takes a process each.
async fn engine_versions(config: &Config) -> &'static (Option<String>, Option<String>) {
    static VERSIONS: OnceCell<(Option<String>, Option<String>)> = OnceCell::const_new();
    VERSIONS
        .get_or_init(|| async {
            let qpdf = match &config.qpdf_bin {
                Some(bin) => tool_version(bin, "--version").await,
                None => None,
            };
            let pdfcpu = match &config.pdfcpu_bin {
                Some(bin) => tool_version(bin, "version").await,
                None => None,
            };
            (qpdf, pdfcpu)
        })
        .await
}

async fn debug(State(state): State<AppState>) -> Json<Value> {
    let config = &state.config;
    if !config.compat.build_debug_data {
        return Json(json!({
            "version": "",
            "timezone": "",
            "architecture": "",
            "modules": null,
            "modules_additional_data": null,
            "flags": null,
        }));
    }
    let (qpdf, pdfcpu) = engine_versions(config).await;
    let unknown = || "unknown".to_string();
    Json(json!({
        "version": rusticdoc_core::VERSION,
        "timezone": std::env::var("TZ").ok().filter(|tz| !tz.is_empty()).unwrap_or_else(|| "UTC".to_string()),
        "architecture": match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "amd64",
            other => other,
        },
        "modules": [
            "api",
            "libreoffice",
            "libreoffice-api",
            "libreoffice-pdfengine",
            "pdfcpu",
            "pdfengines",
            "prometheus",
            "qpdf",
            "qpdf-metadata",
            "webhook",
        ],
        "modules_additional_data": {
            "libreoffice-api": { "version": rusticdoc_core::VERSION },
            "pdfcpu": { "version": pdfcpu.clone().unwrap_or_else(unknown) },
            "qpdf": { "version": qpdf.clone().unwrap_or_else(unknown) },
        },
        "flags": flags(config),
    }))
}

/// The settings by Gotenberg's flag names, as `/debug` lists them.
fn flags(config: &Config) -> BTreeMap<&'static str, String> {
    let compat = &config.compat;
    let list = |variable: &str, default: &str| -> String {
        match compat.engine_orders.get(variable) {
            Some(names) => format!("[{}]", names.join(",")),
            None => default.to_string(),
        }
    };
    let private = |allowed: bool| if allowed { "false" } else { "true" }.to_string();
    let (bind_ip, port) = config
        .bind
        .rsplit_once(':')
        .map(|(ip, port)| (ip.to_string(), port.to_string()))
        .unwrap_or_default();
    BTreeMap::from([
        (
            "api-bind-ip",
            if bind_ip == "0.0.0.0" {
                String::new()
            } else {
                bind_ip
            },
        ),
        ("api-body-limit", config.body_limit.to_string()),
        ("api-correlation-id-header", compat.trace_header.clone()),
        (
            "api-disable-download-from",
            config.download_from_disabled.to_string(),
        ),
        ("api-disable-health-check-logging", "false".to_string()),
        (
            "api-disable-debug-route-telemetry",
            compat.debug_telemetry_off.to_string(),
        ),
        (
            "api-disable-health-check-route-telemetry",
            compat.health_telemetry_off.to_string(),
        ),
        (
            "api-disable-root-route-telemetry",
            compat.root_telemetry_off.to_string(),
        ),
        (
            "api-disable-version-route-telemetry",
            compat.version_telemetry_off.to_string(),
        ),
        ("api-download-from-allow-list", "[.+]".to_string()),
        ("api-download-from-deny-list", "[]".to_string()),
        (
            "api-download-from-deny-private-ips",
            private(config.download_from_allow_private),
        ),
        ("api-download-from-deny-public-ips", "false".to_string()),
        ("api-download-from-max-retry", "0".to_string()),
        (
            "api-enable-basic-auth",
            config.basic_auth.is_some().to_string(),
        ),
        (
            "api-enable-debug-route",
            compat.enable_debug_route.to_string(),
        ),
        ("api-port", port),
        ("api-port-from-env", String::new()),
        ("api-root-path", config.root_path.clone()),
        ("api-start-timeout", "30s".to_string()),
        ("api-timeout", format!("{}s", config.api_timeout.as_secs())),
        ("api-tls-cert-file", String::new()),
        ("api-tls-key-file", String::new()),
        ("api-trace-header", compat.trace_header.clone()),
        (
            "gotenberg-build-debug-data",
            compat.build_debug_data.to_string(),
        ),
        ("gotenberg-graceful-shutdown-duration", "30s".to_string()),
        ("libreoffice-auto-start", "false".to_string()),
        (
            "libreoffice-disable-routes",
            compat.libreoffice_disable_routes.to_string(),
        ),
        ("libreoffice-idle-shutdown-timeout", "0s".to_string()),
        ("libreoffice-max-queue-size", "0".to_string()),
        ("libreoffice-restart-after", "10".to_string()),
        ("libreoffice-start-timeout", "20s".to_string()),
        ("log-enable-gcp-fields", "false".to_string()),
        ("log-fields-prefix", String::new()),
        ("log-format", "auto".to_string()),
        ("log-level", config.log_level.clone()),
        ("log-std-enable-gcp-fields", "false".to_string()),
        ("log-std-format", "auto".to_string()),
        (
            "pdfengines-convert-engines",
            list("PDFENGINES_CONVERT_ENGINES", "[libreoffice-pdfengine]"),
        ),
        (
            "pdfengines-disable-routes",
            compat.pdfengines_disable_routes.to_string(),
        ),
        ("pdfengines-engines", list("PDFENGINES_ENGINES", "[]")),
        (
            "pdfengines-flatten-engines",
            list("PDFENGINES_FLATTEN_ENGINES", "[qpdf,lopdf]"),
        ),
        (
            "pdfengines-merge-engines",
            list("PDFENGINES_MERGE_ENGINES", "[qpdf,pdfcpu,lopdf]"),
        ),
        (
            "pdfengines-read-bookmarks-engines",
            list("PDFENGINES_READ_BOOKMARKS_ENGINES", "[lopdf,pdfcpu]"),
        ),
        (
            "pdfengines-read-metadata-engines",
            list("PDFENGINES_READ_METADATA_ENGINES", "[qpdf-metadata]"),
        ),
        (
            "pdfengines-split-engines",
            list("PDFENGINES_SPLIT_ENGINES", "[lopdf,qpdf,pdfcpu]"),
        ),
        (
            "pdfengines-write-bookmarks-engines",
            list("PDFENGINES_WRITE_BOOKMARKS_ENGINES", "[lopdf,pdfcpu]"),
        ),
        (
            "pdfengines-write-metadata-engines",
            list("PDFENGINES_WRITE_METADATA_ENGINES", "[qpdf-metadata]"),
        ),
        ("prometheus-collect-interval", "1s".to_string()),
        (
            "prometheus-disable-collect",
            compat.prometheus_disabled.to_string(),
        ),
        (
            "prometheus-disable-route-logging",
            compat.prometheus_telemetry_off.to_string(),
        ),
        (
            "prometheus-disable-route-telemetry",
            compat.prometheus_telemetry_off.to_string(),
        ),
        ("prometheus-namespace", compat.prometheus_namespace.clone()),
        ("prometheus-metrics-path", compat.prometheus_path.clone()),
        ("webhook-allow-list", "[.+]".to_string()),
        ("webhook-client-timeout", "30s".to_string()),
        ("webhook-deny-list", "[]".to_string()),
        (
            "webhook-deny-private-ips",
            private(config.webhook_allow_private),
        ),
        ("webhook-deny-public-ips", "false".to_string()),
        ("webhook-disable", compat.webhook_disabled.to_string()),
        ("webhook-error-allow-list", "[]".to_string()),
        ("webhook-error-deny-list", "[]".to_string()),
        (
            "webhook-max-retry",
            config.webhook_attempts.saturating_sub(1).to_string(),
        ),
        ("webhook-retry-max-wait", "30s".to_string()),
        ("webhook-retry-min-wait", "1s".to_string()),
    ])
}
