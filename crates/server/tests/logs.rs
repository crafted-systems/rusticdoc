//! What the service logs. A global subscriber, so tasks and spans behave as they do
//! in the service, and one test at a time, so each reads its own lines.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use rusticdoc_core::Config;
use rusticdoc_server::{AppState, app};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

static LOGS: LazyLock<Captured> = LazyLock::new(|| {
    let captured = Captured::default();
    tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::INFO)
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .init();
    captured
});

/// One test at a time.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn logged_since_clear() -> String {
    String::from_utf8_lossy(&LOGS.0.lock().unwrap()).into_owned()
}

fn clear() {
    LOGS.0.lock().unwrap().clear();
}

fn config() -> Config {
    Config {
        libreoffice_bin: None,
        qpdf_bin: None,
        pdfcpu_bin: None,
        ..Config::default()
    }
}

fn guarded() -> Config {
    let mut c = config();
    c.api_keys = vec!["secret".into()];
    c
}

fn from_peer(mut request: Request<Body>) -> Request<Body> {
    let peer: std::net::SocketAddr = "203.0.113.9:4242".parse().unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    request
}

#[tokio::test]
async fn failed_authentication_logs_who_and_from_where_without_the_secret() {
    let _turn = TURN.lock().await;
    clear();
    let request = Request::get("/v1/fonts")
        .header("x-api-key", "wrong-key-value")
        .body(Body::empty())
        .unwrap();
    let res = app(AppState::new(guarded()))
        .oneshot(from_peer(request))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let logged = logged_since_clear();
    assert!(logged.contains("authentication failed"), "{logged}");
    assert!(logged.contains("203.0.113.9:4242"), "{logged}");
    assert!(logged.contains("key:"), "{logged}");
    assert!(!logged.contains("wrong-key-value"), "{logged}");
}

#[tokio::test]
async fn a_basic_password_stays_out_of_the_log() {
    let _turn = TURN.lock().await;
    clear();
    let mut c = config();
    c.basic_auth = Some(("ada".into(), "right".into()));
    // "ada:wrong-password"
    let request = Request::get("/v1/fonts")
        .header(header::AUTHORIZATION, "Basic YWRhOndyb25nLXBhc3N3b3Jk")
        .body(Body::empty())
        .unwrap();
    let res = app(AppState::new(c)).oneshot(request).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let logged = logged_since_clear();
    assert!(logged.contains("basic:"), "{logged}");
    assert!(!logged.contains("wrong-password"), "{logged}");
    assert!(!logged.contains("YWRhOndyb25n"), "{logged}");
}

#[tokio::test]
async fn the_access_log_names_the_peer_and_the_caller() {
    let _turn = TURN.lock().await;
    clear();
    let request = Request::get("/v1/fonts")
        .header("x-api-key", "secret")
        .body(Body::empty())
        .unwrap();
    let res = app(AppState::new(guarded()))
        .oneshot(from_peer(request))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let logged = logged_since_clear();
    assert!(
        logged.contains(r#""remote_addr":"203.0.113.9:4242""#),
        "{logged}"
    );
    assert!(logged.contains(r#""caller":"key:"#), "{logged}");
    assert!(!logged.contains(r#""secret""#), "{logged}");
}

#[tokio::test]
async fn webhook_job_failures_are_logged_in_the_requests_span() {
    let _turn = TURN.lock().await;
    clear();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut c = config();
    c.webhook_allow_private = true;
    c.webhook_attempts = 1;
    c.api_timeout = Duration::from_millis(200);
    let body = "--b\r\nContent-Disposition: form-data; name=\"template\"; filename=\"t.txt\"\r\n\r\n{{ nobody.home }}\r\n--b--\r\n";
    let request = Request::post("/v1/render")
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=b")
        .header("x-request-id", "job-under-test")
        .header(
            "x-rusticdoc-webhook-url",
            format!("http://127.0.0.1:{port}/d"),
        )
        .body(Body::from(body))
        .unwrap();
    let res = app(AppState::new(c)).oneshot(request).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let logged = logged_since_clear();
    let line = logged
        .lines()
        .find(|line| line.contains("template_error"))
        .unwrap_or_else(|| panic!("the job's failure was not logged: {logged}"));
    assert!(line.contains("job-under-test"), "{line}");
}
