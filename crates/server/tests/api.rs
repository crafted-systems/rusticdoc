//! End-to-end tests over the router, with no LibreOffice or qpdf present:
//! text templates render, errors are JSON, auth gates the API, and a webhook
//! delivery arrives signed at a local receiver.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::routing::post;
use http_body_util::BodyExt;
use rusticdoc_core::Config;
use rusticdoc_server::{AppState, app, webhook};
use tower::ServiceExt;

const BOUNDARY: &str = "rusticdoc-test-boundary";

fn config() -> Config {
    Config {
        libreoffice_bin: None,
        qpdf_bin: None,
        log_format: "pretty".into(),
        ..Config::default()
    }
}

/// Builds a multipart body from (name, filename, content) parts.
fn multipart(parts: &[(&str, Option<&str>, &str)]) -> Body {
    let mut body = String::new();
    for (name, filename, content) in parts {
        body.push_str(&format!("--{BOUNDARY}\r\n"));
        match filename {
            Some(f) => body.push_str(&format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )),
            None => body.push_str(&format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n")),
        }
        body.push_str(content);
        body.push_str("\r\n");
    }
    body.push_str(&format!("--{BOUNDARY}--\r\n"));
    Body::from(body)
}

fn post_form(path: &str, parts: &[(&str, Option<&str>, &str)]) -> Request<Body> {
    Request::post(path)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(multipart(parts))
        .unwrap()
}

async fn text(res: axum::response::Response) -> String {
    String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
}

