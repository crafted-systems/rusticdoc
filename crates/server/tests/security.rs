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
    let cases: [(&str, Vec<Part<'_>>); 2] = [
        (
            "/v1/pdf/rotate",
            vec![
                ("files", Some("a.pdf"), &pdf),
                ("pages", None, spec.as_bytes()),
            ],
        ),
        (
            "/v1/pdf/split",
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
    let cases: [(&str, Vec<Part<'_>>); 2] = [
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
            "x-rusticdoc-webhook-url",
            format!("http://127.0.0.1:{port}/d"),
        )
        .header(
            "x-rusticdoc-webhook-error-url",
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
    // PDF routes count against the same places.
    let pdf = fixtures::pages(1);
    let parts: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    let flatten = app(state)
        .oneshot(hooked("/v1/pdf/flatten", &parts, port, "c"))
        .await
        .unwrap();
    assert_eq!(flatten.status(), StatusCode::TOO_MANY_REQUESTS);
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
    assert!(
        rusticdoc_server::startup_warnings(&c)
            .iter()
            .any(|w| w.contains("RUSTICDOC_RENDER_MODE")),
        "evaluating client templates in this process is worth saying at every start"
    );
    c.render_mode = rusticdoc_core::config::RenderMode::Isolated;
    assert!(rusticdoc_server::startup_warnings(&c).is_empty());
    let mut c = config();
    c.webhook_disabled = true;
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
    for path in ["/metrics", "/v1/version", "/v1/fonts"] {
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
    // With no credentials configured, health shows its details.
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
        ("/v1/version", guarded()),
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
    c.pdf_disabled = true;
    for path in ["/v1/pdf/merge", "/v1/pdf/flatten", "/v1/pdf/forms/fill"] {
        let res = service(c.clone())
            .oneshot(post(path, &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let mut c = config();
    c.convert_disabled = true;
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
    c.webhook_disabled = true;
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
    c.webhook_deny_list = vec!["/d$".into()];
    let res = service(c.clone())
        .oneshot(hooked("/v1/render", &parts, 1, "k"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let pdf = fixtures::pages(1);
    let files: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    let res = service(c)
        .oneshot(hooked("/v1/pdf/flatten", &files, 1, "k"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let mut c = config();
    c.download_from_allow_private = true;
    c.download_deny_list = vec!["evil".into()];
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
async fn each_caller_signs_with_its_own_key() {
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
    let pdf = fixtures::pages(1);
    let parts: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    let request = form("/v1/pdf/flatten", &parts)
        .header("x-api-key", "tenant-a")
        .header(
            "x-rusticdoc-webhook-url",
            format!("http://127.0.0.1:{port}/done"),
        )
        .body(Body::from(body_of(&parts)))
        .unwrap();
    let res = service(c).oneshot(request).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let key_a = webhook::signing_key("master", &auth::key_principal("tenant-a").0);
    let key_b = webhook::signing_key("master", &auth::key_principal("tenant-b").0);
    {
        let (path, headers, body) = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let value = |name: &str| headers.get(name).unwrap().to_str().unwrap().to_string();
        let timestamp: u64 = value(webhook::TIMESTAMP).parse().unwrap();
        let event = value(webhook::EVENT);
        let id = value("x-request-id");
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

// ---------------------------------------------------------------- F14

fn connection_limits(
    header_timeout: Duration,
    max_connections: usize,
) -> rusticdoc_server::ConnectionLimits {
    rusticdoc_server::ConnectionLimits {
        header_timeout,
        max_connections,
        max_connections_per_peer: 0,
        stall_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(300),
        body_timeout: Duration::from_secs(30),
    }
}

async fn running(
    limits: rusticdoc_server::ConnectionLimits,
) -> (
    u16,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    running_app(limits, service(config())).await
}

async fn running_app(
    limits: rusticdoc_server::ConnectionLimits,
    router: Router,
) -> (
    u16,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
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
    let limits = connection_limits(Duration::from_millis(200), 8);
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
    let limits = connection_limits(Duration::from_secs(1), 1);
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
    let limits = connection_limits(Duration::from_secs(1), 8);
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
    c.webhook_deny_list = vec!["^https://evil\\.example/".into()];
    c.download_deny_list = vec!["^https://evil\\.example/".into()];
    let pdf = fixtures::pages(1);
    let template: [Part<'_>; 1] = [("template", Some("t.txt"), b"hello")];
    let files: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    for url in hosts {
        // The result URL, on a render and on a PDF route.
        for (path, parts) in [
            ("/v1/render", &template[..]),
            ("/v1/pdf/flatten", &files[..]),
        ] {
            let request = form(path, parts)
                .header("x-rusticdoc-webhook-url", url)
                .body(Body::from(body_of(parts)))
                .unwrap();
            let res = service(c.clone()).oneshot(request).await.unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{path} {url}");
        }
        // The error URL, on /v1.
        let request = form("/v1/render", &template)
            .header("x-rusticdoc-webhook-url", "https://1.1.1.1/ok")
            .header("x-rusticdoc-webhook-error-url", url)
            .body(Body::from(body_of(&template)))
            .unwrap();
        let res = service(c.clone()).oneshot(request).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "error url {url}");
        // downloadFrom, which has its own lists.
        let entry = format!(r#"[{{"url":"{url}"}}]"#);
        let parts: [Part<'_>; 1] = [("downloadFrom", None, entry.as_bytes())];
        for path in ["/v1/pdf/merge", "/v1/convert"] {
            let res = service(c.clone())
                .oneshot(post(path, &parts))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{path} {url}");
        }
    }
}

// ---------------------------------------------------------------- F16, F17

#[tokio::test]
async fn colours_are_refused_where_overlays_are_built() {
    let pdf = fixtures::pages(1);
    for colour in [
        "#ff0000, url:example.com/x",
        "#ff0000, fontname:Courier",
        "red",
        "a\u{e9}123",
    ] {
        let parts: [Part<'_>; 3] = [
            ("files", Some("a.pdf"), &pdf),
            ("text", None, b"DRAFT"),
            ("colour", None, colour.as_bytes()),
        ];
        let res = service(config())
            .oneshot(post("/v1/pdf/stamp", &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{colour}");
    }
    let parts: [Part<'_>; 3] = [
        ("files", Some("a.pdf"), &pdf),
        ("text", None, b"DRAFT"),
        ("colour", None, b"#FF0000"),
    ];
    let res = service(config())
        .oneshot(post("/v1/pdf/stamp", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_password_with_a_line_break_is_a_400_before_any_engine() {
    let pdf = fixtures::pages(1);
    for path in ["/v1/pdf/encrypt"] {
        let parts: [Part<'_>; 2] = [
            ("files", Some("a.pdf"), &pdf),
            ("userPassword", None, b"a\nb"),
        ];
        let res = service(config()).oneshot(post(path, &parts)).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{path}");
        assert!(text(res).await.contains("line breaks"), "{path}");
    }
}

// ---------------------------------------------------------------- the allow-list

#[tokio::test]
async fn only_known_file_types_reach_libreoffice() {
    for name in [
        "evil.oxt",
        "macro.xba",
        "conf.xcu",
        "db.odb",
        "script.py",
        "noext",
    ] {
        let parts: [Part<'_>; 1] = [("files", Some(name), b"x")];
        let res = service(config())
            .oneshot(post("/v1/convert", &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{name}");
        let parts: [Part<'_>; 2] = [("template", Some(name), b"x"), ("steps", None, b"convert")];
        let res = service(config())
            .oneshot(post("/v1/render", &parts))
            .await
            .unwrap();
        assert_ne!(res.status(), StatusCode::OK, "{name}");
        assert_ne!(res.status(), StatusCode::SERVICE_UNAVAILABLE, "{name}");
    }
}

// ------------------------------------------------- connections that stop moving

async fn connect(port: u16) -> tokio::net::TcpStream {
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap()
}

/// True when a fresh connection gets an answer within `wait`, which it can only do
/// once the one place the service has is free again.
async fn served_within(port: u16, wait: Duration) -> bool {
    use tokio::io::AsyncWriteExt;
    let started = tokio::time::Instant::now();
    let mut stream = connect(port).await;
    stream.write_all(GET_HEALTH).await.unwrap();
    let mut out = Vec::new();
    let read = async {
        use tokio::io::AsyncReadExt;
        let _ = stream.read_to_end(&mut out).await;
    };
    let finished = tokio::time::timeout(wait, read).await.is_ok();
    finished && out.starts_with(b"HTTP/1.1 200") && started.elapsed() <= wait
}

fn one_place(
    stall: Duration,
    body: Duration,
    header: Duration,
) -> rusticdoc_server::ConnectionLimits {
    rusticdoc_server::ConnectionLimits {
        header_timeout: header,
        max_connections: 1,
        max_connections_per_peer: 0,
        stall_timeout: stall,
        write_timeout: LONG,
        body_timeout: body,
    }
}

const LONG: Duration = Duration::from_secs(60);

#[tokio::test]
async fn a_client_that_stops_reading_loses_its_place() {
    use tokio::io::AsyncWriteExt;
    let limits = one_place(Duration::from_millis(500), LONG, LONG);
    let (port, _stop, _task) = running(limits).await;
    // A small window, pipelined requests for a large open response, and no reads.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let stream = socket
        .connect(format!("127.0.0.1:{port}").parse().unwrap())
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    tokio::spawn(async move {
        let request = b"GET /openapi.json HTTP/1.1\r\nHost: x\r\n\r\n".repeat(2_000);
        let _ = writer.write_all(&request).await;
        tokio::time::sleep(LONG).await;
    });
    // Long enough for the writes to block, not to be refused at the first place.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        served_within(port, Duration::from_secs(15)).await,
        "the blocked connection kept the only place"
    );
    drop(reader);
}

#[tokio::test]
async fn a_body_that_stops_arriving_loses_its_place() {
    use tokio::io::AsyncWriteExt;
    let limits = one_place(Duration::from_millis(300), LONG, LONG);
    let (port, _stop, _task) = running(limits).await;
    let mut stream = connect(port).await;
    stream
        .write_all(
            format!(
                "POST /v1/pdf/merge HTTP/1.1\r\nHost: x\r\nContent-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: 100000\r\n\r\n--{BOUNDARY}\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    assert!(
        served_within(port, Duration::from_secs(10)).await,
        "the stalled upload kept the only place"
    );
}

#[tokio::test]
async fn a_body_that_only_drips_is_stopped_by_the_total_limit() {
    use tokio::io::AsyncWriteExt;
    let limits = one_place(Duration::from_millis(400), Duration::from_secs(1), LONG);
    let (port, _stop, _task) = running(limits).await;
    let mut stream = connect(port).await;
    stream
        .write_all(
            format!(
                "POST /v1/pdf/merge HTTP/1.1\r\nHost: x\r\nContent-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: 100000\r\n\r\n--{BOUNDARY}\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    tokio::spawn(async move {
        // Always moving, so the stall timer never fires.
        while stream.write_all(b"x").await.is_ok() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    assert!(
        served_within(port, Duration::from_secs(10)).await,
        "the dripping upload kept the only place"
    );
}

#[tokio::test]
async fn a_slow_body_that_keeps_moving_is_allowed() {
    use tokio::io::AsyncWriteExt;
    let limits = one_place(Duration::from_millis(500), Duration::from_secs(20), LONG);
    let (port, _stop, _task) = running(limits).await;
    let pdf = fixtures::pages(1);
    let parts: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
    let body = body_of(&parts);
    let mut stream = connect(port).await;
    stream
        .write_all(
            format!(
                "POST /v1/pdf/merge HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    // Slower in all than the stall limit, never slower than it between chunks.
    for chunk in body.chunks(body.len().div_ceil(8)) {
        stream.write_all(chunk).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(answer(&mut stream).await.starts_with(b"HTTP/1.1 200"));
}

#[tokio::test]
async fn a_silent_socket_loses_its_place_at_the_header_timeout() {
    let limits = one_place(LONG, LONG, Duration::from_millis(300));
    let (port, _stop, _task) = running(limits).await;
    let _silent = connect(port).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        served_within(port, Duration::from_secs(5)).await,
        "a socket that never sent a byte kept the only place"
    );
}

#[tokio::test]
async fn an_idle_keep_alive_socket_loses_its_place_at_the_header_timeout() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let limits = one_place(LONG, LONG, Duration::from_millis(300));
    let (port, _stop, _task) = running(limits).await;
    let mut idle = connect(port).await;
    idle.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut buf = [0u8; 4096];
    let read = idle.read(&mut buf).await.unwrap();
    assert!(buf[..read].starts_with(b"HTTP/1.1 200"));
    // Answered and kept alive, it now holds the place doing nothing.
    assert!(
        served_within(port, Duration::from_secs(5)).await,
        "an idle connection kept the only place"
    );
}

#[tokio::test]
async fn a_handler_that_takes_longer_than_the_stall_limit_still_finishes() {
    use tokio::io::AsyncWriteExt;
    let dir = tempfile::tempdir().unwrap();
    let state = with_slow_office(config(), &dir);
    // Four conversions at 300 ms each, with no bytes moving meanwhile.
    let limits = one_place(Duration::from_millis(200), LONG, LONG);
    let (port, _stop, _task) = running_app(limits, app(state)).await;
    let names = ["1.txt", "2.txt", "3.txt", "4.txt"];
    let parts: Vec<Part<'_>> = names
        .iter()
        .map(|n| ("files", Some(*n), &b"x"[..]))
        .collect();
    let body = body_of(&parts);
    let mut stream = connect(port).await;
    stream
        .write_all(
            format!(
                "POST /v1/convert HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.write_all(&body).await.unwrap();
    let started = tokio::time::Instant::now();
    let answered = answer(&mut stream).await;
    assert!(
        started.elapsed() > Duration::from_millis(600),
        "{:?}",
        started.elapsed()
    );
    assert!(
        answered.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&answered[..answered.len().min(200)])
    );
}

#[tokio::test]
async fn connections_per_peer_are_limited_when_asked() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut limits = connection_limits(Duration::from_secs(5), 8);
    limits.max_connections_per_peer = 1;
    let (port, _stop, _task) = running(limits).await;
    let mut first = connect(port).await;
    first
        .write_all(b"GET /health HTTP/1.1\r\nX-Slow: ")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut second = connect(port).await;
    let mut buf = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(2), second.read(&mut buf)).await;
    assert!(
        matches!(read, Ok(Ok(0)) | Ok(Err(_))),
        "a second connection from one address was served"
    );
    drop(first);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut third = connect(port).await;
    third.write_all(GET_HEALTH).await.unwrap();
    assert!(answer(&mut third).await.starts_with(b"HTTP/1.1 200"));
}

// ---------------------------------------------------------------- F19

#[tokio::test]
async fn a_named_proxy_changes_nothing_about_the_private_address_check() {
    // Nothing listens on the proxy: a refused URL must not even try it.
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let mut c = config();
    c.outbound_proxy = Some(proxy_url);
    let template: [Part<'_>; 1] = [("template", Some("t.txt"), b"hello")];
    for url in [
        "http://127.0.0.1:9/x",
        "http://localhost:9/x",
        "http://10.0.0.1/x",
    ] {
        let request = form("/v1/render", &template)
            .header("x-rusticdoc-webhook-url", url)
            .body(Body::from(body_of(&template)))
            .unwrap();
        let res = service(c.clone()).oneshot(request).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{url}");
        assert!(text(res).await.contains("private"), "{url}");
        let entry = format!(r#"[{{"url":"{url}"}}]"#);
        let parts: [Part<'_>; 1] = [("downloadFrom", None, entry.as_bytes())];
        let res = service(c.clone())
            .oneshot(post("/v1/pdf/merge", &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{url}");
        assert!(text(res).await.contains("private"), "{url}");
    }
    let accepted = tokio::time::timeout(Duration::from_millis(200), proxy.accept()).await;
    assert!(accepted.is_err(), "a refused URL reached the proxy");
    let warnings = rusticdoc_server::startup_warnings(&c);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("DNS pinning") && w.contains("private ranges")),
        "{warnings:?}"
    );
}

// ------------------------------------------------------- readers that barely move

/// A client that pipelines large responses and reads a little at a time, always
/// sooner than the stall limit.
async fn barely_reading(port: u16) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let stream = socket
        .connect(format!("127.0.0.1:{port}").parse().unwrap())
        .await
        .unwrap();
    let (mut reader, mut writer) = stream.into_split();
    tokio::spawn(async move {
        let request = b"GET /openapi.json HTTP/1.1\r\nHost: x\r\n\r\n".repeat(20_000);
        let _ = writer.write_all(&request).await;
        tokio::time::sleep(LONG).await;
    });
    tokio::spawn(async move {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    });
}

#[tokio::test]
async fn a_reader_that_takes_a_little_at_a_time_loses_its_place_at_the_total() {
    let limits = rusticdoc_server::ConnectionLimits {
        header_timeout: LONG,
        max_connections: 1,
        max_connections_per_peer: 0,
        stall_timeout: LONG,
        write_timeout: Duration::from_secs(2),
        body_timeout: LONG,
    };
    let (port, _stop, _task) = running(limits).await;
    barely_reading(port).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        served_within(port, Duration::from_secs(20)).await,
        "the slow reader kept the only place"
    );
}

/// A response of `size` bytes from a text template, through a real socket.
fn large_render(size: usize) -> Vec<u8> {
    let template = format!(r#"{{{{ "a" * {size} }}}}"#);
    let parts: [Part<'_>; 1] = [("template", Some("t.txt"), template.as_bytes())];
    let body = body_of(&parts);
    let mut request = format!(
        "POST /v1/render HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(&body);
    request
}

#[tokio::test]
async fn a_fast_large_download_is_not_affected() {
    use tokio::io::AsyncWriteExt;
    let limits = rusticdoc_server::ConnectionLimits {
        header_timeout: LONG,
        max_connections: 1,
        max_connections_per_peer: 0,
        stall_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
        body_timeout: LONG,
    };
    let (port, _stop, _task) = running(limits).await;
    let size = 40 * 1024 * 1024;
    let mut stream = connect(port).await;
    stream.write_all(&large_render(size)).await.unwrap();
    let out = answer(&mut stream).await;
    assert!(out.starts_with(b"HTTP/1.1 200"));
    assert!(out.len() > size, "{}", out.len());
}

#[tokio::test]
async fn a_keep_alive_client_is_not_affected() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Far less total than the requests below would use if it never started over.
    let limits = rusticdoc_server::ConnectionLimits {
        header_timeout: Duration::from_secs(5),
        max_connections: 1,
        max_connections_per_peer: 0,
        stall_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_millis(50),
        body_timeout: LONG,
    };
    let (port, _stop, _task) = running(limits).await;
    let mut stream = connect(port).await;
    let mut buf = vec![0u8; 256 * 1024];
    for i in 0..200 {
        stream
            .write_all(b"GET /openapi.json HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut seen = Vec::new();
        // Read to the end of this response: its headers name the length.
        let expected = loop {
            let read = stream.read(&mut buf).await.unwrap();
            assert!(read > 0, "the connection closed at request {i}");
            seen.extend_from_slice(&buf[..read]);
            if let Some(end) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&seen[..end]).to_ascii_lowercase();
                let length: usize = head
                    .split("content-length: ")
                    .nth(1)
                    .and_then(|rest| rest.split("\r\n").next())
                    .and_then(|v| v.parse().ok())
                    .unwrap();
                break end + 4 + length;
            }
        };
        while seen.len() < expected {
            let read = stream.read(&mut buf).await.unwrap();
            assert!(read > 0, "the connection closed at request {i}");
            seen.extend_from_slice(&buf[..read]);
        }
        assert!(seen.starts_with(b"HTTP/1.1 200"), "request {i}");
    }
}

/// One request declaring all of the budget, sent over a socket and left unfinished.
fn declaring(length: usize) -> Vec<u8> {
    format!(
        "POST /v1/pdf/flatten HTTP/1.1\r\nHost: x\r\nContent-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: {length}\r\n\r\n--{BOUNDARY}"
    )
    .into_bytes()
}

#[tokio::test]
async fn request_bodies_wait_for_room_in_the_budget() {
    use tokio::io::AsyncWriteExt;
    const MIB: usize = 1024 * 1024;
    let mut c = config();
    c.max_inflight_bytes = 2 * MIB;
    c.body_limit = 8 * MIB;
    c.api_timeout = Duration::from_millis(600);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = service(c);
    let limits = rusticdoc_server::ConnectionLimits {
        header_timeout: Duration::from_secs(5),
        max_connections: 16,
        max_connections_per_peer: 0,
        stall_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
        body_timeout: Duration::from_secs(30),
    };
    let task = tokio::spawn(rusticdoc_server::serve_on(
        listener,
        router,
        limits,
        std::future::pending::<()>(),
    ));

    // The first takes the whole budget and never finishes its body.
    let mut first = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    first.write_all(&declaring(2 * MIB)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The second waits, and is told so at the timeout, with its body unread.
    let started = std::time::Instant::now();
    let mut second = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    second.write_all(&declaring(2 * MIB)).await.unwrap();
    let answer = read_head(&mut second).await;
    assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
    assert!(
        answer.to_ascii_lowercase().contains("retry-after: 5"),
        "{answer}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "it did not wait"
    );

    // Routes without a body are not charged, whatever the budget says.
    let mut health = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    health
        .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let answer = read_head(&mut health).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");

    // With the first gone, there is room again.
    drop(first);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let mut third = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    third.write_all(&declaring(1)).await.unwrap();
    third.write_all(b"\r\n").await.unwrap();
    let answer = read_head(&mut third).await;
    assert!(!answer.starts_with("HTTP/1.1 503"), "{answer}");
    task.abort();
}

#[tokio::test]
async fn a_caller_without_credentials_does_not_join_the_queue() {
    const MIB: usize = 1024 * 1024;
    let mut c = config();
    c.api_keys = vec!["secret".to_string()];
    c.max_inflight_bytes = MIB;
    let state = AppState::new(c);
    // All of the budget is taken, so only the check before it can answer.
    let held = state.admission.clone();
    let _held = held.hold_all_for_test();
    let router = app(state);
    let parts: [Part<'_>; 1] = [("files", Some("a.pdf"), b"%PDF")];
    let started = std::time::Instant::now();
    let res = router
        .oneshot(post("/v1/pdf/flatten", &parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(started.elapsed() < Duration::from_secs(5));
}

async fn read_head(stream: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;
    let mut buffer = vec![0u8; 4096];
    let read = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buffer))
        .await
        .expect("no answer in time")
        .unwrap_or(0);
    String::from_utf8_lossy(&buffer[..read]).into_owned()
}

// ---------------------------------------------------------------- admins and tenants

/// A request to a route only an admin may use. Without a store an admin gets
/// 501 and anyone else 403, so the answer says which the caller was.
fn admin_only(auth: Option<(&str, String)>) -> Request<Body> {
    let mut req = Request::delete(format!("/v1/fonts/{}", "a".repeat(64)));
    if let Some((name, value)) = auth {
        req = req.header(name, value);
    }
    req.body(Body::empty()).unwrap()
}

fn basic(user: &str, pass: &str) -> Option<(&'static str, String)> {
    use base64::Engine as _;
    let pair = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
    Some(("authorization", format!("Basic {pair}")))
}

#[tokio::test]
async fn the_basic_auth_user_is_an_admin_only_when_the_operator_says() {
    let mut c = config();
    c.basic_auth = Some(("ada".into(), "pw".into()));
    let res = service(c.clone())
        .oneshot(admin_only(basic("ada", "pw")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "not by default");
    assert!(text(res).await.contains("RUSTICDOC_BASIC_AUTH_ADMIN"));

    c.basic_auth_admin = true;
    let res = service(c.clone())
        .oneshot(admin_only(basic("ada", "pw")))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_IMPLEMENTED,
        "past the admin check"
    );
    let res = service(c.clone())
        .oneshot(admin_only(basic("ada", "wrong")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // An API key beside it is not an admin for that.
    c.api_keys = vec!["caller".into()];
    let res = service(c)
        .oneshot(admin_only(Some(("x-api-key", "caller".into()))))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_settings_without_their_credentials_stop_the_service() {
    let from = |vars: &[(&str, &str)]| {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_vars(&move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    };
    for vars in [
        &[("RUSTICDOC_BASIC_AUTH_ADMIN", "true")][..],
        &[("RUSTICDOC_OIDC_ADMIN_SUBJECTS", "alice")],
        &[("RUSTICDOC_OIDC_ADMIN_CLAIM", "groups=admins")],
    ] {
        assert!(from(vars).check().is_err(), "{vars:?}");
    }
}

const OIDC_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgmJ9nJkaP9Rzg5Wma
6ZS+eBkD12USL+y5CHfhMXkuEcahRANCAASsdQ+QOJ1w4B4l7IUIUui+zWM2m4Zj
Yfo0bRmNQhP1WmUPaBpWQtZEqxmMlQWeJzftl7o5GzJCX1oJHh9aL/HF
-----END PRIVATE KEY-----
";
const OIDC_JWKS: &str = r#"{"keys":[{"kty":"EC","crv":"P-256","kid":"one","use":"sig","alg":"ES256",
    "x":"rHUPkDidcOAeJeyFCFLovs1jNpuGY2H6NG0ZjUIT9Vo",
    "y":"ZQ9oGlZC1kSrGYyVBZ4nN-2XujkbMkJfWgkeH1ov8cU"}]}"#;

/// A provider serving the keys, and a configuration that trusts it.
async fn oidc_config() -> Config {
    let provider = Router::new().route("/jwks", axum::routing::get(|| async { OIDC_JWKS }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, provider).await });
    let mut c = config();
    c.oidc = Some(rusticdoc_core::config::OidcConfig {
        issuer: "https://issuer.test/".into(),
        audience: "rusticdoc".into(),
        jwks_url: format!("http://127.0.0.1:{port}/jwks"),
    });
    c
}

fn token(claims: serde_json::Value) -> Option<(&'static str, String)> {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut all =
        serde_json::json!({"aud": "rusticdoc", "iss": "https://issuer.test/", "exp": now + 600});
    for (k, v) in claims.as_object().unwrap() {
        all[k] = v.clone();
    }
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("one".into());
    let token = encode(
        &header,
        &all,
        &EncodingKey::from_ec_pem(OIDC_KEY.as_bytes()).unwrap(),
    )
    .unwrap();
    Some(("authorization", format!("Bearer {token}")))
}

#[tokio::test]
async fn chosen_openid_connect_users_are_admins_and_others_are_not() {
    let mut c = oidc_config().await;
    let alice =
        serde_json::json!({"sub": "alice", "realm_access": {"roles": ["rusticdoc-admin", "user"]}});
    let bob = serde_json::json!({"sub": "bob", "realm_access": {"roles": ["user"]}});
    let carol = serde_json::json!({"sub": "carol", "groups": "rusticdoc-admin"});

    // Nobody, until the operator names someone.
    let res = service(c.clone())
        .oneshot(admin_only(token(alice.clone())))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    c.oidc_admin_claim = Some(("realm_access.roles".into(), "rusticdoc-admin".into()));
    c.oidc_admin_subjects = vec!["carol".into()];
    let app = service(c);
    let res = app.clone().oneshot(admin_only(token(alice))).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_IMPLEMENTED,
        "the claim holds the value"
    );
    let res = app.clone().oneshot(admin_only(token(bob))).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "the claim lacks it");
    let res = app.clone().oneshot(admin_only(token(carol))).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_IMPLEMENTED,
        "a listed subject"
    );
    // A token that does not verify is no credential, whatever it claims.
    let res = app
        .clone()
        .oneshot(admin_only(Some(("authorization", "Bearer forged".into()))))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_keys_still_work_beside_admin_users() {
    let mut c = config();
    c.api_keys = vec!["caller".into()];
    c.admin_keys = vec!["admin".into()];
    c.basic_auth = Some(("ada".into(), "pw".into()));
    c.basic_auth_admin = true;
    let app = service(c);
    for (auth, expected) in [
        (
            Some(("x-api-key", "admin".to_string())),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (basic("ada", "pw"), StatusCode::NOT_IMPLEMENTED),
        (
            Some(("x-api-key", "caller".to_string())),
            StatusCode::FORBIDDEN,
        ),
        (None, StatusCode::UNAUTHORIZED),
    ] {
        let res = app.clone().oneshot(admin_only(auth.clone())).await.unwrap();
        assert_eq!(res.status(), expected, "{auth:?}");
    }
}

#[tokio::test]
async fn every_key_of_a_tenant_is_one_caller_and_signs_with_one_key() {
    use rusticdoc_server::{auth, webhook};
    let mut c = config();
    c.api_keys = vec!["solo".into()];
    c.tenant_keys = vec![("acme".into(), "old".into()), ("acme".into(), "new".into())];
    let state = AppState::new(c.clone());
    let as_key = |key: &str| {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-api-key", key.parse().unwrap());
        headers
    };
    let old = auth::identify(&state, &as_key("old")).await.unwrap();
    let new = auth::identify(&state, &as_key("new")).await.unwrap();
    assert_eq!(old.0, "tenant:acme");
    assert_eq!(new.0, old.0);
    // An unnamed key keeps the principal it always had, so its data stays.
    let solo = auth::identify(&state, &as_key("solo")).await.unwrap();
    assert_eq!(solo.0, auth::key_principal("solo").0);
    assert!(auth::identify(&state, &as_key("acme")).await.is_none());

    // Deliveries for either key are signed with the tenant's key.
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let receiver = Router::new().fallback(move |request: axum::extract::Request| {
        let tx = tx.clone();
        async move {
            let (parts, body) = request.into_parts();
            let body = body.collect().await.unwrap().to_bytes();
            tx.send((parts.headers, body)).await.unwrap();
            StatusCode::OK
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, receiver).await });
    c.webhook_allow_private = true;
    c.webhook_secret = Some("master".into());
    let app = service(c);
    let tenant_key = webhook::signing_key("master", "tenant:acme");
    for key in ["old", "new"] {
        let pdf = fixtures::pages(1);
        let parts: [Part<'_>; 1] = [("files", Some("a.pdf"), &pdf)];
        let request = form("/v1/pdf/flatten", &parts)
            .header("x-api-key", key)
            .header(
                "x-rusticdoc-webhook-url",
                format!("http://127.0.0.1:{port}/done"),
            )
            .body(Body::from(body_of(&parts)))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let (headers, body) = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let value = |name: &str| headers.get(name).unwrap().to_str().unwrap().to_string();
        let timestamp: u64 = value(webhook::TIMESTAMP).parse().unwrap();
        assert_eq!(
            value(webhook::SIGNATURE),
            webhook::sign(
                &tenant_key,
                timestamp,
                &value(webhook::EVENT),
                &value("x-request-id"),
                &body
            ),
            "{key}"
        );
    }
}

#[tokio::test]
async fn tenant_keys_are_checked_before_the_service_starts() {
    let from = |vars: &[(&str, &str)]| {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_vars(&move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    };
    let good = from(&[("RUSTICDOC_TENANT_KEYS", "acme:one, acme:two,beta-2:three")]);
    assert!(good.check().is_ok());
    assert!(good.auth_configured());
    assert_eq!(good.tenant_keys.len(), 3);
    for bad in [
        &[("RUSTICDOC_TENANT_KEYS", "nameless")][..],
        &[("RUSTICDOC_TENANT_KEYS", "Acme:sekrit1")],
        &[("RUSTICDOC_TENANT_KEYS", "ac.me:sekrit1")],
        &[("RUSTICDOC_TENANT_KEYS", "acme:")],
        &[("RUSTICDOC_TENANT_KEYS", " , ")],
        &[("RUSTICDOC_TENANT_KEYS", "a:sekrit2,b:sekrit2")],
        &[
            ("RUSTICDOC_TENANT_KEYS", "a:sekrit2"),
            ("RUSTICDOC_API_KEYS", "sekrit2"),
        ],
    ] {
        let config = from(bad);
        let err = config.check().unwrap_err();
        // The message never repeats what may be a key.
        assert!(!err.contains("sekrit"), "{bad:?}: {err}");
    }
}
