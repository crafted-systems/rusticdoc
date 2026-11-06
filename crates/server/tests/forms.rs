//! Gotenberg's paths over the router, with no LibreOffice or process engines:
//! plain-text errors with its strings, its file naming, its settings. `/v1/*`
//! keeps JSON errors alongside them.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::routing::post;
use http_body_util::BodyExt;
use rusticdoc_core::Config;
use rusticdoc_core::pdfops::fixtures;
use rusticdoc_server::{AppState, app};
use tower::ServiceExt;

const BOUNDARY: &str = "rusticdoc-forms-boundary";

fn config() -> Config {
    Config {
        libreoffice_bin: None,
        qpdf_bin: None,
        pdfcpu_bin: None,
        log_format: "pretty".into(),
        ..Config::default()
    }
}

type Part<'a> = (&'a str, Option<&'a str>, &'a [u8]);

fn form(path: &str, parts: &[Part<'_>], headers: &[(&str, &str)]) -> Request<Body> {
    let mut body = Vec::new();
    for (name, filename, content) in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        let disposition = match filename {
            Some(f) => format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            ),
            None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
        };
        body.extend_from_slice(disposition.as_bytes());
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    let mut request = Request::post(path).header(
        header::CONTENT_TYPE,
        format!("multipart/form-data; boundary={BOUNDARY}"),
    );
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.body(Body::from(body)).unwrap()
}

async fn bytes_of(res: axum::response::Response) -> Vec<u8> {
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
}

async fn text(res: axum::response::Response) -> String {
    String::from_utf8(bytes_of(res).await).unwrap()
}

fn get(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).unwrap()
}

fn header_of<'a>(res: &'a axum::response::Response, name: &str) -> &'a str {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap())
        .unwrap_or("")
}

fn zip_names(bytes: Vec<u8>) -> Vec<String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn errors_are_plain_text_with_gotenbergs_strings_and_v1_stays_json() {
    let router = app(AppState::new(config()));

    let res = router
        .clone()
        .oneshot(form("/forms/pdfengines/flatten", &[], &[]))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(header_of(&res, "content-type"), "text/plain; charset=UTF-8");
    assert_eq!(
        text(res).await,
        "Invalid form data: no form file found for extensions: [.pdf]"
    );

    let res = router
        .clone()
        .oneshot(form("/forms/pdfengines/metadata/write", &[], &[]))
        .await
        .unwrap();
    assert_eq!(
        text(res).await,
        "Invalid form data: form field 'metadata' is required\nno form file found for extensions: [.pdf]"
    );

    let res = router
        .clone()
        .oneshot(form("/v1/pdf/flatten", &[], &[]))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(header_of(&res, "content-type"), "application/json");
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["error"], "invalid_request");

    let res = router
        .oneshot(
            Request::post("/forms/pdfengines/flatten")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        text(res).await,
        "Invalid 'Content-Type' header value: want 'multipart/form-data'"
    );
}