#[tokio::test]
async fn health_version_metrics_and_openapi_are_open() {
    let router = app(AppState::new(config()));

    let res = router
        .clone()
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers().get("x-request-id").is_some(),
        "request id missing"
    );
    assert_eq!(
        res.headers().get("x-rusticdoc-version").unwrap(),
        rusticdoc_core::VERSION
    );
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["status"], "up");
    assert_eq!(body["engines"]["libreoffice"], false);

    let res = router
        .clone()
        .oneshot(Request::get("/v1/version").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["version"], rusticdoc_core::VERSION);

    let res = router
        .clone()
        .oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let spec: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert!(spec["paths"]["/v1/render"]["post"].is_object());
    assert!(spec["paths"]["/v1/convert"]["post"].is_object());
    assert!(spec["components"]["securitySchemes"]["api_key"].is_object());

    let res = router
        .clone()
        .oneshot(Request::get("/docs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "docs are opt-in");

    let res = router
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = text(res).await;
    assert!(body.contains("rusticdoc_requests"));
    assert!(
        body.contains("rusticdoc_template_cache_hits_total"),
        "{body}"
    );
    assert!(
        body.contains("rusticdoc_template_cache_misses_total"),
        "{body}"
    );
}

#[tokio::test]
async fn data_sent_as_a_file_fills_the_template_in_any_order() {
    let template = (
        "template",
        Some("letter.txt"),
        "Dear {{ customer.name }} of {{ ctx.company }}",
    );
    let data = (
        "data",
        Some("invoice.json"),
        r#"{"customer":{"name":"Ada"}}"#,
    );
    let ctx = ("ctx", Some("company.json"), r#"{"company":"Crafted"}"#);
    for parts in [
        [template, data, ctx],
        [data, ctx, template],
        [ctx, template, data],
    ] {
        let res = app(AppState::new(config()))
            .oneshot(post_form("/v1/render", &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(text(res).await, "Dear Ada of Crafted");
    }
}

#[tokio::test]
async fn data_sent_as_a_file_reaches_batch_and_validate() {
    let res = app(AppState::new(config()))
        .oneshot(post_form(
            "/v1/render/batch",
            &[
                (
                    "data",
                    Some("people.json"),
                    r#"{"people":[{"name":"Ada"},{"name":"Bob"}]}"#,
                ),
                ("template", Some("letter.txt"), "Dear {{ item.name }}"),
                ("splitBy", None, "people"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/zip"
    );

    let res = app(AppState::new(config()))
        .oneshot(post_form(
            "/v1/templates/validate",
            &[
                ("data", Some("sample.json"), r#"{"name":"Ada"}"#),
                ("template", Some("letter.txt"), "Dear {{ name }}"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = text(res).await;
    assert!(body.contains("\"ok\":true"), "{body}");
    assert!(
        body.contains("\"format\":\"txt\""),
        "the template, not the data: {body}"
    );
}

#[tokio::test]
async fn data_that_cannot_be_read_is_refused_and_not_ignored() {
    let template = ("template", Some("letter.txt"), "Hello");
    for (parts, expected) in [
        (
            [("data", Some("notes.txt"), "not json"), template],
            "notes.txt",
        ),
        (
            [("data", Some("a.json"), "{}"), ("data", None, "{}")],
            "more than once",
        ),
    ] {
        let res = app(AppState::new(config()))
            .oneshot(post_form("/v1/render", &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = text(res).await;
        assert!(body.contains(expected), "{body}");
    }
}

#[tokio::test]
async fn html_templates_escape_values_over_the_api() {
    let res = app(AppState::new(config()))
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("page.html"), "<p>{{ name }}</p>"),
                ("data", None, r#"{"name":"<script>alert(1)</script>"}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = text(res).await;
    assert!(!body.contains("<script>"), "{body}");
    assert!(body.contains("&lt;script&gt;"), "{body}");
}

#[tokio::test]
async fn renders_a_text_template_with_data_and_ctx() {
    let router = app(AppState::new(config()));
    let res = router
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("letter.txt"), "Dear {{ customer.name }}, from {{ ctx.company }}: {{ total | currency('KES') }}"),
                ("data", None, r#"{"customer":{"name":"Ada"},"total":1500}"#),
                ("ctx", None, r#"{"company":"Crafted"}"#),
                ("outputFilename", None, "greeting"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/plain; charset=utf-8"
    );
    assert_eq!(
        res.headers().get(header::CONTENT_DISPOSITION).unwrap(),
        "attachment; filename=\"greeting.txt\""
    );
    assert!(
        res.headers()
            .get("server-timing")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("template;dur=")
    );
    assert_eq!(text(res).await, "Dear Ada, from Crafted: KES 1,500.00");
}

#[tokio::test]
async fn errors_are_json_with_a_kind() {
    let router = app(AppState::new(config()));

    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("a.txt"), "x"),
                ("steps", None, "teleport"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["error"], "invalid_request");
    assert!(body["message"].as_str().unwrap().contains("teleport"));

    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/render",
            &[("template", Some("a.txt"), "{{ missing }}")],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("a.docx"), "PK"),
                ("data", None, r#"{"a":1}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY); // not a real DOCX: a template error

    // A convert with no LibreOffice configured is a 503, not a crash.
    let res = router
        .oneshot(post_form("/v1/convert", &[("files", Some("a.docx"), "PK")]))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["error"], "engine_unavailable");
}

#[tokio::test]
async fn validate_reports_without_rendering_a_file() {
    let router = app(AppState::new(config()));
    let res = router
        .oneshot(post_form(
            "/v1/templates/validate",
            &[
                ("template", Some("a.md"), "# {{ title }} {{ nope }}"),
                ("data", None, r#"{"title":"T"}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["format"], "md");
    assert_eq!(body["template_hash"].as_str().unwrap().len(), 64);
    assert!(body["error"].as_str().unwrap().contains("nope"));
}

#[tokio::test]
async fn api_keys_gate_the_api_but_not_health() {
    let mut c = config();
    c.api_keys = vec!["k1".into(), "k2".into()];
    let router = app(AppState::new(c));

    let res = router
        .clone()
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = router
        .clone()
        .oneshot(post_form("/v1/render", &[("template", Some("a.txt"), "x")]))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let mut req = post_form("/v1/render", &[("template", Some("a.txt"), "x")]);
    req.headers_mut().insert("x-api-key", "k2".parse().unwrap());
    let res = router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mut req = post_form("/v1/render", &[("template", Some("a.txt"), "x")]);
    req.headers_mut()
        .insert(header::AUTHORIZATION, "Bearer k1".parse().unwrap());
    let res = router.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn webhook_delivers_the_result_signed() {
    // A receiver that forwards what it gets.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(axum::http::HeaderMap, bytes::Bytes)>(4);
    let receiver = Router::new().route(
        "/done",
        post(move |headers: axum::http::HeaderMap, body: bytes::Bytes| {
            let tx = tx.clone();
            async move {
                tx.send((headers, body)).await.unwrap();
                StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, receiver).await.unwrap() });

    let mut c = config();
    c.webhook_allow_private = true;
    c.webhook_secret = Some("s3cret".into());
    let state = AppState::new(c);
    let router = app(state);

    let mut req = post_form(
        "/v1/render",
        &[
            ("template", Some("a.txt"), "Hi {{ name }}"),
            ("data", None, r#"{"name":"Ada"}"#),
        ],
    );
    req.headers_mut().insert(
        "x-rusticdoc-webhook-url",
        format!("http://{addr}/done").parse().unwrap(),
    );
    req.headers_mut().insert(
        "x-rusticdoc-webhook-extra-headers",
        r#"{"X-Tenant":"t1"}"#.parse().unwrap(),
    );
    let res = router.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let request_id = res
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let (headers, body) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("delivery in time")
        .unwrap();
    assert_eq!(&body[..], b"Hi Ada");
    assert_eq!(
        headers.get("x-rusticdoc-event").unwrap(),
        "render.completed"
    );
    assert_eq!(
        headers.get("x-request-id").unwrap().to_str().unwrap(),
        request_id
    );
    assert_eq!(headers.get("x-tenant").unwrap(), "t1");
    assert_eq!(
        headers.get("content-disposition").unwrap(),
        "attachment; filename=\"a.txt\""
    );
    assert_eq!(
        headers.get(webhook::SIGNATURE).unwrap().to_str().unwrap(),
        webhook::sign(
            b"s3cret",
            headers
                .get(webhook::TIMESTAMP)
                .unwrap()
                .to_str()
                .unwrap()
                .parse()
                .unwrap(),
            "render.completed",
            headers.get("x-request-id").unwrap().to_str().unwrap(),
            &body
        )
    );
}

#[tokio::test]
async fn webhook_failures_go_to_the_error_url_as_json() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(String, bytes::Bytes)>(4);
    let receiver = Router::new()
        .route(
            "/ok",
            post({
                let tx = tx.clone();
                move |b: bytes::Bytes| {
                    let tx = tx.clone();
                    async move {
                        tx.send(("ok".into(), b)).await.unwrap();
                        StatusCode::OK
                    }
                }
            }),
        )
        .route(
            "/err",
            post(move |b: bytes::Bytes| {
                let tx = tx.clone();
                async move {
                    tx.send(("err".into(), b)).await.unwrap();
                    StatusCode::OK
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, receiver).await.unwrap() });

    let mut c = config();
    c.webhook_allow_private = true;
    let router = app(AppState::new(c));
    let mut req = post_form("/v1/render", &[("template", Some("a.txt"), "{{ nope }}")]);
    req.headers_mut().insert(
        "x-rusticdoc-webhook-url",
        format!("http://{addr}/ok").parse().unwrap(),
    );
    req.headers_mut().insert(
        "x-rusticdoc-webhook-error-url",
        format!("http://{addr}/err").parse().unwrap(),
    );
    assert_eq!(
        router.oneshot(req).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );

    let (route, body) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(route, "err");
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "template_error");
    assert_eq!(json["status"], 422);
}

#[tokio::test]
async fn webhooks_to_private_addresses_are_refused_by_default() {
    let router = app(AppState::new(config()));
    let mut req = post_form("/v1/render", &[("template", Some("a.txt"), "x")]);
    req.headers_mut().insert(
        "x-rusticdoc-webhook-url",
        "http://127.0.0.1:1/x".parse().unwrap(),
    );
    let res = router.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(text(res).await.contains("private address"));
}

#[tokio::test]
async fn convert_and_pdf_routes_do_not_template_their_input() {
    let router = app(AppState::new(config()));
    // A text file with braces is a document to convert, not a template:
    // the failure is the missing engine, not a template error.
    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/convert",
            &[("files", Some("notes.txt"), "literal {{ braces }}")],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    // A text file sent to merge is refused as not a PDF, before any engine.
    let res = router
        .oneshot(post_form(
            "/v1/pdf/merge",
            &[
                ("files", Some("a.txt"), "{{ x }}"),
                ("files", Some("b.pdf"), "%PDF-1.4"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn batch_renders_one_file_per_element_into_a_zip() {
    let router = app(AppState::new(config()));
    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/render/batch",
            &[
                ("template", Some("slip.txt"), "{{ item.name }} owes {{ item.due }} ({{ batch.index }}/{{ batch.count }}); first in list: {{ order.lines[0].name }}"),
                ("data", None, r#"{"order":{"lines":[{"name":"Ada","due":5},{"name":"Bob","due":7},{"name":"Cy","due":9}]}}"#),
                ("splitBy", None, "order.lines"),
                ("batchName", None, "slip-{{ item.name | lower }}"),
                ("outputFilename", None, "slips"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_DISPOSITION).unwrap(),
        "attachment; filename=\"slips.zip\""
    );
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    assert_eq!(archive.len(), 3);
    let names: Vec<String> = (0..3)
        .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
        .collect();
    assert_eq!(names, ["slip-ada.txt", "slip-bob.txt", "slip-cy.txt"]);
    let mut second = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("slip-bob.txt").unwrap(), &mut second)
        .unwrap();
    assert_eq!(second, "Bob owes 7 (2/3); first in list: Bob");

    let res = router
        .oneshot(post_form(
            "/v1/render/batch",
            &[
                ("template", Some("a.txt"), "x"),
                ("data", None, r#"{"n": 1}"#),
                ("splitBy", None, "n"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn root_path_prefixes_every_route() {
    let mut c = config();
    c.root_path = "/docs/".into();
    let router = app(AppState::new(c));
    let res = router
        .clone()
        .oneshot(Request::get("/docs/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = router
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_request_id_is_kept_cleaned_and_echoed() {
    let router = app(AppState::new(config()));
    let mut req = post_form("/v1/render", &[("template", Some("letter.txt"), "Hi")]);
    req.headers_mut()
        .insert("x-request-id", "order-2041".parse().unwrap());
    let res = router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers().get("x-request-id").unwrap(), "order-2041");

    // Nothing printable left: the service makes its own.
    let mut req = post_form("/v1/render", &[("template", Some("letter.txt"), "Hi")]);
    req.headers_mut()
        .insert("x-request-id", "   ".parse().unwrap());
    let res = router.clone().oneshot(req).await.unwrap();
    let id = res.headers().get("x-request-id").unwrap().to_str().unwrap();
    assert!(!id.trim().is_empty());

    for gone in [
        "/",
        "/version",
        "/debug",
        "/prometheus/metrics",
        "/favicon.ico",
    ] {
        let res = router
            .clone()
            .oneshot(Request::get(gone).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{gone}");
    }
    let res = router
        .oneshot(post_form(
            "/forms/libreoffice/convert",
            &[("files", Some("a.docx"), "PK")],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn disabled_route_groups_answer_404() {
    let mut c = config();
    c.convert_disabled = true;
    c.pdf_disabled = true;
    let router = app(AppState::new(c));
    for path in [
        "/v1/convert",
        "/v1/pdf/convert",
        "/v1/pdf/merge",
        "/v1/pdf/rotate",
    ] {
        let res = router
            .clone()
            .oneshot(post_form(path, &[("files", Some("a.pdf"), "x")]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn download_from_fetches_files_with_extra_headers() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<axum::http::HeaderMap>(4);
    let server = Router::new().route(
        "/letter.txt",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let tx = tx.clone();
            async move {
                tx.send(headers).await.unwrap();
                "Dear {{ name }}"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });

    let mut c = config();
    c.download_from_allow_private = true;
    let router = app(AppState::new(c));
    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/render",
            &[
                ("downloadFrom", None, &format!(r#"[{{"url":"http://{addr}/letter.txt","extraHttpHeaders":{{"X-Foo":"bar"}}}}]"#)),
                ("data", None, r#"{"name":"Ada"}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_DISPOSITION).unwrap(),
        "attachment; filename=\"letter.txt\""
    );
    assert_eq!(text(res).await, "Dear Ada");
    let headers = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(headers.get("x-foo").unwrap(), "bar");

    // Without allow-private, a loopback download is refused like a webhook.
    let router = app(AppState::new(config()));
    let res = router
        .oneshot(post_form(
            "/v1/render",
            &[(
                "downloadFrom",
                None,
                &format!(r#"[{{"url":"http://{addr}/letter.txt"}}]"#),
            )],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn state_is_shareable() {
    let state = Arc::new(AppState::new(config()));
    assert!(state.engines.libreoffice.is_none());
}

#[tokio::test]
async fn pdf_templates_are_filled_forms() {
    use lopdf::{Document as Pdf, Object, Stream, dictionary};
    // A one-page PDF with one text field named `customer.name`.
    let mut pdf = Pdf::with_version("1.7");
    let pages_id = pdf.new_object_id();
    let font_id = pdf.add_object(
        dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"},
    );
    let content_id = pdf.add_object(Stream::new(
        dictionary! {},
        b"BT /F1 12 Tf 72 700 Td (Hello) Tj ET".to_vec(),
    ));
    let page_id = pdf.new_object_id();
    let field_id = pdf.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("customer.name"),
        "Rect" => vec![72.into(), 600.into(), 300.into(), 620.into()], "P" => page_id,
        "DA" => Object::string_literal("/Helv 10 Tf 0 g"), "F" => 4
    });
    pdf.objects.insert(page_id, Object::Dictionary(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        "Resources" => dictionary! {"Font" => dictionary! {"F1" => font_id}},
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Annots" => vec![field_id.into()]
    }));
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! {"Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1},
        ),
    );
    let catalog_id = pdf.add_object(dictionary! {
        "Type" => "Catalog", "Pages" => pages_id,
        "AcroForm" => dictionary! {"Fields" => vec![field_id.into()], "DR" => dictionary! {"Font" => dictionary! {"Helv" => font_id}}}
    });
    pdf.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let body = String::from_utf8_lossy(&bytes).to_string();

    let router = app(AppState::new(config()));
    let res = router
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("form.pdf"), &body),
                ("data", None, r#"{"customer":{"name":"Ada Lovelace"}}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let out = res.into_body().collect().await.unwrap().to_bytes();
    let filled = Pdf::load_mem(&out).unwrap();
    let field = filled.get_object(field_id).unwrap().as_dict().unwrap();
    let value = field.get(b"V").unwrap().as_str().unwrap();
    assert!(String::from_utf8_lossy(value).contains("Ada Lovelace"));
}

/// Multipart with binary parts, for PDF fixtures.
fn post_binary(path: &str, parts: &[(&str, Option<&str>, Vec<u8>)]) -> Request<Body> {
    let mut body: Vec<u8> = Vec::new();
    for (name, filename, content) in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        match filename {
            Some(f) => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes(),
            ),
            None => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes()),
        }
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    Request::post(path)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap()
}

async fn bytes_of(res: axum::response::Response) -> Vec<u8> {
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
}

#[tokio::test]
async fn pdf_operations_run_on_the_pure_rust_engine() {
    use rusticdoc_core::pdfops::fixtures;
    let router = app(AppState::new(config()));
    let three = fixtures::pages(3);

    // split by pages into a ZIP of two parts
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/split",
            &[
                ("files", Some("in.pdf"), three.clone()),
                ("splitMode", None, b"pages".to_vec()),
                ("splitSpan", None, b"1-2,3".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/zip"
    );
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes_of(res).await)).unwrap();
    assert_eq!(archive.len(), 2);
    assert_eq!(archive.by_index(0).unwrap().name().unwrap(), "in_1.pdf");

    // rotate, then read the rotation back
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/rotate",
            &[
                ("files", Some("in.pdf"), three.clone()),
                ("rotation", None, b"90".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let rotated = lopdf::Document::load_mem(&bytes_of(res).await).unwrap();
    let first = rotated.get_pages()[&1];
    assert_eq!(
        rotated
            .get_object(first)
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Rotate")
            .unwrap()
            .as_i64()
            .unwrap(),
        90
    );

    // metadata write then read
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/metadata/write",
            &[
                ("files", Some("in.pdf"), three.clone()),
                (
                    "metadata",
                    None,
                    br#"{"Title":"Invoice 7","Author":"Ada"}"#.to_vec(),
                ),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let with_meta = bytes_of(res).await;
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/metadata/read",
            &[("files", Some("in.pdf"), with_meta)],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let meta: serde_json::Value = serde_json::from_slice(&bytes_of(res).await).unwrap();
    assert_eq!(meta["in.pdf"]["Title"], "Invoice 7");
    assert_eq!(meta["in.pdf"]["Author"], "Ada");

    // bookmarks write then read
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/bookmarks/write",
            &[
                ("files", Some("in.pdf"), three.clone()),
                (
                    "bookmarks",
                    None,
                    br#"[{"title":"Start","page":1,"children":[{"title":"End","page":3}]}]"#
                        .to_vec(),
                ),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let marked = bytes_of(res).await;
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/bookmarks/read",
            &[("files", Some("in.pdf"), marked)],
        ))
        .await
        .unwrap();
    let marks: serde_json::Value = serde_json::from_slice(&bytes_of(res).await).unwrap();
    assert_eq!(marks["in.pdf"][0]["title"], "Start");
    assert_eq!(marks["in.pdf"][0]["children"][0]["page"], 3);

    // text stamp
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/stamp",
            &[
                ("files", Some("in.pdf"), three.clone()),
                ("text", None, b"DRAFT".to_vec()),
                ("opacity", None, b"0.5".to_vec()),
                ("rotation", None, b"45".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(bytes_of(res).await.len() > three.len());

    // embed, then the attachment is in the name tree
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/embed",
            &[
                ("files", Some("in.pdf"), three.clone()),
                ("files", Some("note.txt"), b"hello".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let embedded = bytes_of(res).await;
    assert!(String::from_utf8_lossy(&embedded).contains("EmbeddedFile"));

    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/merge",
            &[
                ("files", Some("a.pdf"), three.clone()),
                ("files", Some("b.pdf"), fixtures::pages(2)),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let merged = lopdf::Document::load_mem(&bytes_of(res).await).unwrap();
    assert_eq!(merged.get_pages().len(), 5);

    // bad requests are 400
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/rotate",
            &[
                ("files", Some("in.pdf"), three.clone()),
                ("rotation", None, b"45".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = router
        .oneshot(post_binary(
            "/v1/pdf/split",
            &[
                ("files", Some("in.pdf"), three),
                ("splitMode", None, b"sideways".to_vec()),
                ("splitSpan", None, b"1".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn boolean_fields_are_strict() {
    let router = app(AppState::new(config()));
    let res = router
        .oneshot(post_form(
            "/v1/convert",
            &[
                ("files", Some("a.docx"), "PK"),
                ("landscape", None, "maybe"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn templates_can_be_reused_by_hash_and_a_miss_is_a_404() {
    let router = app(AppState::new(config()));
    let body = "Reuse {{ who }} ".to_string() + &format!("{:?}", std::thread::current().id());
    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("reuse.txt"), &body),
                ("data", None, r#"{"who":"me"}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let hash = rusticdoc_core::template::template_hash(body.as_bytes());
    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/render",
            &[
                ("templateHash", None, &hash),
                ("data", None, r#"{"who":"again"}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(text(res).await.starts_with("Reuse again"));

    let res = router
        .oneshot(post_form(
            "/v1/render",
            &[("templateHash", None, "00ff"), ("data", None, "{}")],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["error"], "template_not_cached");
}

#[tokio::test]
async fn named_steps_take_their_fields_and_refuse_empty_ones() {
    use rusticdoc_core::pdfops::fixtures;
    let router = app(AppState::new(config()));
    // A PDF through the render route with a stamp in the step list.
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("template", Some("in.pdf"), fixtures::pages(1)),
                ("steps", None, b"stamp,rotate".to_vec()),
                ("stampSource", None, b"text".to_vec()),
                ("stampExpression", None, b"PAID".to_vec()),
                ("rotateAngle", None, b"90".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let res = router
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("template", Some("in.pdf"), fixtures::pages(1)),
                ("steps", None, b"watermark".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn underlay_lays_the_template_over_a_base_pdf() {
    use rusticdoc_core::pdfops::fixtures;
    let router = app(AppState::new(config()));
    // The base comes first in the form and is still not taken as the
    // template.
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("underlay", Some("letterhead.pdf"), fixtures::pages(3)),
                ("template", Some("letter.pdf"), fixtures::pages(1)),
                ("steps", None, b"underlay".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let out = bytes_of(res).await;
    let doc = lopdf::Document::load_mem(&out).unwrap();
    assert_eq!(doc.get_pages().len(), 3);
    let first = doc.get_page_content(doc.get_pages()[&1]);
    assert!(String::from_utf8_lossy(&first).contains("/RdOv1 Do"));

    let res = router
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("template", Some("letter.pdf"), fixtures::pages(1)),
                ("steps", None, b"underlay".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(text(res).await.contains("under 'underlay'"));
}

#[tokio::test]
async fn signatures_land_at_exact_coordinates() {
    use rusticdoc_core::pdfops::fixtures;
    let router = app(AppState::new(config()));
    let png = fixtures::PNG_DOT.to_vec();
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/sign",
            &[
                ("files", Some("in.pdf"), fixtures::pages(2)),
                ("files", Some("sig.png"), png.clone()),
                ("page", None, b"2".to_vec()),
                ("x", None, b"100".to_vec()),
                ("y", None, b"72".to_vec()),
                ("width", None, b"150".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let pdf = lopdf::Document::load_mem(&bytes_of(res).await).unwrap();
    let page2 = pdf.get_pages()[&2];
    let content = String::from_utf8_lossy(&pdf.get_page_content(page2)).to_string();
    assert!(
        content.contains("175"),
        "the centre is x + width / 2: {content}"
    );
    assert!(content.contains("/FxI"), "{content}");

    let res = router
        .oneshot(post_binary(
            "/v1/pdf/sign",
            &[
                ("files", Some("in.pdf"), fixtures::pages(1)),
                ("files", Some("sig.png"), png),
                ("page", None, b"1".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// A service with a throwaway ECDSA key and self-signed certificate in
/// `dir`, as `RUSTICDOC_SIGNING_KEY` and `RUSTICDOC_SIGNING_CERT` would give it.
fn signing_config(dir: &std::path::Path) -> Config {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let cert = rcgen::CertificateParams::new(vec!["signer.test".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(dir.join("key.pem"), key.serialize_pem()).unwrap();
    std::fs::write(dir.join("cert.pem"), cert.pem()).unwrap();
    let config = Config {
        signing_key: Some(dir.join("key.pem")),
        signing_cert: Some(dir.join("cert.pem")),
        ..config()
    };
    config.check().expect("the test key loads");
    config
}

fn signed_by_certificate(original: &[u8], signed: &[u8]) {
    assert!(signed.starts_with(original), "an incremental update");
    let tail = &signed[original.len()..];
    assert!(tail.windows(19).any(|w| w == b"ETSI.CAdES.detached"));
    assert!(tail.windows(10).any(|w| w == b"/ByteRange"));
    lopdf::Document::load_mem(signed).expect("the signed file parses");
}

#[tokio::test]
async fn certificate_signatures_answer_503_without_a_key() {
    use rusticdoc_core::pdfops::fixtures;
    let router = app(AppState::new(config()));
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/sign",
            &[
                ("files", Some("in.pdf"), fixtures::pages(1)),
                ("signature", None, b"certificate".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("RUSTICDOC_SIGNING_KEY"),
        "{body}"
    );

    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("template", Some("in.pdf"), fixtures::pages(1)),
                ("steps", None, b"sign".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);

    // The image signature is still there, and stays the default.
    let res = router
        .oneshot(post_binary(
            "/v1/pdf/sign",
            &[
                ("files", Some("in.pdf"), fixtures::pages(1)),
                ("files", Some("sig.png"), fixtures::PNG_DOT.to_vec()),
                ("page", None, b"1".to_vec()),
                ("x", None, b"10".to_vec()),
                ("y", None, b"10".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
}

#[tokio::test]
async fn certificate_signatures_sign_with_the_configured_key() {
    use rusticdoc_core::pdfops::fixtures;
    let dir = tempfile::tempdir().unwrap();
    let router = app(AppState::new(signing_config(dir.path())));
    let original = fixtures::pages(2);

    // Invisible, with the descriptive fields.
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/sign",
            &[
                ("files", Some("in.pdf"), original.clone()),
                ("signature", None, b"certificate".to_vec()),
                ("reason", None, b"Approved".to_vec()),
                ("location", None, b"Nairobi".to_vec()),
                ("contactInfo", None, b"ops@example.com".to_vec()),
                ("signerName", None, b"Ada".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let signed = bytes_of(res).await;
    signed_by_certificate(&original, &signed);
    assert!(signed.windows(10).any(|w| w == b"(Approved)"));

    // Visible, with the image as the widget's appearance on page 2.
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/sign",
            &[
                ("files", Some("in.pdf"), original.clone()),
                ("files", Some("sig.png"), fixtures::PNG_DOT.to_vec()),
                ("signature", None, b"certificate".to_vec()),
                ("page", None, b"2".to_vec()),
                ("x", None, b"100".to_vec()),
                ("y", None, b"72".to_vec()),
                ("width", None, b"150".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    signed_by_certificate(&original, &bytes_of(res).await);

    // As the last step of a render.
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("template", Some("in.pdf"), original.clone()),
                ("steps", None, b"rotate,sign".to_vec()),
                ("reason", None, b"Rendered".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let rendered = bytes_of(res).await;
    assert!(rendered.windows(19).any(|w| w == b"ETSI.CAdES.detached"));
    assert!(rendered.windows(10).any(|w| w == b"(Rendered)"));
    let pdf = lopdf::Document::load_mem(&rendered).unwrap();
    let page = pdf.get_dictionary(pdf.get_pages()[&1]).unwrap();
    assert_eq!(
        page.get(b"Rotate").unwrap().as_i64().unwrap(),
        90,
        "rotated, then signed"
    );

    // What the request gets wrong is a 400.
    for (parts, says) in [
        (
            vec![
                ("files", Some("in.pdf"), original.clone()),
                ("files", Some("a.png"), fixtures::PNG_DOT.to_vec()),
                ("files", Some("b.png"), fixtures::PNG_DOT.to_vec()),
                ("signature", None, b"certificate".to_vec()),
            ],
            "at most one",
        ),
        (
            vec![
                ("files", Some("in.pdf"), original.clone()),
                ("signature", None, b"certificate".to_vec()),
                ("x", None, b"10".to_vec()),
            ],
            "one 'x' and one 'y'",
        ),
        (
            vec![
                ("files", Some("in.pdf"), original.clone()),
                ("files", Some("sig.png"), fixtures::PNG_DOT.to_vec()),
                ("page", None, b"1".to_vec()),
                ("x", None, b"10".to_vec()),
                ("y", None, b"10".to_vec()),
                ("reason", None, b"Approved".to_vec()),
            ],
            "signature=certificate",
        ),
        (
            vec![
                ("files", Some("in.pdf"), original.clone()),
                ("signature", None, b"wet-ink".to_vec()),
            ],
            "image or certificate",
        ),
    ] {
        let res = router
            .clone()
            .oneshot(post_binary("/v1/pdf/sign", &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let message = text(res).await;
        assert!(message.contains(says), "{says}: {message}");
    }
}

#[tokio::test]
async fn sign_steps_come_last_and_never_with_encrypt() {
    use rusticdoc_core::pdfops::fixtures;
    let dir = tempfile::tempdir().unwrap();
    // With a key, so the refusals are about the order, not the key; and
    // without one, where the order is still a 400 rather than a 503.
    for config in [signing_config(dir.path()), config()] {
        let router = app(AppState::new(config));
        for (steps, says) in [
            ("sign,encrypt", "cannot be combined"),
            ("encrypt,sign", "cannot be combined"),
            ("sign,flatten", "last step"),
            ("sign,sign", "last step"),
        ] {
            let res = router
                .clone()
                .oneshot(post_binary(
                    "/v1/render",
                    &[
                        ("template", Some("in.pdf"), fixtures::pages(1)),
                        ("steps", None, steps.as_bytes().to_vec()),
                        ("userPassword", None, b"secret".to_vec()),
                    ],
                ))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{steps}");
            let message = text(res).await;
            assert!(message.contains(says), "{steps}: {message}");
        }
    }
}

/// The `/Rect` of the widget of the last signature field in `signed`.
fn last_signature_rect(signed: &[u8]) -> Vec<f32> {
    let pdf = lopdf::Document::load_mem(signed).unwrap();
    let acroform = match pdf.catalog().unwrap().get(b"AcroForm").unwrap() {
        lopdf::Object::Reference(id) => pdf.get_dictionary(*id).unwrap().clone(),
        lopdf::Object::Dictionary(d) => d.clone(),
        other => panic!("unexpected AcroForm {other:?}"),
    };
    let fields = acroform.get(b"Fields").unwrap().as_array().unwrap();
    let field = pdf
        .get_dictionary(fields.last().unwrap().as_reference().unwrap())
        .unwrap();
    field
        .get(b"Rect")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_float().unwrap())
        .collect()
}

#[tokio::test]
async fn visible_signatures_show_an_image_or_text_on_both_routes() {
    use rusticdoc_core::pdfops::fixtures;
    let dir = tempfile::tempdir().unwrap();
    let router = app(AppState::new(signing_config(dir.path())));
    let original = fixtures::pages(1);

    // /v1/pdf/sign with a place and no image: text, 200 points wide.
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/pdf/sign",
            &[
                ("files", Some("in.pdf"), original.clone()),
                ("signature", None, b"certificate".to_vec()),
                ("x", None, b"72".to_vec()),
                ("y", None, b"72".to_vec()),
                ("reason", None, b"Approved".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let signed = bytes_of(res).await;
    signed_by_certificate(&original, &signed);
    let rect = last_signature_rect(&signed);
    assert_eq!(rect[..3], [72.0, 72.0, 272.0]);

    // The sign step of /v1/render, with an image under signatureImage.
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("template", Some("in.pdf"), original.clone()),
                (
                    "signatureImage",
                    Some("sig.png"),
                    fixtures::PNG_DOT.to_vec(),
                ),
                ("steps", None, b"sign".to_vec()),
                ("page", None, b"1".to_vec()),
                ("x", None, b"100".to_vec()),
                ("y", None, b"50".to_vec()),
                ("width", None, b"80".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let rendered = bytes_of(res).await;
    signed_by_certificate(&original, &rendered);
    assert_eq!(last_signature_rect(&rendered), [100.0, 50.0, 180.0, 130.0]);
    assert!(rendered.windows(7).any(|w| w == b"/Img Do"));

    // And as text, with the signer's name.
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("template", Some("in.pdf"), original.clone()),
                ("steps", None, b"sign".to_vec()),
                ("x", None, b"10".to_vec()),
                ("y", None, b"10".to_vec()),
                ("signerName", None, b"Ada".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let rendered = bytes_of(res).await;
    assert_eq!(last_signature_rect(&rendered)[..3], [10.0, 10.0, 210.0]);

    // A signature image with no sign step is a mistake.
    let res = router
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("template", Some("in.pdf"), original.clone()),
                (
                    "signatureImage",
                    Some("sig.png"),
                    fixtures::PNG_DOT.to_vec(),
                ),
                ("steps", None, b"rotate".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn named_signing_keys_are_for_the_callers_on_their_allow_list() {
    use rusticdoc_core::pdfops::fixtures;
    use rusticdoc_core::pdfops::sign::NamedKey;
    let dir = tempfile::tempdir().unwrap();
    let mut config = signing_config(dir.path());
    config.api_keys = vec!["alpha".into(), "beta".into()];
    config.signing.named = vec![NamedKey {
        name: "acme".into(),
        key: dir.path().join("key.pem").to_string_lossy().into_owned(),
        cert: dir.path().join("cert.pem"),
        allow: vec![rusticdoc_server::auth::key_principal("alpha").0],
    }];
    config.check().expect("the named key loads");
    let router = app(AppState::new(config));
    let request = |caller: &str, key: &str| {
        let mut req = post_binary(
            "/v1/pdf/sign",
            &[
                ("files", Some("in.pdf"), fixtures::pages(1)),
                ("signature", None, b"certificate".to_vec()),
                ("signingKey", None, key.as_bytes().to_vec()),
            ],
        );
        req.headers_mut()
            .insert("x-api-key", caller.parse().unwrap());
        req
    };
    let res = router
        .clone()
        .oneshot(request("alpha", "acme"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let res = router
        .clone()
        .oneshot(request("beta", "acme"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(text(res).await.contains("acme"));
    let res = router
        .clone()
        .oneshot(request("beta", "globex"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // The same check on the sign step of a render.
    let mut req = post_binary(
        "/v1/render",
        &[
            ("template", Some("in.pdf"), fixtures::pages(1)),
            ("steps", None, b"sign".to_vec()),
            ("signingKey", None, b"acme".to_vec()),
        ],
    );
    req.headers_mut()
        .insert("x-api-key", "beta".parse().unwrap());
    let res = router.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn timestamped_levels_need_a_timestamp_authority_reached_under_the_outbound_rules() {
    use rusticdoc_core::pdfops::fixtures;
    let dir = tempfile::tempdir().unwrap();
    let parts = |level: &str| {
        vec![
            ("files", Some("in.pdf"), fixtures::pages(1)),
            ("signature", None, b"certificate".to_vec()),
            ("level", None, level.as_bytes().to_vec()),
        ]
    };

    // No timestamp authority: a 503 that names the setting.
    let router = app(AppState::new(signing_config(dir.path())));
    let res = router
        .clone()
        .oneshot(post_binary("/v1/pdf/sign", &parts("b-t")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(text(res).await.contains("RUSTICDOC_SIGNING_TSA_URL"));
    let res = router
        .oneshot(post_binary("/v1/pdf/sign", &parts("b-q")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // One on loopback, which the outbound rules refuse by default: a 502,
    // and no file.
    let mut config = signing_config(dir.path());
    config.signing.tsa = Some(
        rusticdoc_core::pdfops::sign::Tsa::new("http://127.0.0.1:9/tsa", Duration::from_secs(2))
            .with_anchors_pem(&std::fs::read(dir.path().join("cert.pem")).unwrap())
            .unwrap(),
    );
    let router = app(AppState::new(config.clone()));
    let res = router
        .oneshot(post_binary("/v1/pdf/sign", &parts("b-lta")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    let message = text(res).await;
    assert!(message.contains("timestamp authority"), "{message}");
    assert!(message.contains("private"), "{message}");

    // A deny list refuses it before any connection.
    config.signing.outbound_allow_private = true;
    config.signing.outbound_deny_list = vec!["^http://127\\.0\\.0\\.1:9/".into()];
    let router = app(AppState::new(config));
    let res = router
        .oneshot(post_binary("/v1/pdf/sign", &parts("b-t")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    assert!(text(res).await.contains("not allowed"));
}

#[tokio::test]
async fn fonts_are_listed_behind_auth_and_reload_without_engines() {
    let mut c = config();
    c.fonts_dir = std::env::temp_dir();
    let router = app(AppState::new(c));
    let res = router
        .clone()
        .oneshot(Request::get("/fonts").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert!(list.is_array());
    let res = router
        .oneshot(Request::post("/fonts/reload").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mut c = config();
    c.api_keys = vec!["k".into()];
    let router = app(AppState::new(c));
    let res = router
        .oneshot(Request::get("/fonts").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn template_reuse_is_scoped_to_the_caller() {
    let mut c = config();
    c.api_keys = vec!["tenant-a".into(), "tenant-b".into()];
    let router = app(AppState::new(c));
    let body = format!("Scoped {{{{ who }}}} {:?}", std::thread::current().id());
    let hash = rusticdoc_core::template::template_hash(body.as_bytes());

    let mut req = post_form(
        "/v1/render",
        &[
            ("template", Some("t.txt"), &body),
            ("data", None, r#"{"who":"a"}"#),
        ],
    );
    req.headers_mut()
        .insert("x-api-key", "tenant-a".parse().unwrap());
    assert_eq!(
        router.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    let mut req = post_form(
        "/v1/render",
        &[
            ("templateHash", None, &hash),
            ("data", None, r#"{"who":"a"}"#),
        ],
    );
    req.headers_mut()
        .insert("x-api-key", "tenant-a".parse().unwrap());
    assert_eq!(
        router.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    let mut req = post_form(
        "/v1/render",
        &[
            ("templateHash", None, &hash),
            ("data", None, r#"{"who":"b"}"#),
        ],
    );
    req.headers_mut()
        .insert("x-api-key", "tenant-b".parse().unwrap());
    assert_eq!(
        router.oneshot(req).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn font_reload_is_rate_limited_and_skips_an_unchanged_directory() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.fonts_dir = dir.path().to_path_buf();
    let state = AppState::new(c);
    let router = app(state.clone());
    let res = router
        .clone()
        .oneshot(Request::post("/fonts/reload").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["changed"], true);
    let res = router
        .clone()
        .oneshot(Request::post("/fonts/reload").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);

    // Past the interval, an unchanged directory restarts nothing.
    if let Some((_, at)) = state.fonts_reload.lock().await.as_mut() {
        *at -= Duration::from_secs(60);
    }
    let res = router
        .oneshot(Request::post("/fonts/reload").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["changed"], false);
}

#[tokio::test]
async fn html_converts_with_fulgur_and_its_assets() {
    let router = app(AppState::new(config()));
    // A 1x1 PNG.
    let logo: Vec<u8> = vec![
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let html = br#"<!doctype html><html><body><h1>Invoice</h1><img src="logo.png" width="40" height="40"></body></html>"#;
    let res = router
        .clone()
        .oneshot(post_binary(
            "/v1/convert",
            &[
                ("files", Some("invoice.html"), html.to_vec()),
                ("assets", Some("logo.png"), logo),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_DISPOSITION).unwrap(),
        "attachment; filename=\"invoice.pdf\""
    );
    let pdf = bytes_of(res).await;
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(String::from_utf8_lossy(&pdf).contains("/Image"));

    // An HTML template is filled, then laid out.
    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("letter.html"), "<p>Dear {{ name }}</p>"),
                ("data", None, r#"{"name":"Ada"}"#),
                ("steps", None, "convert"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);

    // Asking for LibreOffice reaches LibreOffice, which this service lacks.
    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/convert",
            &[
                ("files", Some("a.html"), "<p>x</p>"),
                ("htmlEngine", None, "libreoffice"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);

    for (field, value, file) in [
        ("htmlEngine", Some("chromium"), None),
        ("assets", None, Some("script.js")),
    ] {
        let mut parts = vec![("files", Some("a.html"), "<p>x</p>".to_string())];
        if let Some(v) = value {
            parts.push((field, None, v.to_string()));
        }
        if let Some(f) = file {
            parts.push((field, Some(f), "alert(1)".to_string()));
        }
        let parts: Vec<(&str, Option<&str>, &str)> =
            parts.iter().map(|(a, b, c)| (*a, *b, c.as_str())).collect();
        let res = router
            .clone()
            .oneshot(post_form("/v1/convert", &parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{field}");
    }
}

#[tokio::test]
async fn disabling_conversion_stops_fulgur_too() {
    let mut c = config();
    c.convert_disabled = true;
    let router = app(AppState::new(c));
    let res = router
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("a.html"), "<p>x</p>"),
                ("steps", None, "convert"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// A DOCX whose document names `font`, the smallest Word will open.
fn docx_in(font: &str) -> Vec<u8> {
    use std::io::Write;
    let mut out = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut out);
    let options: zip::write::SimpleFileOptions = Default::default();
    for (name, body) in [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string(),
        ),
        (
            "word/document.xml",
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}"/></w:rPr><w:t>Hello {{{{ name }}}}</w:t></w:r></w:p></w:body></w:document>"#),
        ),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    out.into_inner()
}

#[tokio::test]
async fn validation_reports_fonts_the_service_lacks() {
    let template = docx_in("Nonexistent Grotesque 9");
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"template\"; filename=\"letter.docx\"\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&template);
    body.extend_from_slice(
        format!("\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"data\"\r\n\r\n{{\"name\":\"Ada\"}}\r\n--{BOUNDARY}--\r\n").as_bytes(),
    );
    let req = Request::post("/v1/templates/validate")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    let res = app(AppState::new(config())).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let report: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(
        report["ok"], true,
        "a missing font warns, it does not fail: {report}"
    );
    assert_eq!(
        report["missing_fonts"],
        serde_json::json!(["Nonexistent Grotesque 9"]),
        "{report}"
    );
    let warning = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["severity"] == "warning")
        .expect("a warning");
    assert!(
        warning["message"]
            .as_str()
            .unwrap()
            .contains("Nonexistent Grotesque 9"),
        "{warning}"
    );
}

#[tokio::test]
async fn the_api_description_covers_json_bodies_aliases_variables_previews_and_jobs() {
    let res = app(AppState::new(config()))
        .oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let spec: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    for path in ["/v1/render", "/v1/render/batch", "/v1/templates/validate"] {
        let content = &spec["paths"][path]["post"]["requestBody"]["content"];
        assert!(content["multipart/form-data"].is_object(), "{path}");
        assert!(content["application/json"].is_object(), "{path}");
    }
    assert!(spec["paths"]["/v1/render"]["post"]["responses"]["202"].is_object());
    for (path, method) in [
        ("/v1/aliases", "get"),
        ("/v1/aliases/{name}", "put"),
        ("/v1/aliases/{name}", "get"),
        ("/v1/aliases/{name}", "delete"),
        ("/v1/templates/{id}/variables", "get"),
        ("/v1/templates/{id}/preview", "post"),
        ("/v1/jobs/{id}", "get"),
        ("/v1/jobs/{id}/result", "get"),
    ] {
        assert!(spec["paths"][path][method].is_object(), "{method} {path}");
    }
    for schema in [
        "RenderJson",
        "JobSchema",
        "AliasHistorySchema",
        "TemplateVariables",
    ] {
        assert!(
            spec["components"]["schemas"][schema].is_object(),
            "{schema}"
        );
    }
}

#[tokio::test]
async fn background_jobs_and_aliases_need_a_store_and_say_so() {
    let router = app(AppState::new(config()));
    let mut req = post_form(
        "/v1/render",
        &[("template", Some("t.txt"), "Hi"), ("data", None, "{}")],
    );
    req.headers_mut()
        .insert("prefer", "respond-async".parse().unwrap());
    let res = router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    let body = text(res).await;
    assert!(body.contains("store_not_configured"), "{body}");

    for (method, path) in [
        ("GET", "/v1/jobs/01966f3e-7c1a-7d3c-9a44-1d2f5a6b7c8d"),
        ("GET", "/v1/aliases"),
        ("GET", "/v1/aliases/invoice"),
        ("GET", "/v1/templates/invoice/variables"),
    ] {
        let res = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED, "{method} {path}");
    }

    // Without the preference, the same request renders as it always did.
    let res = router
        .oneshot(post_form(
            "/v1/render",
            &[("template", Some("t.txt"), "Hi"), ("data", None, "{}")],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_json_body_without_a_store_can_still_use_a_template_hash() {
    let router = app(AppState::new(config()));
    let res = router
        .clone()
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("hash.txt"), "Hi {{ name }}"),
                ("data", None, r#"{"name":"A"}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let hash = rusticdoc_core::template::template_hash(b"Hi {{ name }}");
    let res = router
        .clone()
        .oneshot(
            Request::post("/v1/render")
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .body(Body::from(
                    serde_json::json!({ "templateHash": hash, "data": { "name": "B" } })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(text(res).await, "Hi B");
    // A templateId needs a store.
    let res = router
        .oneshot(
            Request::post("/v1/render")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"templateId":"invoice"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn undefined_values_can_print_as_nothing() {
    let template = (
        "template",
        Some("letter.txt"),
        "Dear {{ customer.name }}{{ missing.deeper }}.",
    );
    let data = ("data", None, r#"{"customer":{}}"#);
    let strict = app(AppState::new(config()))
        .oneshot(post_form("/v1/render", &[template, data]))
        .await
        .unwrap();
    assert_eq!(strict.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let lenient = app(AppState::new(config()))
        .oneshot(post_form(
            "/v1/render",
            &[template, data, ("undefinedAsEmpty", None, "true")],
        ))
        .await
        .unwrap();
    assert_eq!(lenient.status(), StatusCode::OK);
    assert_eq!(text(lenient).await, "Dear .");
}

#[tokio::test]
async fn keep_names_only_what_rendering_takes_out() {
    let template = ("template", Some("letter.txt"), "Hello");
    let res = app(AppState::new(config()))
        .oneshot(post_form(
            "/v1/render",
            &[template, ("keep", None, "macros, bogus")],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(text(res).await.contains("'bogus'"));
    let res = app(AppState::new(config()))
        .oneshot(post_form(
            "/v1/render",
            &[
                template,
                ("keep", None, "macros,objects,externalLinks,privateLinks"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn language_fields_format_and_translate_in_multipart() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("fr.json"),
        r#"{"due": "Échéance", "total": "Total (serveur)"}"#,
    )
    .unwrap();
    let state = AppState::new(Config {
        translations_dir: Some(dir.path().to_path_buf()),
        ..config()
    });
    let template = r#"{% settings t.total="Total" %}{{ "total" | t }}|{{ "due" | t }}|{{ 1500 | currency }}|{{ 1234.5 | number }}|{{ when | date("%d %B %H:%M") }}"#;
    let res = app(state.clone())
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("t.txt"), template),
                ("data", None, r#"{"when":"2026-07-01T10:00:00Z"}"#),
                ("lang", None, "FR_ca"),
                ("currency", None, "EUR"),
                ("timezone", None, "America/Toronto"),
                ("translations", None, r#"{"fr-CA": {"total": "Montant"}}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        text(res).await,
        "Montant|Échéance|1\u{a0}500,00\u{a0}€|1\u{a0}234,5|01 juillet 06:00"
    );

    // Without the fields, the same template prints as it always has.
    let res = app(state.clone())
        .oneshot(post_form(
            "/v1/render",
            &[
                ("template", Some("t.txt"), template),
                ("data", None, r#"{"when":"2026-07-01T10:00:00Z"}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(text(res).await, "Total|due|1,500.00|1,234.5|01 July 10:00");

    let long = "X".repeat(20);
    for (field, value, wanted) in [
        ("lang", "French!", "'lang'"),
        ("timezone", "Mars/Olympus", "'timezone'"),
        ("currency", long.as_str(), "'currency'"),
        ("translations", r#"{"fr": {"a": 1}}"#, "'translations.fr'"),
        ("translations", r#"["fr"]"#, "'translations'"),
        ("translations", "{", "'translations'"),
    ] {
        let res = app(state.clone())
            .oneshot(post_form(
                "/v1/render",
                &[("template", Some("t.txt"), "x"), (field, None, value)],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{field}={value}");
        let body = text(res).await;
        assert!(body.contains(wanted), "{field}: {body}");
    }
}

#[tokio::test]
async fn validation_and_batches_take_the_language_fields() {
    let state = AppState::new(config());
    let res = app(state.clone())
        .oneshot(post_form(
            "/v1/templates/validate",
            &[
                (
                    "template",
                    Some("t.txt"),
                    r#"{{ "total" | t }} {{ "due" | t }}"#,
                ),
                ("lang", None, "de-CH"),
                ("translations", None, r#"{"de": {"total": "Summe"}}"#),
            ],
        ))
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
    assert_eq!(body["ok"], true, "{body}");
    let warnings: Vec<&str> = body["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["message"].as_str().unwrap())
        .collect();
    assert_eq!(warnings.len(), 1, "{body}");
    assert!(
        warnings[0].contains("no de-ch translation for \"due\""),
        "{body}"
    );

    let res = app(state)
        .oneshot(post_form(
            "/v1/render/batch",
            &[
                ("template", Some("t.txt"), "{{ item.n | number(2) }}"),
                ("data", None, r#"{"items":[{"n":1234.5}]}"#),
                ("splitBy", None, "items"),
                ("batchName", None, "{{ \"file\" | t }}"),
                ("lang", None, "de"),
                ("translations", None, r#"{"de": {"file": "Datei"}}"#),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let zip = res.into_body().collect().await.unwrap().to_bytes();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip.to_vec())).unwrap();
    let mut entry = archive.by_index(0).unwrap();
    assert_eq!(entry.name().unwrap(), "Datei.txt");
    let mut content = String::new();
    std::io::Read::read_to_string(&mut entry, &mut content).unwrap();
    assert_eq!(content, "1.234,50");
}

#[tokio::test]
async fn pdf_form_tags_translate_and_format() {
    use lopdf::{Document as Pdf, Object, Stream, dictionary};
    // A text field whose default value is a tag.
    let mut pdf = Pdf::with_version("1.7");
    let pages_id = pdf.new_object_id();
    let font_id = pdf.add_object(
        dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"},
    );
    let content_id = pdf.add_object(Stream::new(dictionary! {}, b"".to_vec()));
    let page_id = pdf.new_object_id();
    let field_id = pdf.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("label"),
        "DV" => Object::string_literal("{{ \"total\" | t }} {{ amount | currency }} {{ day | date(\"%d %B\") }}"),
        "Rect" => vec![72.into(), 600.into(), 400.into(), 620.into()], "P" => page_id,
        "DA" => Object::string_literal("/Helv 10 Tf 0 g"), "F" => 4
    });
    pdf.objects.insert(page_id, Object::Dictionary(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Annots" => vec![field_id.into()]
    }));
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! {"Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1},
        ),
    );
    let catalog_id = pdf.add_object(dictionary! {
        "Type" => "Catalog", "Pages" => pages_id,
        "AcroForm" => dictionary! {"Fields" => vec![field_id.into()], "DR" => dictionary! {"Font" => dictionary! {"Helv" => font_id}}}
    });
    pdf.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();

    let res = app(AppState::new(config()))
        .oneshot(post_binary(
            "/v1/render",
            &[
                ("template", Some("form.pdf"), bytes),
                (
                    "data",
                    None,
                    br#"{"amount": 1500, "day": "2026-05-01"}"#.to_vec(),
                ),
                ("lang", None, b"de".to_vec()),
                ("currency", None, b"EUR".to_vec()),
                (
                    "translations",
                    None,
                    br#"{"de": {"total": "Summe"}}"#.to_vec(),
                ),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let filled = Pdf::load_mem(&bytes_of(res).await).unwrap();
    let field = filled.get_object(field_id).unwrap().as_dict().unwrap();
    let value = field.get(b"V").unwrap().as_str().unwrap();
    // The no-break spaces and the euro sign need UTF-16.
    let units: Vec<u16> = value[2..]
        .chunks(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    let value = String::from_utf16(&units).unwrap();
    assert!(value.starts_with("Summe 1.500,00\u{a0}€"), "{value}");
    assert!(value.contains("01 Mai"), "{value}");
}
