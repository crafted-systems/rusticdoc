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
        .oneshot(Request::get("/version").body(Body::empty()).unwrap())
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
    assert_eq!(res.status(), StatusCode::OK);

    let res = router
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(text(res).await.contains("rusticdoc_requests"));
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
        .oneshot(post_form(
            "/forms/libreoffice/convert",
            &[("files", Some("a.docx"), "PK")],
        ))
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
            "s3cret",
            headers
                .get(webhook::TIMESTAMP)
                .unwrap()
                .to_str()
                .unwrap()
                .parse()
                .unwrap(),
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
        "gotenberg-webhook-url",
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
async fn gotenberg_headers_rename_the_file_and_carry_the_trace() {
    let router = app(AppState::new(config()));
    let mut req = post_form("/v1/render", &[("template", Some("letter.txt"), "Hi")]);
    req.headers_mut()
        .insert("gotenberg-output-filename", "foo".parse().unwrap());
    req.headers_mut()
        .insert("gotenberg-trace", "trace-123".parse().unwrap());
    let res = router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_DISPOSITION).unwrap(),
        "attachment; filename=\"foo.txt\""
    );
    assert_eq!(res.headers().get("gotenberg-trace").unwrap(), "trace-123");
    assert_eq!(res.headers().get("x-request-id").unwrap(), "trace-123");

    let res = router
        .oneshot(Request::get("/favicon.ico").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
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
            "/forms/pdfengines/split",
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
            "/forms/pdfengines/metadata/read",
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
            "/forms/pdfengines/embed",
            &[
                ("files", Some("in.pdf"), three.clone()),
                ("files", Some("note.txt"), b"hello".to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let embedded = bytes_of(res).await;
    assert!(String::from_utf8_lossy(&embedded).contains("EmbeddedFile"));

    // Gotenberg's merge route with post-processing: merge + rotate + metadata in one call
    let res = router
        .clone()
        .oneshot(post_binary(
            "/forms/pdfengines/merge",
            &[
                ("files", Some("a.pdf"), three.clone()),
                ("files", Some("b.pdf"), fixtures::pages(2)),
                ("rotateAngle", None, b"180".to_vec()),
                ("metadata", None, br#"{"Title":"Merged"}"#.to_vec()),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{}", text(res).await);
    let merged = lopdf::Document::load_mem(&bytes_of(res).await).unwrap();
    assert_eq!(merged.get_pages().len(), 5);

    // bad requests stay 400 in Gotenberg's style
    let res = router
        .clone()
        .oneshot(post_binary(
            "/forms/pdfengines/rotate",
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
            "/forms/pdfengines/split",
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
async fn gotenberg_boolean_fields_are_strict() {
    let router = app(AppState::new(config()));
    let res = router
        .oneshot(post_form(
            "/forms/libreoffice/convert",
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
