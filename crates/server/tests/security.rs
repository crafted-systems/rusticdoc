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

// ---------------------------------------------------------------- F9

#[tokio::test]
async fn each_caller_signs_with_its_own_key_and_events_are_signed() {
    use rusticdoc_server::{auth, webhook};
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let receiver = Router::new().fallback(move |request: axum::extract::Request| {
        let tx = tx.clone();
        async move {
            let (parts, body) = request.into_parts();
            let body = body.collect().await.unwrap().to_bytes();
            tx.send((parts.uri.path().to_string(), parts.headers, body))
                .await
                .unwrap();
            StatusCode::OK
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, receiver).await });

    let mut c = config();
    c.webhook_allow_private = true;
    c.webhook_secret = Some("master".into());
    c.api_keys = vec!["tenant-a".into(), "tenant-b".into()];
    let parts: [Part<'_>; 1] = [("files", Some("a.pdf"), &[][..])];
    let pdf = fixtures::pages(1);
    let _ = parts;
    let parts: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    let request = form("/forms/pdfengines/flatten", &parts)
        .header("x-api-key", "tenant-a")
        .header(
            "gotenberg-webhook-url",
            format!("http://127.0.0.1:{port}/done"),
        )
        .header(
            "gotenberg-webhook-events-url",
            format!("http://127.0.0.1:{port}/events"),
        )
        .body(Body::from(body_of(&parts)))
        .unwrap();
    let res = service(c).oneshot(request).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let key_a = webhook::signing_key("master", &auth::key_principal("tenant-a").0);
    let key_b = webhook::signing_key("master", &auth::key_principal("tenant-b").0);
    for _ in 0..2 {
        let (path, headers, body) = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let value = |name: &str| headers.get(name).unwrap().to_str().unwrap().to_string();
        let timestamp: u64 = value(webhook::TIMESTAMP).parse().unwrap();
        let event = value(webhook::EVENT);
        let id = value("gotenberg-trace");
        let signature = value(webhook::SIGNATURE);
        assert_eq!(
            signature,
            webhook::sign(&key_a, timestamp, &event, &id, &body),
            "{path}"
        );
        assert_ne!(
            signature,
            webhook::sign(&key_b, timestamp, &event, &id, &body)
        );
        assert_ne!(
            signature,
            webhook::sign(b"master", timestamp, &event, &id, &body)
        );
    }
}

// ---------------------------------------------------------------- F10

#[tokio::test]
async fn output_names_cannot_carry_a_path() {
    let pdf = fixtures::pages(2);
    let parts: [Part<'_>; 4] = [
        ("files", Some("a.pdf"), &pdf),
        ("splitMode", None, b"pages"),
        ("splitSpan", None, b"1,2"),
        ("outputFilename", None, b"../../../../tmp/evil"),
    ];
    let res = service(config())
        .oneshot(post("/v1/pdf/split", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("content-disposition").unwrap(),
        "attachment; filename=\"evil.zip\""
    );
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let names: Vec<String> = (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().unwrap().to_string())
        .collect();
    assert_eq!(names, ["evil_1.pdf", "evil_2.pdf"]);
    let parts: [Part<'_>; 2] = [
        ("files", Some("a.pdf"), &pdf),
        ("outputFilename", None, b"../../evil"),
    ];
    for path in ["/v1/pdf/flatten", "/v1/pdf/merge"] {
        let res = service(config()).oneshot(post(path, &parts)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
        assert_eq!(
            res.headers().get("content-disposition").unwrap(),
            "attachment; filename=\"evil.pdf\"",
            "{path}"
        );
    }
}

// ---------------------------------------------------------------- F11

#[tokio::test]
async fn download_failures_do_not_carry_the_query() {
    let mut c = config();
    c.download_from_allow_private = true;
    let parts: [Part<'_>; 1] = [(
        "downloadFrom",
        None,
        br#"[{"url":"http://127.0.0.1:1/file.pdf?token=SECRET123"}]"#,
    )];
    let res = service(c)
        .oneshot(post("/v1/pdf/merge", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(!text(res).await.contains("SECRET123"));
    let parts: [Part<'_>; 1] = [(
        "downloadFrom",
        None,
        br#"[{"url":"http://user:pw@example.com/x"}]"#,
    )];
    let res = service(config())
        .oneshot(post("/v1/pdf/merge", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = text(res).await;
    assert!(!body.contains("pw"), "{body}");
}

// ---------------------------------------------------------------- F13

/// Collects what is logged on this thread while `f` runs.
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

fn capture() -> (Captured, tracing::subscriber::DefaultGuard) {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::INFO)
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .finish();
    (
        captured.clone(),
        tracing::subscriber::set_default(subscriber),
    )
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
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
    let (logs, _guard) = capture();
    let request = Request::get("/v1/fonts")
        .header("x-api-key", "wrong-key-value")
        .body(Body::empty())
        .unwrap();
    let res = service(guarded())
        .oneshot(from_peer(request))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let logged = logs.text();
    assert!(logged.contains("authentication failed"), "{logged}");
    assert!(logged.contains("203.0.113.9:4242"), "{logged}");
    assert!(logged.contains("key:"), "{logged}");
    assert!(!logged.contains("wrong-key-value"), "{logged}");
}

#[tokio::test]
async fn the_access_log_names_the_peer_and_the_caller() {
    let (logs, _guard) = capture();
    let request = Request::get("/v1/fonts")
        .header("x-api-key", "secret")
        .body(Body::empty())
        .unwrap();
    let res = service(guarded())
        .oneshot(from_peer(request))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let logged = logs.text();
    assert!(
        logged.contains(r#""remote_addr":"203.0.113.9:4242""#),
        "{logged}"
    );
    assert!(logged.contains(r#""caller":"key:"#), "{logged}");
    assert!(!logged.contains(r#""secret""#), "{logged}");
}

#[tokio::test]
async fn webhook_job_failures_are_logged_in_the_requests_span() {
    let (logs, _guard) = capture();
    let port = silent_receiver().await;
    let mut c = config();
    c.webhook_allow_private = true;
    c.webhook_attempts = 1;
    c.api_timeout = Duration::from_millis(200);
    let parts: [Part<'_>; 1] = [("template", Some("t.txt"), b"{{ nobody.home }}")];
    let request = form("/v1/render", &parts)
        .header("x-request-id", "job-under-test")
        .header(
            "x-rusticdoc-webhook-url",
            format!("http://127.0.0.1:{port}/d"),
        )
        .body(Body::from(body_of(&parts)))
        .unwrap();
    let res = service(c).oneshot(request).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let logged = logs.text();
    let line = logged
        .lines()
        .find(|line| line.contains("template_error"))
        .unwrap_or_else(|| panic!("the job's failure was not logged: {logged}"));
    assert!(line.contains("job-under-test"), "{line}");
}

// ---------------------------------------------------------------- F14

async fn running(
    limits: rusticdoc_server::ConnectionLimits,
) -> (
    u16,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let router = service(config());
    let task = tokio::spawn(async move {
        rusticdoc_server::serve_on(listener, router, limits, async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    (port, stop, task)
}

const GET_HEALTH: &[u8] = b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";

async fn answer(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await;
    out
}

#[tokio::test]
async fn a_client_that_stalls_in_its_headers_is_dropped() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let limits = rusticdoc_server::ConnectionLimits {
        header_timeout: Duration::from_millis(200),
        max_connections: 8,
    };
    let (port, _stop, _task) = running(limits).await;
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nX-Slow: ")
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await;
    assert!(read.is_ok(), "the connection was left open");
    // A client that sends its headers is served as usual.
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream.write_all(GET_HEALTH).await.unwrap();
    assert!(answer(&mut stream).await.starts_with(b"HTTP/1.1 200"));
}

#[tokio::test]
async fn connections_past_the_limit_wait() {
    use tokio::io::AsyncWriteExt;
    let limits = rusticdoc_server::ConnectionLimits {
        header_timeout: Duration::from_secs(1),
        max_connections: 1,
    };
    let (port, _stop, _task) = running(limits).await;
    let mut first = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    first
        .write_all(b"GET /health HTTP/1.1\r\nX-Slow: ")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut second = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    second.write_all(GET_HEALTH).await.unwrap();
    let mut peek = [0u8; 1];
    let early = tokio::time::timeout(Duration::from_millis(300), second.peek(&mut peek)).await;
    assert!(
        early.is_err(),
        "the second connection was served past the limit"
    );
    // The header timeout frees the place, and then it is served.
    assert!(answer(&mut second).await.starts_with(b"HTTP/1.1 200"));
}

#[tokio::test]
async fn shutdown_lets_requests_finish_and_then_returns() {
    let limits = rusticdoc_server::ConnectionLimits {
        header_timeout: Duration::from_secs(1),
        max_connections: 8,
    };
    let (port, stop, task) = running(limits).await;
    let mut idle = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the server did not stop")
        .unwrap();
    assert!(answer(&mut idle).await.is_empty());
}

// ---------------------------------------------------------------- deny lists

/// Every way a webhook URL reaches the service, in the spellings a deny pattern on
/// the canonical URL must still catch.
#[tokio::test]
async fn deny_lists_catch_a_trailing_dot_and_an_uppercase_host_on_every_path() {
    let hosts = ["https://evil.example./x", "https://EVIL.example/x"];
    let mut c = config();
    c.compat.webhook_deny_list = vec!["^https://evil\\.example/".into()];
    c.compat.download_deny_list = vec!["^https://evil\\.example/".into()];
    let pdf = fixtures::pages(1);
    let template: [Part<'_>; 1] = [("template", Some("t.txt"), b"hello")];
    let files: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    for url in hosts {
        // /v1, under both header names.
        for header_name in ["x-rusticdoc-webhook-url", "gotenberg-webhook-url"] {
            let request = form("/v1/render", &template)
                .header(header_name, url)
                .body(Body::from(body_of(&template)))
                .unwrap();
            let res = service(c.clone()).oneshot(request).await.unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{header_name} {url}");
        }
        // The error URL, on /v1.
        let request = form("/v1/render", &template)
            .header("x-rusticdoc-webhook-url", "https://1.1.1.1/ok")
            .header("x-rusticdoc-webhook-error-url", url)
            .body(Body::from(body_of(&template)))
            .unwrap();
        let res = service(c.clone()).oneshot(request).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "error url {url}");
        // The result, error and events URLs on /forms.
        for (result, error, events) in [
            (url, "https://1.1.1.1/e", "https://1.1.1.1/v"),
            ("https://1.1.1.1/r", url, "https://1.1.1.1/v"),
            ("https://1.1.1.1/r", "https://1.1.1.1/e", url),
        ] {
            let request = form("/forms/pdfengines/flatten", &files)
                .header("gotenberg-webhook-url", result)
                .header("gotenberg-webhook-error-url", error)
                .header("gotenberg-webhook-events-url", events)
                .body(Body::from(body_of(&files)))
                .unwrap();
            let res = service(c.clone()).oneshot(request).await.unwrap();
            assert_eq!(
                res.status(),
                StatusCode::FORBIDDEN,
                "{result} {error} {events}"
            );
        }
        // downloadFrom, which has its own lists.
        let entry = format!(r#"[{{"url":"{url}"}}]"#);
        let parts: [Part<'_>; 1] = [("downloadFrom", None, entry.as_bytes())];
        for path in ["/v1/pdf/merge", "/forms/pdfengines/merge"] {
            let res = service(c.clone())
                .oneshot(post(path, &parts))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{path} {url}");
        }
    }
}