#[tokio::test]
async fn many_pdfs_come_back_as_a_zip_with_their_names() {
    let router = app(AppState::new(config()));
    let (one, two) = (fixtures::pages(1), fixtures::pages(1));

    let res = router
        .clone()
        .oneshot(form(
            "/forms/pdfengines/rotate",
            &[
                ("files", Some("page_1.pdf"), &one),
                ("files", Some("page_2.pdf"), &two),
                ("rotateAngle", None, b"90"),
            ],
            &[("Gotenberg-Output-Filename", "foo")],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header_of(&res, "content-type"), "application/zip");
    assert_eq!(
        header_of(&res, "content-disposition"),
        "attachment; filename=\"foo.zip\""
    );
    assert_eq!(zip_names(bytes_of(res).await), ["page_1.pdf", "page_2.pdf"]);

    // One PDF stays a PDF; a path in the requested name is dropped.
    let res = router
        .clone()
        .oneshot(form(
            "/forms/pdfengines/rotate",
            &[("files", Some("page_1.pdf"), &one), ("rotateAngle", None, b"90")],
            &[("Gotenberg-Output-Filename", "C:\\Windows\\Temp\\foo")],
        ))
        .await
        .unwrap();
    assert_eq!(header_of(&res, "content-type"), "application/pdf");
    assert_eq!(
        header_of(&res, "content-disposition"),
        "attachment; filename=\"foo.pdf\""
    );

    // Split parts are numbered from zero.
    let three = fixtures::pages(3);
    let res = router
        .oneshot(form(
            "/forms/pdfengines/split",
            &[
                ("files", Some("pages_3.pdf"), &three),
                ("splitMode", None, b"intervals"),
                ("splitSpan", None, b"2"),
            ],
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        zip_names(bytes_of(res).await),
        ["pages_3_0.pdf", "pages_3_1.pdf"]
    );
}

#[tokio::test]
async fn names_with_paths_stay_out_of_the_zip() {
    let router = app(AppState::new(config()));
    let one = fixtures::pages(1);
    let res = router
        .oneshot(form(
            "/forms/pdfengines/rotate",
            &[
                ("files", Some("../../evil.pdf"), &one),
                ("files", Some("/etc/ab.pdf"), &one),
                ("rotateAngle", None, b"90"),
            ],
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(zip_names(bytes_of(res).await), ["ab.pdf", "evil.pdf"]);
}

#[tokio::test]
async fn metadata_round_trips_in_exiftools_shapes() {
    let router = app(AppState::new(config()));
    let one = fixtures::pages(1);
    let metadata = br#"{"Title":"Sample","Marked":true,"CreateDate":"2006-09-18T16:27:50-04:00","Keywords":["first","second"]}"#;
    let res = router
        .clone()
        .oneshot(form(
            "/forms/pdfengines/metadata/write",
            &[("files", Some("a.pdf"), &one), ("metadata", None, metadata)],
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let written = bytes_of(res).await;
    let res = router
        .clone()
        .oneshot(form(
            "/forms/pdfengines/metadata/read",
            &[("files", Some("a.pdf"), &written)],
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(header_of(&res, "content-type"), "application/json");
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["a.pdf"]["Title"], "Sample");
    assert_eq!(body["a.pdf"]["Marked"], true);
    assert_eq!(body["a.pdf"]["CreateDate"], "2006:09:18 16:27:50-04:00");
    assert_eq!(
        body["a.pdf"]["Keywords"],
        serde_json::json!(["first", "second"])
    );

    let res = router
        .oneshot(form(
            "/forms/pdfengines/metadata/write",
            &[
                ("files", Some("a.pdf"), &one),
                ("metadata", None, br#"{"j":"/tmp/db.json"}"#),
            ],
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(text(res).await.contains("could not be written"));
}

#[tokio::test]
async fn bookmarks_take_a_list_or_a_map() {
    let router = app(AppState::new(config()));
    let one = fixtures::pages(1);
    let res = router
        .clone()
        .oneshot(form(
            "/forms/pdfengines/bookmarks/write",
            &[
                ("files", Some("page_1.pdf"), &one),
                (
                    "bookmarks",
                    None,
                    br#"{"page_1.pdf":[{"title":"Index","page":1}]}"#,
                ),
            ],
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let written = bytes_of(res).await;
    let res = router
        .clone()
        .oneshot(form(
            "/forms/pdfengines/bookmarks/read",
            &[("files", Some("page_1.pdf"), &written)],
            &[],
        ))
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["page_1.pdf"][0]["title"], "Index");

    let res = router
        .oneshot(form(
            "/forms/pdfengines/bookmarks/write",
            &[("files", Some("page_1.pdf"), &one), ("bookmarks", None, b"foo")],
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(
        text(res).await,
        "Invalid form data: form field 'bookmarks' is invalid (got 'foo', resulting to unmarshal bookmarks: invalid character 'o' in literal false (expecting 'a'))"
    );
}

#[tokio::test]
async fn embeds_and_their_relationships() {
    let router = app(AppState::new(config()));
    let one = fixtures::pages(1);
    let res = router
        .oneshot(form(
            "/forms/pdfengines/embed",
            &[
                ("files", Some("page_1.pdf"), &one),
                ("embeds", Some("embed_1.xml"), b"<a/>"),
                (
                    "embedsMetadata",
                    None,
                    br#"{"embed_1.xml":{"mimeType":"text/xml","relationship":"Alternative"}}"#,
                ),
            ],
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = bytes_of(res).await;
    let body = String::from_utf8_lossy(&bytes);
    assert!(body.contains("/AFRelationship/Alternative") || body.contains("/AFRelationship /Alternative"));
}

#[tokio::test]
async fn stamps_need_their_sources() {
    let router = app(AppState::new(config()));
    let one = fixtures::pages(1);
    for (parts, want) in [
        (
            vec![("files", Some("a.pdf"), one.as_slice())],
            "Invalid form data: form field 'stampSource' is required",
        ),
        (
            vec![
                ("files", Some("a.pdf"), one.as_slice()),
                ("stampSource", None, b"pdf".as_slice()),
                ("stampExpression", None, b"/etc/hostname".as_slice()),
            ],
            "Invalid form data: a stamp file is required for image or pdf source",
        ),
    ] {
        let res = router
            .clone()
            .oneshot(form("/forms/pdfengines/stamp", &parts, &[]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_eq!(text(res).await, want);
    }
    let res = router
        .oneshot(form(
            "/forms/pdfengines/stamp",
            &[
                ("files", Some("a.pdf"), &one),
                ("stampSource", None, b"text"),
                ("stampExpression", None, b"DRAFT"),
                ("stampOptions", None, br#"{"rotation":"0","position":"tl"}"#),
            ],
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn disabled_routes_are_not_there() {
    let mut c = config();
    c.compat.pdfengines_disable_routes = true;
    c.compat.libreoffice_disable_routes = true;
    let router = app(AppState::new(c));
    for path in ["/forms/pdfengines/merge", "/forms/libreoffice/convert"] {
        let res = router.clone().oneshot(form(path, &[], &[])).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn service_routes() {
    let mut c = config();
    c.compat.enable_debug_route = true;
    let router = app(AppState::new(c));

    let res = router.clone().oneshot(get("/")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header_of(&res, "content-type"), "text/html; charset=UTF-8");

    let res = router.clone().oneshot(get("/version")).await.unwrap();
    assert_eq!(header_of(&res, "content-type"), "text/plain; charset=UTF-8");
    assert_eq!(
        header_of(&res, "gotenberg-version"),
        rusticdoc_core::VERSION
    );
    assert_eq!(text(res).await, rusticdoc_core::VERSION);

    let res = router.clone().oneshot(get("/v1/version")).await.unwrap();
    assert_eq!(header_of(&res, "content-type"), "application/json");

    let res = router.clone().oneshot(get("/health")).await.unwrap();
    assert_eq!(
        header_of(&res, "content-type"),
        "application/json; charset=utf-8"
    );

    let res = router.clone().oneshot(get("/debug")).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["version"], rusticdoc_core::VERSION);
    assert_eq!(body["flags"]["api-enable-debug-route"], "true");

    let res = router.oneshot(get("/prometheus/metrics")).await.unwrap();
    assert_eq!(
        header_of(&res, "content-type"),
        "text/plain; version=0.0.4; charset=utf-8; escaping=underscores"
    );
    let body = text(res).await;
    assert!(body.contains("gotenberg_libreoffice_requests_queue_size 0"));
    assert!(body.contains("gotenberg_libreoffice_restarts_count 0"));
}

#[tokio::test]
async fn metrics_settings() {
    let mut c = config();
    c.compat.prometheus_namespace = "foo".into();
    c.compat.prometheus_path = "/custom/metrics".into();
    let router = app(AppState::new(c));
    let res = router.clone().oneshot(get("/custom/metrics")).await.unwrap();
    assert!(text(res).await.contains("foo_libreoffice_restarts_count"));
    let res = router.oneshot(get("/prometheus/metrics")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let mut c = config();
    c.compat.prometheus_disabled = true;
    let router = app(AppState::new(c));
    let res = router.oneshot(get("/prometheus/metrics")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn auth_covers_gotenbergs_routes_but_not_health() {
    let mut c = config();
    c.basic_auth = Some(("foo".into(), "bar".into()));
    c.compat.enable_debug_route = true;
    let router = app(AppState::new(c));
    for path in [
        "/",
        "/favicon.ico",
        "/version",
        "/debug",
        "/prometheus/metrics",
    ] {
        let res = router.clone().oneshot(get(path)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(header_of(&res, "gotenberg-version"), "", "{path}");
    }
    let res = router
        .clone()
        .oneshot(form("/forms/pdfengines/merge", &[], &[]))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(header_of(&res, "content-type"), "text/plain; charset=UTF-8");
    let res = router.oneshot(get("/health")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn root_path_covers_everything_with_or_without_the_slash() {
    let mut c = config();
    c.root_path = "/foo/".into();
    let router = app(AppState::new(c));
    for path in ["/foo", "/foo/", "/foo/health", "/foo/version", "/foo/favicon.ico"] {
        let res = router.clone().oneshot(get(path)).await.unwrap();
        assert!(res.status().is_success(), "{path}: {}", res.status());
    }
    let res = router.oneshot(get("/health")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn trace_headers_are_echoed_and_cleaned() {
    let router = app(AppState::new(config()));
    let res = router
        .clone()
        .oneshot(
            Request::get("/version")
                .header("Gotenberg-Trace", "abc-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(header_of(&res, "gotenberg-trace"), "abc-123");

    let long = "x".repeat(4000);
    let res = router
        .oneshot(
            Request::get("/version")
                .header("Gotenberg-Trace", &long)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(header_of(&res, "gotenberg-trace").len() <= 128);
}

#[tokio::test]
async fn webhooks_deliver_with_gotenbergs_headers_and_events() {
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(String, axum::http::HeaderMap, Vec<u8>)>>>;
    let seen: Seen = Arc::default();
    let record = |path: &'static str, seen: Seen| {
        post(move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
            let seen = seen.clone();
            async move {
                seen.lock()
                    .unwrap()
                    .push((path.to_string(), headers, body.to_vec()));
                StatusCode::OK
            }
        })
    };
    let receiver = Router::new()
        .route("/hook", record("hook", seen.clone()))
        .route("/events", record("events", seen.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, receiver).await });

    let mut c = config();
    c.webhook_allow_private = true;
    c.compat.webhook_sync = true;
    let router = app(AppState::new(c));
    let one = fixtures::pages(1);
    let (hook, events) = (
        format!("http://127.0.0.1:{port}/hook"),
        format!("http://127.0.0.1:{port}/events"),
    );

    let res = router
        .clone()
        .oneshot(form(
            "/forms/pdfengines/flatten",
            &[("files", Some("page_1.pdf"), &one)],
            &[
                ("Gotenberg-Webhook-Url", &hook),
                ("Gotenberg-Webhook-Events-Url", &events),
                ("Gotenberg-Trace", "trace-1"),
                (
                    "traceparent",
                    "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
                ),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    {
        let seen = seen.lock().unwrap();
        let (_, headers, body) = seen.iter().find(|(p, _, _)| p == "hook").unwrap();
        assert_eq!(headers["content-type"], "application/pdf");
        assert_eq!(headers["gotenberg-trace"], "trace-1");
        assert_eq!(headers["gotenberg-version"], rusticdoc_core::VERSION);
        assert!(
            headers["traceparent"]
                .to_str()
                .unwrap()
                .contains("0af7651916cd43dd8448eb211c80319c")
        );
        assert_eq!(
            headers["content-disposition"],
            "attachment; filename=\"page_1.pdf\""
        );
        assert!(body.starts_with(b"%PDF"));
        let (_, _, event) = seen.iter().find(|(p, _, _)| p == "events").unwrap();
        let event: serde_json::Value = serde_json::from_slice(event).unwrap();
        assert_eq!(event["event"], "webhook.success");
        assert_eq!(event["correlationId"], "trace-1");
    }

    // A result URL alone is not enough.
    let res = router
        .clone()
        .oneshot(form(
            "/forms/pdfengines/flatten",
            &[("files", Some("page_1.pdf"), &one)],
            &[("Gotenberg-Webhook-Url", &hook)],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // A read has no file to deliver: the failure goes to the error URL.
    seen.lock().unwrap().clear();
    let res = router
        .oneshot(form(
            "/forms/pdfengines/metadata/read",
            &[("files", Some("page_1.pdf"), &one)],
            &[
                ("Gotenberg-Webhook-Url", &hook),
                ("Gotenberg-Webhook-Error-Url", &events),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let seen = seen.lock().unwrap();
    let (_, headers, body) = &seen[0];
    assert_eq!(headers["content-type"], "application/json");
    let body: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(body["status"], 400);
    assert_eq!(
        body["message"],
        "The webhook middleware can only work with multipart/form-data routes that results in output files"
    );
}

#[tokio::test]
async fn webhooks_to_private_addresses_stay_refused_by_default() {
    let router = app(AppState::new(config()));
    let one = fixtures::pages(1);
    let res = router
        .oneshot(form(
            "/forms/pdfengines/flatten",
            &[("files", Some("page_1.pdf"), &one)],
            &[
                ("Gotenberg-Webhook-Url", "http://127.0.0.1:1/hook"),
                ("Gotenberg-Webhook-Error-Url", "http://127.0.0.1:1/error"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn webhook_header_reservations_stay() {
    let mut c = config();
    c.webhook_allow_private = true;
    let router = app(AppState::new(c));
    let one = fixtures::pages(1);
    let res = router
        .oneshot(form(
            "/forms/pdfengines/flatten",
            &[("files", Some("page_1.pdf"), &one)],
            &[
                ("Gotenberg-Webhook-Url", "http://127.0.0.1:1/hook"),
                ("Gotenberg-Webhook-Error-Url", "http://127.0.0.1:1/error"),
                (
                    "Gotenberg-Webhook-Extra-Http-Headers",
                    r#"{"Content-Disposition":"inline"}"#,
                ),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
