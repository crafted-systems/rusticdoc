//! rusticdoc server: the HTTP API over the core pipeline.
//!
//! Routes are versioned under `/v1`. Every response carries an
//! `x-request-id`; webhooks deliver results asynchronously with an HMAC
//! signature; `/openapi.json` describes the API for SDK generation.

pub mod admission;
pub mod auth;
pub mod error;
pub mod fontsync;
pub mod jobs;
pub mod metrics;
pub mod oidc;
pub mod openapi;
pub mod preview;
pub mod routes;
pub mod signing;
pub mod stall;
pub mod state;
pub mod telemetry;
pub mod variables;
pub mod webhook;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{ConnectInfo, DefaultBodyLimit};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::sync::Semaphore;
use tower::ServiceExt;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

pub use state::AppState;

pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The full application router with its middleware.
pub fn app(state: AppState) -> Router {
    let timeout = state.config.api_timeout + Duration::from_secs(5);
    let body_limit = state.config.body_limit;
    let version = HeaderValue::from_static(rusticdoc_core::VERSION);

    // Behind authentication, so it says nothing the version route would not.
    let own_version = SetResponseHeaderLayer::if_not_present(
        HeaderName::from_static("x-rusticdoc-version"),
        version,
    );

    let guarded = |group: Router<AppState>, limit: Duration| {
        group
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                admission::admit,
            ))
            .layer(own_version.clone())
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                auth::require,
            ))
            .layer(DefaultBodyLimit::max(body_limit))
            .layer(TimeoutLayer::with_status_code(
                StatusCode::SERVICE_UNAVAILABLE,
                limit,
            ))
    };
    let api = guarded(routes::router(&state.config), timeout);
    let long = guarded(
        routes::long_router(&state.config),
        state.config.batch_timeout + Duration::from_secs(5),
    );

    // Without credentials configured nothing is hidden from anyone. With them,
    // the open routes do not say which version runs.
    let mut public = routes::public(&state.config);
    if !state.config.auth_configured() {
        public = public.layer(own_version);
    }
    let router = Router::new()
        .merge(public)
        .merge(api)
        .merge(long)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            metrics::observe,
        ))
        .layer(axum::middleware::from_fn(telemetry::access))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
        .layer(axum::middleware::from_fn(clean_request_id))
        .layer(cors(&state.config))
        .layer(CatchPanicLayer::new())
        .with_state(state.clone());

    // RUSTICDOC_ROOT_PATH: everything, health included, under a prefix.
    let root = state.config.root_path.trim_end_matches('/');
    if root.is_empty() {
        router
    } else {
        Router::new().nest(root, router)
    }
}

/// A request id the client chose is kept to printable ASCII and a length cap,
/// and dropped when nothing is left, so the service makes one instead.
async fn clean_request_id(
    mut req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Some(given) = req.headers().get(&REQUEST_ID) {
        let clean = given
            .to_str()
            .ok()
            .and_then(telemetry::sanitize_correlation_id)
            .and_then(|id| HeaderValue::from_str(&id).ok());
        match clean {
            Some(id) => req.headers_mut().insert(REQUEST_ID, id),
            None => req.headers_mut().remove(&REQUEST_ID),
        };
    }
    next.run(req).await
}

/// Cross-origin access is off unless `RUSTICDOC_CORS_ORIGINS` lists origins,
/// or `*` for any.
fn cors(config: &rusticdoc_core::Config) -> CorsLayer {
    use tower_http::cors::{AllowOrigin, Any};
    if config.cors_origins.is_empty() {
        return CorsLayer::new();
    }
    let origin = if config.cors_origins.iter().any(|o| o == "*") {
        AllowOrigin::any()
    } else {
        AllowOrigin::list(
            config
                .cors_origins
                .iter()
                .filter_map(|o| o.parse::<HeaderValue>().ok()),
        )
    };
    CorsLayer::new()
        .allow_origin(origin)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers([header::CONTENT_DISPOSITION, REQUEST_ID])
}

