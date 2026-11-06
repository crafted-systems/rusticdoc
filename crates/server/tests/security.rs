//! Regression tests for the security review: each one fails without its fix.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use rusticdoc_core::convert::LibreOffice;
use rusticdoc_core::pdfops::fixtures;
use rusticdoc_core::{Config, Engines, Pipeline};
use rusticdoc_server::{AppState, app};
use tower::ServiceExt;

const BOUNDARY: &str = "b0undary";

type Part<'a> = (&'a str, Option<&'a str>, &'a [u8]);

fn config() -> Config {
    Config {
        libreoffice_bin: None,
        qpdf_bin: None,
        pdfcpu_bin: None,
        log_format: "pretty".into(),
        ..Config::default()
    }
}

fn service(config: Config) -> Router {
    app(AppState::new(config))
}

fn body_of(parts: &[Part<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, file, data) in parts {
        out.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes(),
        );
        if let Some(file) = file {
            out.extend_from_slice(format!("; filename=\"{file}\"").as_bytes());
        }
        out.extend_from_slice(b"\r\n\r\n");
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    out
}

fn form(path: &str, parts: &[Part<'_>]) -> axum::http::request::Builder {
    let _ = parts;
    Request::post(path).header(
        header::CONTENT_TYPE,
        format!("multipart/form-data; boundary={BOUNDARY}"),
    )
}

fn post(path: &str, parts: &[Part<'_>]) -> Request<Body> {
    form(path, parts).body(Body::from(body_of(parts))).unwrap()
}

async fn text(res: axum::response::Response) -> String {
    String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).into_owned()
}

// ---------------------------------------------------------------- F2

#[tokio::test]
async fn too_many_file_parts_are_refused_as_soon_as_passed() {
    let parts: Vec<Part<'_>> = (0..11).map(|_| ("f", Some("a.pdf"), &b""[..])).collect();
    let mut c = config();
    c.max_form_parts = 10;
    let res = service(c.clone())
        .oneshot(post("/forms/pdfengines/merge", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(text(res).await.contains("more than 10 files"));
    let res = service(c)
        .oneshot(post("/v1/pdf/merge", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(text(res).await.contains("more than 10 files"));
}

#[tokio::test]
async fn too_many_text_parts_are_refused_as_soon_as_passed() {
    let parts: Vec<Part<'_>> = (0..2_001).map(|_| ("x", None, &b"1"[..])).collect();
    let res = service(config())
        .oneshot(post("/v1/pdf/merge", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(text(res).await.contains("more than 2000 fields"));
}

// ---------------------------------------------------------------- F3

#[tokio::test]
async fn page_selections_are_capped_before_they_expand() {
    let pdf = fixtures::pages(3);
    let spec = "1-,".repeat(1_001);
    let cases: [(&str, Vec<Part<'_>>); 3] = [
        (
            "/v1/pdf/rotate",
            vec![
                ("files", Some("a.pdf"), &pdf),
                ("pages", None, spec.as_bytes()),
            ],
        ),
        (
            "/forms/pdfengines/rotate",
            vec![
                ("files", Some("a.pdf"), &pdf),
                ("rotateAngle", None, b"90"),
                ("rotatePages", None, spec.as_bytes()),
            ],
        ),
        (
            "/forms/pdfengines/split",
            vec![
                ("files", Some("a.pdf"), &pdf),
                ("splitMode", None, b"pages"),
                ("splitSpan", None, spec.as_bytes()),
            ],
        ),
    ];
    for (path, parts) in cases {
        let res = service(config()).oneshot(post(path, &parts)).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{path}");
    }
}

// ---------------------------------------------------------------- F4

#[tokio::test]
async fn stamp_text_is_capped_on_every_route() {
    let pdf = fixtures::pages(1);
    let long = "x".repeat(64 * 1024 + 1);
    let cases: [(&str, Vec<Part<'_>>); 4] = [
        (
            "/v1/pdf/stamp",
            vec![
                ("files", Some("a.pdf"), &pdf),
                ("text", None, long.as_bytes()),
            ],
        ),
        (
            "/v1/pdf/watermark",
            vec![
                ("files", Some("a.pdf"), &pdf),
                ("text", None, long.as_bytes()),
            ],
        ),
        (
            "/forms/pdfengines/stamp",
            vec![
                ("files", Some("a.pdf"), &pdf),
                ("stampSource", None, b"text"),
                ("stampExpression", None, long.as_bytes()),
            ],
        ),
        (
            "/forms/pdfengines/watermark",
            vec![
                ("files", Some("a.pdf"), &pdf),
                ("watermarkSource", None, b"text"),
                ("watermarkExpression", None, long.as_bytes()),
            ],
        ),
    ];
    for (path, parts) in cases {
        let res = service(config()).oneshot(post(path, &parts)).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{path}");
        assert!(text(res).await.contains("65536"), "{path}");
    }
}

// ---------------------------------------------------------------- F5

/// A receiver that takes connections and never answers, so a delivery holds its
/// job's place until `api_timeout`. Returns its port.
async fn silent_receiver() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    port
}

fn hooked(path: &str, parts: &[Part<'_>], port: u16, key: &str) -> Request<Body> {
    form(path, parts)
        .header("x-api-key", key)
        .header(
            "gotenberg-webhook-url",
            format!("http://127.0.0.1:{port}/d"),
        )
        .header(
            "gotenberg-webhook-error-url",
            format!("http://127.0.0.1:{port}/e"),
        )
        .body(Body::from(body_of(parts)))
        .unwrap()
}

#[tokio::test]
async fn webhook_jobs_are_limited_per_caller_and_in_all() {
    let port = silent_receiver().await;
    let mut c = config();
    c.webhook_allow_private = true;
    c.webhook_attempts = 1;
    c.api_keys = vec!["a".into(), "b".into(), "c".into()];
    c.max_async_jobs = 2;
    c.max_async_jobs_per_caller = 1;
    let state = AppState::new(c);
    let parts: [Part<'_>; 1] = [("template", Some("t.txt"), b"hello")];
    let send = |key: &str, path: &str| hooked(path, &parts, port, key);
    let first = app(state.clone())
        .oneshot(send("a", "/v1/render"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::NO_CONTENT);
    // Held through the delivery, which never gets an answer.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let second = app(state.clone())
        .oneshot(send("a", "/v1/render"))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    let other = app(state.clone())
        .oneshot(send("b", "/v1/render"))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::NO_CONTENT);
    let third = app(state.clone())
        .oneshot(send("c", "/v1/render"))
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS, "two in all");
    // The Gotenberg routes count against the same places.
    let pdf = fixtures::pages(1);
    let parts: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    let forms = app(state)
        .oneshot(hooked("/forms/pdfengines/flatten", &parts, port, "c"))
        .await
        .unwrap();
    assert_eq!(forms.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_jobs_place_comes_back_after_its_delivery() {
    let port = silent_receiver().await;
    let mut c = config();
    c.webhook_allow_private = true;
    c.webhook_attempts = 1;
    c.api_timeout = Duration::from_millis(300);
    c.max_async_jobs_per_caller = 1;
    let state = AppState::new(c);
    let parts: [Part<'_>; 1] = [("template", Some("t.txt"), b"hello")];
    let first = app(state.clone())
        .oneshot(hooked("/v1/render", &parts, port, "k"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::NO_CONTENT);
    tokio::time::sleep(Duration::from_millis(600)).await;
    let again = app(state)
        .oneshot(hooked("/v1/render", &parts, port, "k"))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::NO_CONTENT);
}

/// A service whose LibreOffice is a script: every conversion takes 300 ms.
fn with_slow_office(config: Config, dir: &tempfile::TempDir) -> AppState {
    let script = dir.path().join("bridge.py");
    std::fs::write(
        &script,
        r#"
import json, sys, time
print(json.dumps({"ready": True, "soffice_pid": None}), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    time.sleep(0.3)
    with open(req["output"], "w") as f:
        f.write("%PDF-1.4 fake")
    print(json.dumps({"id": req["id"], "ok": True, "pages": 1, "ms": 1}), flush=True)
"#,
    )
    .unwrap();
    let mut state = AppState::new(config);
    let office = LibreOffice::new("/nonexistent/soffice".into(), 1).with_bridge(script);
    let engines = Arc::new(Engines {
        libreoffice: Some(office),
        ..Engines::default()
    });
    state.pipeline = Pipeline::new(engines.clone(), state.config.api_timeout);
    state.engines = engines;
    state
}

#[tokio::test]
async fn a_multi_file_conversion_has_one_time_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.batch_timeout = Duration::from_secs(1);
    let state = with_slow_office(c, &dir);
    let parts: Vec<Part<'_>> = (0..20)
        .map(|_| ("files", Some("a.txt"), &b"x"[..]))
        .collect();
    let started = std::time::Instant::now();
    let res = app(state)
        .oneshot(post("/v1/convert", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_webhook_conversion_has_the_same_time_limit() {
    // Before the limit, this job ran all twenty files in the background.
    let dir = tempfile::tempdir().unwrap();
    let hits = Arc::new(Mutex::new(Vec::new()));
    let seen = hits.clone();
    let receiver = Router::new().fallback(move |body: String| {
        let seen = seen.clone();
        async move {
            seen.lock().unwrap().push(body);
            StatusCode::OK
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, receiver).await });
    let mut c = config();
    c.batch_timeout = Duration::from_secs(1);
    c.webhook_allow_private = true;
    let state = with_slow_office(c, &dir);
    let parts: Vec<Part<'_>> = (0..20)
        .map(|_| ("files", Some("a.txt"), &b"x"[..]))
        .collect();
    let res = app(state)
        .oneshot(hooked("/v1/convert", &parts, port, "k"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    let hits = hits.lock().unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].contains("\"status\":503"), "{hits:?}");
}

#[tokio::test]
async fn a_multi_file_conversion_may_outlast_the_request_limit() {
    // The route limit is the batch's, or this would be cut off at 6 s.
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.api_timeout = Duration::from_secs(1);
    let state = with_slow_office(c, &dir);
    let names: Vec<String> = (0..22).map(|i| format!("{i}.txt")).collect();
    let parts: Vec<Part<'_>> = names
        .iter()
        .map(|name| ("files", Some(name.as_str()), &b"x"[..]))
        .collect();
    let res = app(state)
        .oneshot(post("/v1/convert", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// ---------------------------------------------------------------- F7, F15

fn get(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).unwrap()
}

#[test]
fn every_start_warns_about_what_is_open() {
    let warnings = rusticdoc_server::startup_warnings(&config());
    assert!(warnings.iter().any(|w| w.contains("RUSTICDOC_API_KEYS")));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("RUSTICDOC_WEBHOOK_SECRET"))
    );
    let mut c = config();
    c.api_keys = vec!["k".into()];
    c.webhook_secret = Some("s".into());
    assert!(rusticdoc_server::startup_warnings(&c).is_empty());
    let mut c = config();
    c.compat.webhook_disabled = true;
    let warnings = rusticdoc_server::startup_warnings(&c);
    assert!(!warnings.iter().any(|w| w.contains("WEBHOOK_SECRET")));
}

fn guarded() -> Config {
    let mut c = config();
    c.api_keys = vec!["secret".into()];
    c
}

#[tokio::test]
async fn with_credentials_configured_little_is_open() {
    for path in ["/metrics", "/v1/version", "/version", "/v1/fonts"] {
        let res = service(guarded()).oneshot(get(path)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let keyed = |path: &str| {
        Request::get(path)
            .header("x-api-key", "secret")
            .body(Body::empty())
            .unwrap()
    };
    for path in ["/metrics", "/v1/version"] {
        let res = service(guarded()).oneshot(keyed(path)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
    }
    let mut c = guarded();
    c.metrics_public = true;
    let res = service(c).oneshot(get("/metrics")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn health_shows_only_its_status_without_credentials() {
    for path in ["/health", "/ready"] {
        let res = service(guarded()).oneshot(get(path)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().get("x-rusticdoc-version").is_none());
        assert_eq!(text(res).await, r#"{"status":"up"}"#, "{path}");
    }
    let keyed = Request::get("/health")
        .header("x-api-key", "secret")
        .body(Body::empty())
        .unwrap();
    let res = service(guarded()).oneshot(keyed).await.unwrap();
    assert!(text(res).await.contains("engines"));
    // With no credentials configured, health stays as Gotenberg's clients expect.
    let res = service(config()).oneshot(get("/health")).await.unwrap();
    let body = text(res).await;
    assert!(
        body.contains("details") && body.contains("libreoffice"),
        "{body}"
    );
}

#[tokio::test]
async fn docs_are_opt_in_and_limited_when_on() {
    let res = service(config()).oneshot(get("/docs")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let mut c = config();
    c.enable_docs = true;
    let res = service(c).oneshot(get("/docs")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let policy = res.headers().get("content-security-policy").unwrap();
    assert!(policy.to_str().unwrap().contains("default-src 'none'"));
}

#[tokio::test]
async fn every_response_says_nosniff() {
    for (path, c) in [
        ("/health", config()),
        ("/version", guarded()),
        ("/nowhere", config()),
    ] {
        let res = service(c).oneshot(get(path)).await.unwrap();
        assert_eq!(
            res.headers().get("x-content-type-options").unwrap(),
            "nosniff",
            "{path}"
        );
    }
}

// ---------------------------------------------------------------- F8

#[tokio::test]
async fn disabling_routes_holds_on_v1_too() {
    let pdf = fixtures::pages(1);
    let parts: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    let mut c = config();
    c.compat.pdfengines_disable_routes = true;
    for path in ["/v1/pdf/merge", "/v1/pdf/flatten", "/v1/pdf/forms/fill"] {
        let res = service(c.clone())
            .oneshot(post(path, &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let mut c = config();
    c.compat.libreoffice_disable_routes = true;
    for path in ["/v1/convert", "/v1/pdf/convert"] {
        let res = service(c.clone())
            .oneshot(post(path, &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
    }
    // A conversion step in a render refuses as well.
    let parts: [Part<'_>; 2] = [
        ("template", Some("t.txt"), b"x"),
        ("steps", None, b"convert"),
    ];
    let res = service(c)
        .oneshot(post("/v1/render", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn webhook_disable_holds_on_v1_too() {
    let receiver = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = receiver.local_addr().unwrap().port();
    let mut c = config();
    c.compat.webhook_disabled = true;
    c.webhook_allow_private = true;
    let parts: [Part<'_>; 1] = [("template", Some("t.txt"), b"hello")];
    let res = service(c)
        .oneshot(hooked("/v1/render", &parts, port, "k"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "the headers are ignored");
    assert_eq!(text(res).await, "hello");
    let accepted = tokio::time::timeout(Duration::from_millis(300), receiver.accept()).await;
    assert!(accepted.is_err(), "nothing was delivered");
}

#[tokio::test]
async fn url_lists_hold_on_every_route() {
    let parts: [Part<'_>; 1] = [("template", Some("t.txt"), b"hello")];
    let mut c = config();
    c.webhook_allow_private = true;
    c.compat.webhook_deny_list = vec!["/d$".into()];
    let res = service(c.clone())
        .oneshot(hooked("/v1/render", &parts, 1, "k"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let pdf = fixtures::pages(1);
    let files: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    let res = service(c)
        .oneshot(hooked("/forms/pdfengines/flatten", &files, 1, "k"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let mut c = config();
    c.download_from_allow_private = true;
    c.compat.download_deny_list = vec!["evil".into()];
    let parts: [Part<'_>; 1] = [(
        "downloadFrom",
        None,
        br#"[{"url":"http://127.0.0.1:1/evil"}]"#,
    )];
    let res = service(c)
        .oneshot(post("/v1/pdf/merge", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn debug_prints_the_real_lists() {
    let mut c = config();
    c.compat.enable_debug_route = true;
    let res = service(c.clone()).oneshot(get("/debug")).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["flags"]["webhook-allow-list"], "[.+]");
    c.compat.webhook_allow_list = vec!["^https://a".into()];
    c.compat.download_deny_list = vec!["b".into()];
    let res = service(c).oneshot(get("/debug")).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["flags"]["webhook-allow-list"], "[^https://a]");
    assert_eq!(body["flags"]["api-download-from-deny-list"], "[b]");
}

#[tokio::test]
async fn engine_failures_stay_out_of_v1_responses() {
    use rusticdoc_core::Error;
    use rusticdoc_server::error::ApiError;
    let stderr = "exit 2: qpdf: /tmp/rusticdoc-AbC/in.pdf: unable to find trailer";
    let cases = [
        Error::Engine {
            engine: "qpdf",
            message: stderr.into(),
        },
        Error::Io(std::io::Error::other(
            "/tmp/rusticdoc-AbC/out.pdf: no space",
        )),
    ];
    for error in cases {
        let res = axum::response::IntoResponse::into_response(ApiError::from(error));
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = text(res).await;
        assert!(!body.contains("/tmp") && !body.contains("qpdf"), "{body}");
        assert!(body.contains("Internal Server Error"), "{body}");
    }
    // A client's own mistake still says what it was.
    let res = axum::response::IntoResponse::into_response(ApiError::from(Error::InvalidRequest(
        "bad page".into(),
    )));
    assert!(text(res).await.contains("bad page"));
}