/// What an operator should hear at every start.
pub fn startup_warnings(config: &rusticdoc_core::Config) -> Vec<String> {
    let mut warnings = Vec::new();
    if !config.auth_configured() {
        warnings.push(
            "no authentication is configured, so anyone who can reach this service can use it; set RUSTICDOC_API_KEYS".to_string(),
        );
    }
    if !config.webhook_disabled && config.webhook_secret.is_none() {
        warnings.push(
            "webhooks are possible and deliveries are not signed; set RUSTICDOC_WEBHOOK_SECRET"
                .to_string(),
        );
    }
    if config.outbound_proxy.is_some() {
        warnings.push(
            "RUSTICDOC_OUTBOUND_PROXY is set, so DNS pinning no longer applies: the proxy resolves names itself and must enforce its own egress policy, including blocking private ranges"
                .to_string(),
        );
    }
    if config.store.is_some() && !config.auth_configured() {
        warnings.push(
            "RUSTICDOC_STORE is set and no credentials are, so anyone who can reach this service can store, read and delete templates and fonts"
                .to_string(),
        );
    }
    if config.render_mode == rusticdoc_core::config::RenderMode::InProcess {
        warnings.push(
            "RUSTICDOC_RENDER_MODE is in-process, so a client's template can exhaust this service's memory; use isolated unless every template is trusted"
                .to_string(),
        );
    }
    let encrypt_order = config.engine_orders.get("RUSTICDOC_PDF_ENGINES_ENCRYPT");
    let pdfcpu_encrypts = match encrypt_order {
        Some(names) => names.iter().any(|name| name == "pdfcpu"),
        None => config.qpdf_bin.is_none() && config.pdfcpu_bin.is_some(),
    };
    if pdfcpu_encrypts {
        warnings.push(
            "pdfcpu may encrypt here, and it takes passwords as arguments that this user's other processes can read; use qpdf"
                .to_string(),
        );
    }
    warnings
}

/// Binds, serves, and stops cleanly on SIGINT or SIGTERM.
pub async fn serve(state: AppState) -> std::io::Result<()> {
    make_undumpable()?;
    state
        .pipeline
        .renderer()
        .check()
        .await
        .map_err(|e| std::io::Error::other(format!("the render worker cannot start: {e}")))?;
    let bind = state.config.bind.clone();
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    for warning in startup_warnings(&state.config) {
        tracing::warn!("{warning}");
    }
    // Which certificate signs, never anything about the key itself.
    for (name, signer) in state.engines.signing.keys() {
        tracing::info!(
            key = name.unwrap_or("default"),
            subject = signer.subject(),
            algorithm = %signer.algorithm(),
            not_after = %signer.expiry(),
            "PDF signing key loaded"
        );
        for warning in signer.warnings() {
            tracing::warn!(key = name.unwrap_or("default"), "{warning}");
        }
    }
    if let Some(tsa) = state.engines.signing.tsa() {
        tracing::info!(url = %webhook::redact(&tsa.url), level = state.engines.signing.default_level().name(), "PDF signing timestamp authority");
    }
    tracing::info!(%bind, version = rusticdoc_core::VERSION, engines = %state.engines.describe(), "rusticdoc listening");
    // Start the LibreOffice workers now rather than on the first request,
    // so the first conversions are as fast as the rest.
    let fonts_first = state.clone();
    if state.store.is_some() {
        fontsync::spawn(state.clone());
        jobs::spawn_cleanup(state.clone());
    }
    if let Some(lo) = state.engines.libreoffice.clone() {
        tokio::spawn(async move {
            // Workers read their fonts as they start, so start them on the right ones.
            if fonts_first.store.is_some() {
                let _ = fontsync::run_once(&fonts_first).await;
            }
            match lo.warm_up().await {
                Ok(()) => tracing::info!("LibreOffice workers warm"),
                Err(e) => {
                    tracing::warn!(error = %e, "LibreOffice warm-up failed; workers start on demand")
                }
            }
        });
    }
    let limits = ConnectionLimits::from_config(&state.config);
    serve_on(listener, app(state), limits, shutdown_signal()).await
}

/// Closes `/proc/<pid>/environ` and `/proc/<pid>/mem` of this process to its
/// children, which run as the same user and work on documents from clients. They
/// get their own setting back when they exec.
#[cfg(target_os = "linux")]
fn make_undumpable() -> std::io::Result<()> {
    // SAFETY: prctl with these arguments only sets a flag on this process.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn make_undumpable() -> std::io::Result<()> {
    Ok(())
}

/// What one connection may cost.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionLimits {
    /// How long a client may take to send its request headers.
    pub header_timeout: Duration,
    /// Connections served at once.
    pub max_connections: usize,
    /// Connections served at once from one address; 0 is no limit. Behind a load
    /// balancer every connection has the same peer, so it is off by default.
    pub max_connections_per_peer: usize,
    /// How long a connection may stay blocked on its client: a write that does not
    /// finish, or a body a handler waits for that does not arrive.
    pub stall_timeout: Duration,
    /// How long writes may spend blocked in all, until everything sent has gone,
    /// however often each one moves.
    pub write_timeout: Duration,
    /// How long reading one request's body may take, even while it moves.
    pub body_timeout: Duration,
}

impl ConnectionLimits {
    pub fn from_config(config: &rusticdoc_core::Config) -> Self {
        Self {
            header_timeout: config.header_timeout,
            max_connections: config.max_connections,
            max_connections_per_peer: config.max_connections_per_peer,
            stall_timeout: config.stall_timeout,
            write_timeout: config.response_write_timeout,
            body_timeout: config.api_timeout,
        }
    }
}

/// What the per-peer limit counts by: an IPv4 address, or an IPv6 /64, since one
/// client usually holds a whole one. An IPv4-mapped address is its IPv4 address, so
/// one host cannot count twice.
pub fn peer_key(ip: std::net::IpAddr) -> std::net::IpAddr {
    use std::net::{IpAddr, Ipv6Addr};
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

/// One address's connections, for the per-peer limit.
#[derive(Clone, Default)]
struct PeerCounts(Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>>);

struct PeerPlace {
    counts: PeerCounts,
    ip: std::net::IpAddr,
}

impl PeerCounts {
    fn take(&self, ip: std::net::IpAddr, limit: usize) -> Option<PeerPlace> {
        let ip = peer_key(ip);
        let mut counts = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let count = counts.entry(ip).or_insert(0);
        if *count >= limit {
            return None;
        }
        *count += 1;
        Some(PeerPlace {
            counts: self.clone(),
            ip,
        })
    }
}

impl Drop for PeerPlace {
    fn drop(&mut self) {
        let mut counts = self.counts.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = counts.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.ip);
            }
        }
    }
}

/// Serves until `shutdown`, then lets the connections in flight finish.
///
/// `axum::serve` sets no timer, which turns off hyper's timeout on request
/// headers, and has no limit on connections, so a client could hold sockets for
/// nothing. A connection past the limit waits in the listen queue.
pub async fn serve_on(
    listener: tokio::net::TcpListener,
    router: Router,
    limits: ConnectionLimits,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let places = Arc::new(Semaphore::new(limits.max_connections.max(1)));
    let peers = PeerCounts::default();
    let (closing, closed) = tokio::sync::watch::channel(false);
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let place = tokio::select! {
            place = places.clone().acquire_owned() => place.map_err(std::io::Error::other)?,
            _ = &mut shutdown => break,
        };
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(e) => {
                    // Out of file descriptors, most likely: wait for some.
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            _ = &mut shutdown => break,
        };
        let peer_place = if limits.max_connections_per_peer > 0 {
            match peers.take(peer.ip(), limits.max_connections_per_peer) {
                Some(taken) => Some(taken),
                None => {
                    tracing::warn!(remote_addr = %peer, "too many connections from one address");
                    continue;
                }
            }
        } else {
            None
        };
        let _ = stream.set_nodelay(true);
        let service = TowerToHyperService::new(router.clone().map_request(
            move |req: axum::http::Request<hyper::body::Incoming>| {
                let mut req = req.map(|body| {
                    axum::body::Body::new(stall::GuardedBody::new(
                        body,
                        limits.stall_timeout,
                        limits.body_timeout,
                    ))
                });
                req.extensions_mut().insert(ConnectInfo(peer));
                req
            },
        ));
        let mut closed = closed.clone();
        tokio::spawn(async move {
            let _places = (place, peer_place);
            let stream = stall::StallGuard::new(stream, limits.stall_timeout, limits.write_timeout);
            let conn = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(limits.header_timeout)
                .serve_connection(TokioIo::new(stream), service);
            let mut conn = std::pin::pin!(conn);
            let mut closing = false;
            loop {
                tokio::select! {
                    result = conn.as_mut() => {
                        if let Err(e) = result {
                            tracing::debug!(error = %e, "connection ended");
                        }
                        break;
                    }
                    _ = closed.changed(), if !closing => {
                        closing = true;
                        conn.as_mut().graceful_shutdown();
                    }
                }
            }
        });
    }
    let _ = closing.send(true);
    // Every place back means every connection has finished.
    let all = u32::try_from(limits.max_connections.max(1)).unwrap_or(u32::MAX);
    let _ = places.acquire_many(all).await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown requested; finishing in-flight requests");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> std::net::IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn one_ipv6_64_counts_as_one_peer() {
        let peers = PeerCounts::default();
        let first = peers.take(ip("2001:db8:1:2:aaaa::1"), 1).unwrap();
        assert!(peers.take(ip("2001:db8:1:2:bbbb::2"), 1).is_none());
        assert!(
            peers.take(ip("2001:db8:1:3::1"), 1).is_some(),
            "another /64"
        );
        drop(first);
        assert!(peers.take(ip("2001:db8:1:2:cccc::3"), 1).is_some());
    }

    #[test]
    fn an_ipv4_mapped_address_is_its_ipv4_address() {
        let peers = PeerCounts::default();
        let _first = peers.take(ip("203.0.113.9"), 1).unwrap();
        assert!(peers.take(ip("::ffff:203.0.113.9"), 1).is_none());
        assert_eq!(peer_key(ip("::ffff:203.0.113.9")), ip("203.0.113.9"));
        assert!(peers.take(ip("203.0.113.10"), 1).is_some());
    }
}
