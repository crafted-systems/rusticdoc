//! Stored templates and fonts over the router, with a store in a temporary
//! directory. Two `AppState`s on the same directory stand in for two replicas: they
//! share nothing else, as containers would not.

use std::io::Write as _;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use rusticdoc_core::Config;
use rusticdoc_core::config::StoreScope;
use rusticdoc_server::{AppState, app, fontsync};
use tower::ServiceExt;

const BOUNDARY: &str = "rusticdoc-store-boundary";

type Part<'a> = (&'a str, Option<&'a str>, &'a [u8]);

/// One container's configuration on a shared store, with its own fonts copy.
fn replica(store: &tempfile::TempDir, cache: &tempfile::TempDir) -> Config {
    Config {
        libreoffice_bin: None,
        qpdf_bin: None,
        pdfcpu_bin: None,
        log_format: "pretty".into(),
        store: Some(format!("file://{}", store.path().display())),
        fonts_dir: cache.path().join("mounted"),
        font_cache_dir: cache.path().join("copy"),
        store_sync_interval: Duration::from_secs(3600),
        ..Config::default()
    }
}

struct Pair {
    a: Router,
    b: Router,
    state_a: AppState,
    state_b: AppState,
    store: tempfile::TempDir,
    _caches: (tempfile::TempDir, tempfile::TempDir),
}

fn two_replicas(configure: impl Fn(&mut Config)) -> Pair {
    let store = tempfile::tempdir().unwrap();
    let (cache_a, cache_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut config_a = replica(&store, &cache_a);
    let mut config_b = replica(&store, &cache_b);
    configure(&mut config_a);
    configure(&mut config_b);
    let (state_a, state_b) = (AppState::new(config_a), AppState::new(config_b));
    Pair {
        a: app(state_a.clone()),
        b: app(state_b.clone()),
        state_a,
        state_b,
        store,
        _caches: (cache_a, cache_b),
    }
}

fn body_of(parts: &[Part<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, filename, content) in parts {
        write!(body, "--{BOUNDARY}\r\n").unwrap();
        match filename {
            Some(f) => write!(
                body,
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .unwrap(),
            None => write!(body, "Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").unwrap(),
        }
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    write!(body, "--{BOUNDARY}--\r\n").unwrap();
    body
}

fn post_as(path: &str, parts: &[Part<'_>], key: Option<&str>) -> Request<Body> {
    let mut req = Request::post(path).header(
        header::CONTENT_TYPE,
        format!("multipart/form-data; boundary={BOUNDARY}"),
    );
    if let Some(key) = key {
        req = req.header("x-api-key", key);
    }
    req.body(Body::from(body_of(parts))).unwrap()
}

fn post(path: &str, parts: &[Part<'_>]) -> Request<Body> {
    post_as(path, parts, None)
}

fn call(method: &str, path: &str, key: Option<&str>) -> Request<Body> {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(key) = key {
        req = req.header("x-api-key", key);
    }
    req.body(Body::empty()).unwrap()
}

async fn bytes(res: axum::response::Response) -> Vec<u8> {
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
}

async fn json(res: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&bytes(res).await).unwrap()
}

async fn store_template(router: &Router, key: Option<&str>) -> String {
    let parts: &[Part<'_>] = &[("template", Some("hello.txt"), b"Hello {{ name }}")];
    let res = router
        .clone()
        .oneshot(post_as("/v1/templates", parts, key))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    json(res).await["id"].as_str().unwrap().to_string()
}

async fn status(router: &Router, req: Request<Body>) -> StatusCode {
    router.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn upload_returns_an_id_that_renders_on_another_replica() {
    let pair = two_replicas(|_| {});
    let parts: &[Part<'_>] = &[("template", Some("hello.txt"), b"Hello {{ name }}")];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/templates", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let location = res.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string();
    let meta = json(res).await;
    let id = meta["id"].as_str().unwrap();
    assert_eq!(location, format!("/v1/templates/{id}"));
    assert_eq!(meta["name"], "hello.txt");
    assert_eq!(meta["format"], "txt");
    assert_eq!(meta["size"], 16);
    assert_eq!(meta["sha256"].as_str().unwrap().len(), 64);
    assert!(meta["createdAt"].as_str().unwrap().ends_with('Z'));

    let render: &[Part<'_>] = &[
        ("templateId", None, id.as_bytes()),
        ("data", None, br#"{"name":"Ada"}"#),
    ];
    let res = pair
        .b
        .clone()
        .oneshot(post("/v1/render", render))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(bytes(res).await, b"Hello Ada");
}

#[tokio::test]
async fn validate_and_batch_take_an_id_too() {
    let pair = two_replicas(|_| {});
    let id = store_template(&pair.a, None).await;

    let parts: &[Part<'_>] = &[("templateId", None, id.as_bytes()), ("data", None, b"{}")];
    let res = pair
        .b
        .clone()
        .oneshot(post("/v1/templates/validate", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        json(res).await["ok"],
        false,
        "'name' is missing from the data"
    );

    let parts: &[Part<'_>] = &[
        ("templateId", None, id.as_bytes()),
        (
            "data",
            None,
            br#"{"name":"All","people":[{"id":1},{"id":2}]}"#,
        ),
        ("splitBy", None, b"people"),
    ];
    let res = pair
        .b
        .clone()
        .oneshot(post("/v1/render/batch", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(bytes(res).await.starts_with(b"PK"));
}

#[tokio::test]
async fn a_file_and_an_id_together_are_refused() {
    let pair = two_replicas(|_| {});
    let id = store_template(&pair.a, None).await;
    let parts: &[Part<'_>] = &[
        ("template", Some("other.txt"), b"x"),
        ("templateId", None, id.as_bytes()),
    ];
    assert_eq!(
        status(&pair.a, post("/v1/render", parts)).await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn files_beside_an_id_stay_inputs_and_do_not_become_the_template() {
    let pair = two_replicas(|_| {});
    let id = store_template(&pair.a, None).await;
    let parts: &[Part<'_>] = &[
        ("templateId", None, id.as_bytes()),
        ("files", Some("extra.txt"), b"IGNORED {{ name }}"),
        ("data", None, br#"{"name":"Ada"}"#),
    ];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/render", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(bytes(res).await, b"Hello Ada");
}

#[tokio::test]
async fn list_pages_through_every_template() {
    let pair = two_replicas(|_| {});
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(store_template(&pair.a, None).await);
    }
    let first = json(
        pair.b
            .clone()
            .oneshot(call("GET", "/v1/templates?limit=2", None))
            .await
            .unwrap(),
    )
    .await;
    let cursor = first["nextCursor"].as_str().unwrap().to_string();
    let seen: Vec<_> = first["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(seen, ids[..2]);
    let rest = json(
        pair.b
            .clone()
            .oneshot(call(
                "GET",
                &format!("/v1/templates?limit=10&cursor={cursor}"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let seen: Vec<_> = rest["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(seen, ids[2..]);
    assert!(rest.get("nextCursor").is_none());
}

#[tokio::test]
async fn content_comes_back_as_a_download_byte_for_byte() {
    let pair = two_replicas(|_| {});
    let parts: &[Part<'_>] = &[("template", Some("page.html"), b"<script>alert(1)</script>")];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/templates", parts))
        .await
        .unwrap();
    let id = json(res).await["id"].as_str().unwrap().to_string();
    let res = pair
        .b
        .clone()
        .oneshot(call("GET", &format!("/v1/templates/{id}/content"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert!(
        res.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
    assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(bytes(res).await, b"<script>alert(1)</script>");
}

#[tokio::test]
async fn a_delete_is_seen_at_once_by_a_replica_that_has_it_cached() {
    let pair = two_replicas(|_| {});
    let id = store_template(&pair.a, None).await;
    let render: &[Part<'_>] = &[
        ("templateId", None, id.as_bytes()),
        ("data", None, br#"{"name":"Ada"}"#),
    ];
    // Replica B now holds the content in memory.
    assert_eq!(
        status(&pair.b, post("/v1/render", render)).await,
        StatusCode::OK
    );
    assert_eq!(
        status(
            &pair.a,
            call("DELETE", &format!("/v1/templates/{id}"), None)
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(&pair.b, post("/v1/render", render)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&pair.b, call("GET", &format!("/v1/templates/{id}"), None)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(
            &pair.a,
            call("DELETE", &format!("/v1/templates/{id}"), None)
        )
        .await,
        StatusCode::NOT_FOUND
    );
    // A template rendered by id is not left behind for 'templateHash' either.
    let hash = rusticdoc_core::template::template_hash(b"Hello {{ name }}");
    let recall: &[Part<'_>] = &[
        ("templateHash", None, hash.as_bytes()),
        ("data", None, br#"{"name":"A"}"#),
    ];
    assert_eq!(
        status(&pair.b, post("/v1/render", recall)).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn one_caller_cannot_touch_anothers_templates() {
    let pair = two_replicas(|c| c.api_keys = vec!["alice-key".into(), "bob-key".into()]);
    let id = store_template(&pair.a, Some("alice-key")).await;
    let bob = Some("bob-key");
    for (method, path) in [
        ("GET", format!("/v1/templates/{id}")),
        ("GET", format!("/v1/templates/{id}/content")),
        ("DELETE", format!("/v1/templates/{id}")),
    ] {
        assert_eq!(
            status(&pair.b, call(method, &path, bob)).await,
            StatusCode::NOT_FOUND,
            "{method} {path}"
        );
    }
    let render: &[Part<'_>] = &[("templateId", None, id.as_bytes())];
    assert_eq!(
        status(&pair.b, post_as("/v1/render", render, bob)).await,
        StatusCode::NOT_FOUND
    );
    let listed = json(
        pair.b
            .clone()
            .oneshot(call("GET", "/v1/templates", bob))
            .await
            .unwrap(),
    )
    .await;
    assert!(listed["templates"].as_array().unwrap().is_empty());

    // Still there for its owner, and nothing at all without a credential.
    assert_eq!(
        status(
            &pair.a,
            call("GET", &format!("/v1/templates/{id}"), Some("alice-key"))
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        status(&pair.a, call("GET", "/v1/templates", None)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_shared_scope_shares() {
    let pair = two_replicas(|c| {
        c.api_keys = vec!["old-key".into(), "new-key".into()];
        c.store_scope = StoreScope::Shared;
    });
    let id = store_template(&pair.a, Some("old-key")).await;
    assert_eq!(
        status(
            &pair.b,
            call("GET", &format!("/v1/templates/{id}"), Some("new-key"))
        )
        .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn ids_cannot_reach_outside_the_store() {
    let pair = two_replicas(|_| {});
    std::fs::write(pair.store.path().join("secret.json"), b"{}").unwrap();
    for id in [
        "..%2F..%2F..%2Fsecret",
        "%2e%2e%2fsecret",
        "not-a-uuid",
        "01966F3E-7C1A-7D3C-9A44-1D2F5A6B7C8D",
        "..",
    ] {
        let res = status(&pair.a, call("GET", &format!("/v1/templates/{id}"), None)).await;
        assert_eq!(res, StatusCode::NOT_FOUND, "{id}");
        let res = status(&pair.a, call("DELETE", &format!("/v1/fonts/{id}"), None)).await;
        assert_eq!(res, StatusCode::NOT_FOUND, "{id}");
    }
    let parts: &[Part<'_>] = &[("templateId", None, b"../../secret")];
    assert_eq!(
        status(&pair.a, post("/v1/render", parts)).await,
        StatusCode::NOT_FOUND
    );
    assert!(pair.store.path().join("secret.json").exists());
}

#[tokio::test]
async fn altered_content_is_not_served() {
    let pair = two_replicas(|_| {});
    let id = store_template(&pair.a, None).await;
    let scope = rusticdoc_core::store::scope_key("public");
    let path = pair
        .store
        .path()
        .join(format!("templates/{scope}/content/{id}"));
    std::fs::write(&path, b"Hello {{ altered }}").unwrap();
    // Replica A cached nothing: it has only written.
    let res = pair
        .a
        .clone()
        .oneshot(call("GET", &format!("/v1/templates/{id}/content"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = String::from_utf8(bytes(res).await).unwrap();
    assert!(body.contains("store_integrity"), "{body}");
    assert!(!body.contains("altered"), "{body}");
}

#[tokio::test]
async fn templates_are_checked_before_anything_is_stored() {
    let pair = two_replicas(|c| c.max_template_bytes = 64);
    let stored = |dir: &std::path::Path| dir.join("templates").exists();

    let parts: &[Part<'_>] = &[("template", Some("x.exe"), b"MZ")];
    assert_eq!(
        status(&pair.a, post("/v1/templates", parts)).await,
        StatusCode::BAD_REQUEST
    );
    let parts: &[Part<'_>] = &[("template", Some("x.pdf"), b"not a pdf")];
    assert_eq!(
        status(&pair.a, post("/v1/templates", parts)).await,
        StatusCode::BAD_REQUEST
    );
    let parts: &[Part<'_>] = &[("template", Some("x.docx"), b"not a zip")];
    assert_eq!(
        status(&pair.a, post("/v1/templates", parts)).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let parts: &[Part<'_>] = &[("template", Some("x.txt"), b"{% for %}")];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/templates", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json(res).await["error"], "template_error");
    let big = vec![b'x'; 65];
    let parts: &[Part<'_>] = &[("template", Some("x.txt"), &big)];
    assert_eq!(
        status(&pair.a, post("/v1/templates", parts)).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let parts: &[Part<'_>] = &[("name", None, b"no file")];
    assert_eq!(
        status(&pair.a, post("/v1/templates", parts)).await,
        StatusCode::BAD_REQUEST
    );
    assert!(
        !stored(pair.store.path()),
        "nothing should have been written"
    );

    // A value the data will supply later is no fault at upload.
    let parts: &[Part<'_>] = &[("template", Some("x.txt"), b"{{ later.on }}")];
    assert_eq!(
        status(&pair.a, post("/v1/templates", parts)).await,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn a_chosen_name_never_changes_the_format() {
    let pair = two_replicas(|_| {});
    let parts: &[Part<'_>] = &[
        ("template", Some("a.txt"), b"x"),
        ("name", None, b"../../invoice.html"),
    ];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/templates", parts))
        .await
        .unwrap();
    let meta = json(res).await;
    assert_eq!(meta["format"], "txt");
    let name = meta["name"].as_str().unwrap();
    assert!(name.ends_with(".txt") && !name.contains('/'), "{name}");
}

#[tokio::test]
async fn quotas_hold_per_caller() {
    let pair = two_replicas(|c| {
        c.api_keys = vec!["alice".into(), "bob".into()];
        c.max_templates = 1;
    });
    store_template(&pair.a, Some("alice")).await;
    let parts: &[Part<'_>] = &[("template", Some("b.txt"), b"x")];
    let res = pair
        .b
        .clone()
        .oneshot(post_as("/v1/templates", parts, Some("alice")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(json(res).await["error"], "quota_exceeded");
    store_template(&pair.a, Some("bob")).await;

    let pair = two_replicas(|c| c.max_template_storage = 20);
    store_template(&pair.a, None).await;
    let parts: &[Part<'_>] = &[("template", Some("b.txt"), b"this one passes the limit")];
    assert_eq!(
        status(&pair.a, post("/v1/templates", parts)).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn without_a_store_the_routes_say_so_and_nothing_else_changes() {
    let config = Config {
        libreoffice_bin: None,
        qpdf_bin: None,
        pdfcpu_bin: None,
        log_format: "pretty".into(),
        ..Config::default()
    };
    let router = app(AppState::new(config));
    let parts: &[Part<'_>] = &[("template", Some("a.txt"), b"x")];
    let res = router
        .clone()
        .oneshot(post("/v1/templates", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(json(res).await["error"], "store_not_configured");
    let parts: &[Part<'_>] = &[("file", Some("a.ttf"), &minimal_font("X"))];
    assert_eq!(
        status(&router, post("/v1/fonts", parts)).await,
        StatusCode::NOT_IMPLEMENTED
    );
    let parts: &[Part<'_>] = &[("templateId", None, b"x")];
    assert_eq!(
        status(&router, post("/v1/render", parts)).await,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        status(&router, call("GET", "/ready", None)).await,
        StatusCode::OK
    );
    let parts: &[Part<'_>] = &[("template", Some("a.txt"), b"plain")];
    assert_eq!(
        status(&router, post("/v1/render", parts)).await,
        StatusCode::OK
    );
}

/// The smallest font the parsers accept: an sfnt with the tables they require and
/// a name table giving it `family`.
fn minimal_font(family: &str) -> Vec<u8> {
    let be16 = |v: u16| v.to_be_bytes().to_vec();
    let be32 = |v: u32| v.to_be_bytes().to_vec();
    let mut head = Vec::new();
    head.extend(be32(0x0001_0000)); // version
    head.extend(be32(0x0001_0000)); // font revision
    head.extend(be32(0)); // checksum adjustment
    head.extend(be32(0x5F0F_3CF5)); // magic
    head.extend(be16(0)); // flags
    head.extend(be16(1000)); // units per em
    head.extend([0u8; 16]); // created, modified
    head.extend([0u8; 8]); // bounding box
    head.extend(be16(0)); // style
    head.extend(be16(8)); // smallest readable size
    head.extend(be16(2)); // direction hint
    head.extend(be16(0)); // short offsets
    head.extend(be16(0)); // glyph data format
    let mut hhea = Vec::new();
    hhea.extend(be32(0x0001_0000));
    hhea.extend(be16(800)); // ascender
    hhea.extend(be16(0xFF38)); // descender
    hhea.extend([0u8; 26]);
    hhea.extend(be16(1)); // long metrics
    let mut maxp = Vec::new();
    maxp.extend(be32(0x0000_5000));
    maxp.extend(be16(1));
    let hmtx = [be16(500), be16(0)].concat();
    let encoded: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut name = Vec::new();
    name.extend(be16(0));
    name.extend(be16(2));
    name.extend(be16(6 + 2 * 12));
    // The family and the PostScript name, which the parser insists on, share a string.
    for id in [1, 6] {
        name.extend(be16(3)); // Windows
        name.extend(be16(1)); // Unicode
        name.extend(be16(0x0409)); // en-US
        name.extend(be16(id));
        name.extend(be16(encoded.len() as u16));
        name.extend(be16(0));
    }
    name.extend(encoded);
    let tables: [(&[u8; 4], Vec<u8>); 5] = [
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"maxp", maxp),
        (b"name", name),
    ];
    let mut font = Vec::new();
    font.extend(be32(0x0001_0000));
    font.extend(be16(tables.len() as u16));
    font.extend([0u8; 6]);
    let mut offset = 12 + 16 * tables.len() as u32;
    for (tag, data) in &tables {
        font.extend_from_slice(*tag);
        font.extend(be32(0));
        font.extend(be32(offset));
        font.extend(be32(data.len() as u32));
        offset += (data.len() as u32).div_ceil(4) * 4;
    }
    for (_, data) in &tables {
        font.extend_from_slice(data);
        font.resize(font.len().div_ceil(4) * 4, 0);
    }
    font
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }
    out
}

async fn font_families(router: &Router) -> Vec<String> {
    let res = router
        .clone()
        .oneshot(call("GET", "/v1/fonts", None))
        .await
        .unwrap();
    json(res)
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["family"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_zip_of_fonts_reaches_the_other_replica_at_its_next_sync_and_leaves_with_a_delete() {
    let pair = two_replicas(|_| {});
    let (one, two) = (
        minimal_font("Replicated One"),
        minimal_font("Replicated Two"),
    );
    let zip = zip_of(&[
        ("fonts/one.ttf", &one),
        ("fonts/two.TTF", &two),
        ("fonts/again.ttf", &one),
        ("fonts/LICENSE.txt", b"terms"),
        ("fonts/web.woff2", b"wOF2"),
        ("__MACOSX/._one.ttf", b"junk"),
        (".DS_Store", b"junk"),
    ]);
    let parts: &[Part<'_>] = &[("file", Some("pack.zip"), &zip)];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/fonts", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let stored = json(res).await;
    let fonts = stored["fonts"].as_array().unwrap();
    assert_eq!(fonts.len(), 2, "{stored}");
    assert_eq!(fonts[0]["file"], "one.ttf");
    assert_eq!(fonts[0]["faces"][0]["family"], "Replicated One");
    let id = fonts[0]["id"].as_str().unwrap().to_string();
    let skipped: Vec<_> = stored["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["file"].as_str().unwrap())
        .collect();
    assert_eq!(skipped, ["LICENSE.txt", "web.woff2"]);

    // In use where it was uploaded, and not yet where it has not synced.
    assert!(
        font_families(&pair.a)
            .await
            .contains(&"Replicated One".to_string())
    );
    assert!(
        !font_families(&pair.b)
            .await
            .contains(&"Replicated One".to_string())
    );
    fontsync::run_once(&pair.state_b).await.unwrap();
    assert!(
        font_families(&pair.b)
            .await
            .contains(&"Replicated One".to_string())
    );

    // Listed under the name it was uploaded as, with the id to delete it by.
    let listed = json(
        pair.b
            .clone()
            .oneshot(call("GET", "/v1/fonts", None))
            .await
            .unwrap(),
    )
    .await;
    let face = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["family"] == "Replicated One")
        .unwrap()
        .clone();
    assert_eq!(face["file"], "one.ttf");
    assert_eq!(face["id"], id.as_str());
    assert_eq!(face["mounted"], false);

    assert_eq!(
        status(&pair.b, call("DELETE", &format!("/v1/fonts/{id}"), None)).await,
        StatusCode::NO_CONTENT
    );
    assert!(
        !font_families(&pair.b)
            .await
            .contains(&"Replicated One".to_string())
    );
    fontsync::run_once(&pair.state_a).await.unwrap();
    assert!(
        !font_families(&pair.a)
            .await
            .contains(&"Replicated One".to_string())
    );
    assert!(
        font_families(&pair.a)
            .await
            .contains(&"Replicated Two".to_string())
    );
    assert_eq!(
        status(&pair.b, call("DELETE", &format!("/v1/fonts/{id}"), None)).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn one_font_file_needs_no_zip_and_storing_it_twice_changes_nothing() {
    let pair = two_replicas(|_| {});
    let font = minimal_font("Single");
    let parts: &[Part<'_>] = &[("file", Some("single.ttf"), &font)];
    let first = json(
        pair.a
            .clone()
            .oneshot(post("/v1/fonts", parts))
            .await
            .unwrap(),
    )
    .await;
    let second = json(
        pair.b
            .clone()
            .oneshot(post("/v1/fonts", parts))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first["fonts"][0]["id"], second["fonts"][0]["id"]);
    let store =
        rusticdoc_core::store::Store::open(&format!("file://{}", pair.store.path().display()))
            .unwrap();
    assert_eq!(store.font_usage().await.unwrap().count, 1);
}

#[tokio::test]
async fn a_hostile_zip_stores_nothing_and_writes_nowhere_else() {
    let pair = two_replicas(|_| {});
    let font = minimal_font("Named Oddly");
    // Names are labels: this must land in the store under its hash, nowhere else.
    let zip = zip_of(&[("../../../../tmp/rusticdoc-escaped.ttf", &font)]);
    let parts: &[Part<'_>] = &[("file", Some("pack.zip"), &zip)];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/fonts", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let stored = json(res).await;
    assert_eq!(stored["fonts"][0]["file"], "rusticdoc-escaped.ttf");
    assert!(!std::path::Path::new("/tmp/rusticdoc-escaped.ttf").exists());

    for (name, zip) in [
        (
            "not a font, named as one",
            zip_of(&[("a.ttf", b"not a font at all")]),
        ),
        (
            "magic bytes and nothing else",
            zip_of(&[("a.ttf", &[0, 1, 0, 0, 0, 0][..])]),
        ),
        (
            "nothing that is a font",
            zip_of(&[("readme.txt", b"hello")]),
        ),
        ("nothing at all", zip_of(&[])),
    ] {
        let parts: &[Part<'_>] = &[("file", Some("pack.zip"), &zip)];
        assert_eq!(
            status(&pair.a, post("/v1/fonts", parts)).await,
            StatusCode::BAD_REQUEST,
            "{name}"
        );
    }
    let parts: &[Part<'_>] = &[("file", Some("pack.zip"), b"PK but not a zip")];
    assert_eq!(
        status(&pair.a, post("/v1/fonts", parts)).await,
        StatusCode::BAD_REQUEST
    );
    let parts: &[Part<'_>] = &[("file", Some("x.exe"), b"MZ")];
    assert_eq!(
        status(&pair.a, post("/v1/fonts", parts)).await,
        StatusCode::BAD_REQUEST
    );

    // Far more than it may expand to, from a small upload: refused, not unpacked.
    let huge = vec![0u8; 300 * 1024 * 1024];
    let bomb = zip_of(&[("big.ttf", &huge)]);
    assert!(bomb.len() < 2 * 1024 * 1024);
    let parts: &[Part<'_>] = &[("file", Some("bomb.zip"), &bomb)];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/fonts", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(
        String::from_utf8(bytes(res).await)
            .unwrap()
            .contains("larger than")
    );
    let store =
        rusticdoc_core::store::Store::open(&format!("file://{}", pair.store.path().display()))
            .unwrap();
    assert_eq!(store.font_usage().await.unwrap().count, 1);
}

#[tokio::test]
async fn fonts_change_only_with_an_admin_key() {
    let pair = two_replicas(|c| {
        c.api_keys = vec!["caller".into()];
        c.admin_keys = vec!["admin".into()];
    });
    let font = minimal_font("Guarded");
    let parts: &[Part<'_>] = &[("file", Some("g.ttf"), &font)];
    assert_eq!(
        status(&pair.a, post_as("/v1/fonts", parts, None)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&pair.a, post_as("/v1/fonts", parts, Some("caller"))).await,
        StatusCode::FORBIDDEN
    );
    let res = pair
        .a
        .clone()
        .oneshot(post_as("/v1/fonts", parts, Some("admin")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let id = json(res).await["fonts"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/v1/fonts/{id}");
    assert_eq!(
        status(&pair.a, call("DELETE", &path, Some("caller"))).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(&pair.a, call("GET", "/v1/fonts", Some("caller"))).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&pair.a, call("DELETE", &path, Some("admin"))).await,
        StatusCode::NO_CONTENT
    );

    // Credentials but no admin keys: nobody, not everybody.
    let pair = two_replicas(|c| c.api_keys = vec!["caller".into()]);
    let res = pair
        .a
        .clone()
        .oneshot(post_as("/v1/fonts", parts, Some("caller")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(
        String::from_utf8(bytes(res).await)
            .unwrap()
            .contains("RUSTICDOC_ADMIN_KEYS")
    );

    // An admin key on an otherwise open service still guards fonts.
    let pair = two_replicas(|c| c.admin_keys = vec!["admin".into()]);
    assert_eq!(
        status(&pair.a, post_as("/v1/fonts", parts, None)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(&pair.a, post_as("/v1/fonts", parts, Some("admin"))).await,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn the_font_quota_holds() {
    let pair = two_replicas(|c| c.max_stored_fonts = 1);
    let (one, two) = (minimal_font("One"), minimal_font("Two"));
    let parts: &[Part<'_>] = &[("file", Some("1.ttf"), &one)];
    assert_eq!(
        status(&pair.a, post("/v1/fonts", parts)).await,
        StatusCode::CREATED
    );
    let parts: &[Part<'_>] = &[("file", Some("2.ttf"), &two)];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/fonts", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(json(res).await["error"], "quota_exceeded");
}

#[tokio::test]
async fn a_replica_is_not_ready_until_its_fonts_are_in_hand() {
    let pair = two_replicas(|_| {});
    assert_eq!(
        status(&pair.a, call("GET", "/ready", None)).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        status(&pair.a, call("GET", "/health", None)).await,
        StatusCode::OK
    );
    fontsync::run_once(&pair.state_a).await.unwrap();
    assert_eq!(
        status(&pair.a, call("GET", "/ready", None)).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_corrupt_font_is_left_out_and_the_rest_still_arrive() {
    let pair = two_replicas(|_| {});
    let (good, bad) = (minimal_font("Good"), minimal_font("Bad"));
    let parts: &[Part<'_>] = &[("file", Some("good.ttf"), &good)];
    assert_eq!(
        status(&pair.a, post("/v1/fonts", parts)).await,
        StatusCode::CREATED
    );
    let parts: &[Part<'_>] = &[("file", Some("bad.ttf"), &bad)];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/fonts", parts))
        .await
        .unwrap();
    let id = json(res).await["fonts"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(
        pair.store.path().join(format!("fonts/data/{id}")),
        minimal_font("Forged"),
    )
    .unwrap();

    fontsync::run_once(&pair.state_b).await.unwrap();
    let families = font_families(&pair.b).await;
    assert!(families.contains(&"Good".to_string()), "{families:?}");
    assert!(!families.contains(&"Forged".to_string()), "{families:?}");
    assert_eq!(
        status(&pair.b, call("GET", "/ready", None)).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_sync_touches_only_what_it_wrote() {
    let pair = two_replicas(|_| {});
    let copy = pair.state_a.config.font_cache_dir.clone();
    std::fs::create_dir_all(&copy).unwrap();
    let theirs = copy.join("someone-elses.ttf");
    std::fs::write(&theirs, b"keep me").unwrap();
    fontsync::run_once(&pair.state_a).await.unwrap();
    assert!(theirs.exists());
}

#[test]
fn a_store_url_cannot_hold_secrets_or_be_something_else() {
    use rusticdoc_core::store::Store;
    for url in [
        "s3://key:secret@bucket/prefix",
        "s3://bucket?secret=1",
        "s3://",
        "https://example.com/bucket",
        "ftp://example.com",
        "relative/path",
    ] {
        assert!(Store::open(url).is_err(), "{url}");
    }
    let error = Store::open("s3://key:hunter2@bucket").unwrap_err();
    assert!(!error.contains("hunter2"), "{error}");
    assert_eq!(
        Store::open("s3://bucket/prefix").unwrap().description(),
        "s3://bucket"
    );
}

#[test]
fn the_default_port_is_9027() {
    let config = Config::default();
    assert_eq!(config.bind, "0.0.0.0:9027");
}

#[tokio::test]
async fn health_names_the_store_only_when_there_is_one() {
    let pair = two_replicas(|_| {});
    let res = pair
        .a
        .clone()
        .oneshot(call("GET", "/health", None))
        .await
        .unwrap();
    assert_eq!(json(res).await["details"]["store"]["fontsSynced"], false);

    let config = Config {
        libreoffice_bin: None,
        qpdf_bin: None,
        pdfcpu_bin: None,
        log_format: "pretty".into(),
        ..Config::default()
    };
    let res = app(AppState::new(config))
        .oneshot(call("GET", "/health", None))
        .await
        .unwrap();
    let health = json(res).await;
    assert!(health["details"].get("store").is_none(), "{health}");
}

// --- JSON bodies, aliases, variables, previews and background jobs ---

fn send_json(
    method: &str,
    path: &str,
    body: serde_json::Value,
    key: Option<&str>,
) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        req = req.header("x-api-key", key);
    }
    req.body(Body::from(body.to_string())).unwrap()
}

fn post_json(path: &str, body: serde_json::Value, key: Option<&str>) -> Request<Body> {
    send_json("POST", path, body, key)
}

async fn store_text(router: &Router, name: &str, content: &[u8], key: Option<&str>) -> String {
    let parts: &[Part<'_>] = &[("template", Some(name), content)];
    let res = router
        .clone()
        .oneshot(post_as("/v1/templates", parts, key))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    json(res).await["id"].as_str().unwrap().to_string()
}

async fn set_alias(
    router: &Router,
    name: &str,
    id: &str,
    key: Option<&str>,
) -> axum::response::Response {
    router
        .clone()
        .oneshot(send_json(
            "PUT",
            &format!("/v1/aliases/{name}"),
            serde_json::json!({ "templateId": id }),
            key,
        ))
        .await
        .unwrap()
}

async fn render_text(router: &Router, template: &str, key: Option<&str>) -> (StatusCode, String) {
    let res = router
        .clone()
        .oneshot(post_json(
            "/v1/render",
            serde_json::json!({ "templateId": template, "data": { "name": "Ada" } }),
            key,
        ))
        .await
        .unwrap();
    let status = res.status();
    (
        status,
        String::from_utf8_lossy(&bytes(res).await).into_owned(),
    )
}

#[tokio::test]
async fn a_json_body_renders_validates_and_batches_by_id() {
    let pair = two_replicas(|_| {});
    let id = store_template(&pair.a, None).await;
    let (status, body) = render_text(&pair.b, &id, None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "Hello Ada"));

    // Options mean what they mean on the form: here a name for the result.
    let res = pair
        .b
        .clone()
        .oneshot(post_json(
            "/v1/render",
            serde_json::json!({ "templateId": id, "data": { "name": "Ada" }, "outputFilename": "greeting" }),
            None,
        ))
        .await
        .unwrap();
    assert!(
        res.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("greeting.txt")
    );

    let res = pair
        .b
        .clone()
        .oneshot(post_json(
            "/v1/templates/validate",
            serde_json::json!({ "templateId": id, "data": {} }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json(res).await["ok"], false);

    let res = pair
        .b
        .clone()
        .oneshot(post_json(
            "/v1/render/batch",
            serde_json::json!({
                "templateId": id,
                "data": { "name": "All", "people": [{ "id": 1 }, { "id": 2 }] },
                "splitBy": "people",
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(bytes(res).await.starts_with(b"PK"));
}

#[tokio::test]
async fn a_json_body_gets_the_forms_checks() {
    let pair = two_replicas(|_| {});
    let id = store_template(&pair.a, None).await;
    let refused = |body: serde_json::Value| post_json("/v1/render", body, None);
    for (body, says) in [
        (
            serde_json::json!(["not", "an", "object"]),
            "object of fields",
        ),
        (
            serde_json::json!({ "template": "x", "templateId": id }),
            "cannot carry files",
        ),
        (
            serde_json::json!({ "templateId": id, "steps": ["teleport"] }),
            "teleport",
        ),
        (
            serde_json::json!({ "templateId": id, "steps": [1] }),
            "list of step names",
        ),
        (
            serde_json::json!({ "data": { "name": "Ada" } }),
            "templateId",
        ),
        (
            serde_json::json!({ "templateId": id, "data": "not json" }),
            "not valid JSON",
        ),
    ] {
        let res = pair.a.clone().oneshot(refused(body.clone())).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body}");
        let text = String::from_utf8(bytes(res).await).unwrap();
        assert!(text.contains(says), "{body}: {text}");
    }
    let many: serde_json::Map<String, serde_json::Value> = (0..2001)
        .map(|i| (format!("f{i}"), serde_json::Value::from(i)))
        .collect();
    assert_eq!(
        status(&pair.a, refused(serde_json::Value::Object(many))).await,
        StatusCode::BAD_REQUEST
    );
    let res = pair
        .a
        .clone()
        .oneshot(
            Request::post("/v1/render")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{ not json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // The body limit holds for JSON as for forms.
    let small = two_replicas(|c| c.body_limit = 64);
    let big = serde_json::json!({ "templateId": id, "data": { "pad": "x".repeat(200) } });
    assert_eq!(
        status(&small.a, post_json("/v1/render", big, None)).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn an_alias_keeps_its_versions_and_renders_any_of_them() {
    let pair = two_replicas(|_| {});
    let first = store_text(&pair.a, "v1.txt", b"One {{ name }}", None).await;
    let second = store_text(&pair.a, "v2.txt", b"Two {{ name }}", None).await;

    let res = set_alias(&pair.a, "invoice", &first, None).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(res.headers()[header::LOCATION], "/v1/aliases/invoice");
    let set = json(res).await;
    assert_eq!(
        (set["version"].as_u64(), set["templateId"].as_str()),
        (Some(1), Some(first.as_str()))
    );
    assert_eq!(render_text(&pair.b, "invoice", None).await.1, "One Ada");

    // A new version, seen by the other replica at once.
    let res = set_alias(&pair.a, "invoice", &second, None).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json(res).await["version"], 2);
    assert_eq!(render_text(&pair.b, "invoice", None).await.1, "Two Ada");
    assert_eq!(render_text(&pair.b, "invoice@1", None).await.1, "One Ada");
    assert_eq!(render_text(&pair.b, "invoice@2", None).await.1, "Two Ada");
    assert_eq!(
        render_text(&pair.b, "invoice@3", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        render_text(&pair.b, "invoice@x", None).await.0,
        StatusCode::BAD_REQUEST
    );

    let history = json(
        pair.b
            .clone()
            .oneshot(call("GET", "/v1/aliases/invoice", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(history["templateId"], second.as_str());
    assert_eq!(history["version"], 2);
    let versions = history["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["templateId"], first.as_str());
    assert_eq!(versions[0]["version"], 1);
    assert_eq!(versions[1]["templateId"], second.as_str());

    // The template routes take an alias too, and say which template it is.
    let meta = json(
        pair.b
            .clone()
            .oneshot(call("GET", "/v1/templates/invoice@1", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(meta["id"], first.as_str());

    // An alias may name another alias's template.
    assert_eq!(
        set_alias(&pair.a, "latest", "invoice", None).await.status(),
        StatusCode::CREATED
    );
    let listed = json(
        pair.a
            .clone()
            .oneshot(call("GET", "/v1/aliases", None))
            .await
            .unwrap(),
    )
    .await;
    let names: Vec<(&str, u64)> = listed["aliases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["name"].as_str().unwrap(), a["version"].as_u64().unwrap()))
        .collect();
    assert_eq!(names, [("invoice", 2), ("latest", 1)]);
    let page = json(
        pair.a
            .clone()
            .oneshot(call("GET", "/v1/aliases?limit=1", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(page["aliases"].as_array().unwrap().len(), 1);
    assert_eq!(page["nextCursor"], "invoice");

    // Deleting the template an alias names says so where it is used.
    assert_eq!(
        status(
            &pair.a,
            call("DELETE", &format!("/v1/templates/{second}"), None)
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let (status_now, body) = render_text(&pair.b, "invoice", None).await;
    assert_eq!(status_now, StatusCode::NOT_FOUND);
    assert!(body.contains("has been deleted"), "{body}");
    assert_eq!(render_text(&pair.b, "invoice@1", None).await.1, "One Ada");

    // Deleting the alias ends it everywhere, and setting it again starts over.
    assert_eq!(
        status(&pair.b, call("DELETE", "/v1/aliases/invoice", None)).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(&pair.a, call("GET", "/v1/aliases/invoice", None)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        render_text(&pair.a, "invoice", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        render_text(&pair.a, "invoice@1", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&pair.a, call("DELETE", "/v1/aliases/invoice", None)).await,
        StatusCode::NOT_FOUND
    );
    let again = set_alias(&pair.a, "invoice", &first, None).await;
    assert_eq!(again.status(), StatusCode::CREATED);
    assert_eq!(json(again).await["version"], 1);
    let history = json(
        pair.a
            .clone()
            .oneshot(call("GET", "/v1/aliases/invoice", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(history["versions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn alias_names_are_checked_and_cannot_reach_outside_the_store() {
    let pair = two_replicas(|_| {});
    let id = store_template(&pair.a, None).await;
    for name in [
        "01966f3e-7c1a-7d3c-9a44-1d2f5a6b7c8d",
        ".hidden",
        "-x",
        "a%2Fb",
        "x".repeat(65).as_str(),
        "has%20space",
    ] {
        let res = set_alias(&pair.a, name, &id, None).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{name}");
    }
    assert_eq!(
        set_alias(&pair.a, "ok", "", None).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        set_alias(&pair.a, "ok", "01966f3e-7c1a-7d3c-9a44-1d2f5a6b7c8d", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        set_alias(&pair.a, "ok", "missing", None).await.status(),
        StatusCode::NOT_FOUND
    );
    for name in ["..", "a..b%2F", "%2E%2E"] {
        assert_eq!(
            render_text(&pair.a, name, None).await.0,
            StatusCode::NOT_FOUND,
            "{name}"
        );
    }
}

#[tokio::test]
async fn one_caller_cannot_see_or_use_anothers_aliases() {
    let pair = two_replicas(|c| c.api_keys = vec!["alice-key".into(), "bob-key".into()]);
    let (alice, bob) = (Some("alice-key"), Some("bob-key"));
    let id = store_template(&pair.a, alice).await;
    assert_eq!(
        set_alias(&pair.a, "invoice", &id, alice).await.status(),
        StatusCode::CREATED
    );

    assert_eq!(
        render_text(&pair.b, "invoice", bob).await.0,
        StatusCode::NOT_FOUND
    );
    for (method, path) in [
        ("GET", "/v1/aliases/invoice"),
        ("DELETE", "/v1/aliases/invoice"),
        ("GET", "/v1/templates/invoice"),
        ("GET", "/v1/templates/invoice/variables"),
    ] {
        assert_eq!(
            status(&pair.b, call(method, path, bob)).await,
            StatusCode::NOT_FOUND,
            "{method} {path}"
        );
    }
    let listed = json(
        pair.b
            .clone()
            .oneshot(call("GET", "/v1/aliases", bob))
            .await
            .unwrap(),
    )
    .await;
    assert!(listed["aliases"].as_array().unwrap().is_empty());
    // Bob cannot point a name of his own at Alice's template either.
    assert_eq!(
        set_alias(&pair.b, "mine", &id, bob).await.status(),
        StatusCode::NOT_FOUND
    );
    // Bob's own alias of the same name is his.
    let bobs = store_text(&pair.b, "b.txt", b"Bob {{ name }}", bob).await;
    assert_eq!(
        set_alias(&pair.b, "invoice", &bobs, bob).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(render_text(&pair.a, "invoice", alice).await.1, "Hello Ada");
    assert_eq!(render_text(&pair.b, "invoice", bob).await.1, "Bob Ada");
}

#[tokio::test]
async fn aliases_count_against_the_template_limit() {
    let pair = two_replicas(|c| c.max_templates = 2);
    let id = store_template(&pair.a, None).await;
    assert_eq!(
        set_alias(&pair.a, "one", &id, None).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        set_alias(&pair.a, "two", &id, None).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        set_alias(&pair.a, "three", &id, None).await.status(),
        StatusCode::FORBIDDEN
    );
    // A new version of an existing alias is not a new alias.
    assert_eq!(
        set_alias(&pair.a, "one", &id, None).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn variables_list_paths_loops_and_item_fields() {
    let pair = two_replicas(|_| {});
    let template = b"Dear {{ customer.name }},\n{% for line in lines %}{{ line.sku }} x {{ line.qty }}\n{% endfor %}{{ total | round(2) }}";
    let id = store_text(&pair.a, "order.txt", template, None).await;
    assert_eq!(
        set_alias(&pair.a, "order", &id, None).await.status(),
        StatusCode::CREATED
    );
    for reference in [id.as_str(), "order", "order@1"] {
        let res = pair
            .b
            .clone()
            .oneshot(call(
                "GET",
                &format!("/v1/templates/{reference}/variables"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{reference}");
        let found = json(res).await;
        assert_eq!(found["templateId"], id.as_str());
        assert_eq!(found["format"], "txt");
        let variables: Vec<(String, String)> = found["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["path"].as_str().unwrap().into(),
                    v["type"].as_str().unwrap().into(),
                )
            })
            .collect();
        for (path, kind) in [
            ("customer", "object"),
            ("customer.name", "value"),
            ("lines", "array"),
            ("lines[].sku", "value"),
            ("lines[].qty", "value"),
            ("total", "value"),
        ] {
            assert!(
                variables.contains(&(path.into(), kind.into())),
                "{path} in {variables:?}"
            );
        }
        assert!(!variables.iter().any(|(p, _)| p.starts_with("line.")));
        assert_eq!(
            found["example"],
            serde_json::json!({"customer": {"name": ""}, "lines": [{"sku": "", "qty": ""}], "total": ""})
        );
    }
    assert_eq!(
        status(
            &pair.a,
            call("GET", "/v1/templates/missing/variables", None)
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_pdf_forms_variables_are_its_fields() {
    let pair = two_replicas(|_| {});
    let form = rusticdoc_core::pdfops::fixtures::form();
    let id = store_text(&pair.a, "form.pdf", &form, None).await;
    let res = pair
        .a
        .clone()
        .oneshot(call("GET", &format!("/v1/templates/{id}/variables"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let found = json(res).await;
    assert_eq!(found["format"], "pdf");
    assert!(
        !found["variables"].as_array().unwrap().is_empty(),
        "{found}"
    );
}

#[tokio::test]
async fn a_preview_without_pdftoppm_is_a_503_before_any_work() {
    let pair = two_replicas(|c| c.pdftoppm_bin = None);
    let id = store_template(&pair.a, None).await;
    let res = pair
        .a
        .clone()
        .oneshot(post_json(
            &format!("/v1/templates/{id}/preview"),
            serde_json::json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(res).await["error"], "engine_unavailable");
}

/// The width and height a PNG says it has.
fn png_size(png: &[u8]) -> (u32, u32) {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
    (w, h)
}

#[tokio::test]
async fn a_preview_draws_the_page_asked_for_within_limits() {
    let pair = two_replicas(|_| {});
    if pair.state_a.config.pdftoppm_bin.is_none() {
        eprintln!("skipped: no pdftoppm here");
        return;
    }
    let id = store_text(&pair.a, "page.html", b"<h1>Hello {{ name }}</h1>", None).await;
    assert_eq!(
        set_alias(&pair.a, "page", &id, None).await.status(),
        StatusCode::CREATED
    );
    let preview = |body: serde_json::Value| post_json("/v1/templates/page/preview", body, None);
    let res = pair
        .b
        .clone()
        .oneshot(preview(
            serde_json::json!({ "data": { "name": "Ada" }, "width": 200 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(res.headers()["x-page-count"], "1");
    let (w, h) = png_size(&bytes(res).await);
    assert_eq!(w, 200);
    assert!(h > 200, "a portrait page is taller than wide: {h}");

    let res = pair
        .b
        .clone()
        .oneshot(preview(
            serde_json::json!({ "data": { "name": "Ada" }, "dpi": 36 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let (w, _) = png_size(&bytes(res).await);
    assert!((290..=310).contains(&w), "A4 or Letter at 36 DPI: {w}");

    for (body, says) in [
        (
            serde_json::json!({ "data": { "name": "Ada" }, "page": 2 }),
            "the document has 1",
        ),
        (
            serde_json::json!({ "data": { "name": "Ada" }, "width": 5000 }),
            "'width'",
        ),
        (
            serde_json::json!({ "data": { "name": "Ada" }, "width": 300, "dpi": 72 }),
            "not both",
        ),
        (
            serde_json::json!({ "data": { "name": "Ada" }, "page": 0 }),
            "'page'",
        ),
    ] {
        let res = pair.b.clone().oneshot(preview(body.clone())).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body}");
        let text = String::from_utf8(bytes(res).await).unwrap();
        assert!(text.contains(says), "{body}: {text}");
    }
    // Missing data is the template's error, as on a render.
    let res = pair
        .b
        .clone()
        .oneshot(preview(serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A PDF template is drawn as it is, or filled first.
    let pdf = rusticdoc_core::pdfops::fixtures::form();
    let pdf_id = store_text(&pair.a, "form.pdf", &pdf, None).await;
    let res = pair
        .a
        .clone()
        .oneshot(post_json(
            &format!("/v1/templates/{pdf_id}/preview"),
            serde_json::json!({ "width": 100 }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(png_size(&bytes(res).await).0, 100);
}

fn async_render(body: serde_json::Value, key: Option<&str>) -> Request<Body> {
    let mut req = post_json("/v1/render", body, key);
    req.headers_mut()
        .insert("prefer", "respond-async".parse().unwrap());
    req
}

/// Asks for a job until it ends.
async fn job_until_ended(router: &Router, location: &str, key: Option<&str>) -> serde_json::Value {
    for _ in 0..200 {
        let res = router
            .clone()
            .oneshot(call("GET", location, key))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let job = json(res).await;
        if matches!(job["status"].as_str(), Some("done" | "failed")) {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the job never ended");
}

#[tokio::test]
async fn a_background_job_answers_202_and_any_replica_serves_its_result() {
    let pair = two_replicas(|c| c.api_keys = vec!["alice-key".into(), "bob-key".into()]);
    let alice = Some("alice-key");
    store_text(&pair.a, "hello.txt", b"Hello {{ name }}", alice).await;
    let id = store_text(&pair.a, "hello.txt", b"Hello {{ name }}", alice).await;
    assert_eq!(
        set_alias(&pair.a, "hello", &id, alice).await.status(),
        StatusCode::CREATED
    );

    let res = pair
        .a
        .clone()
        .oneshot(async_render(
            serde_json::json!({ "templateId": "hello", "data": { "name": "Ada" } }),
            alice,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    assert_eq!(res.headers()["preference-applied"], "respond-async");
    let location = res.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string();
    let job = json(res).await;
    let job_id = job["id"].as_str().unwrap().to_string();
    assert_eq!(location, format!("/v1/jobs/{job_id}"));
    assert_eq!(job["status"], "queued");

    let ended = job_until_ended(&pair.b, &location, alice).await;
    assert_eq!(ended["status"], "done", "{ended}");
    assert_eq!(ended["result"]["name"], "hello.txt");
    assert_eq!(ended["result"]["size"], 9);
    assert!(ended["expiresAt"].as_str().is_some());
    let res = pair
        .b
        .clone()
        .oneshot(call("GET", &format!("{location}/result"), alice))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("hello.txt")
    );
    assert_eq!(bytes(res).await, b"Hello Ada");

    // Another caller cannot see it, whatever it knows.
    let bob = Some("bob-key");
    assert_eq!(
        status(&pair.b, call("GET", &location, bob)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&pair.b, call("GET", &format!("{location}/result"), bob)).await,
        StatusCode::NOT_FOUND
    );
    for bad in ["not-an-id", "..", "01966f3e-7c1a-7d3c-9a44-1d2f5a6b7c8d"] {
        assert_eq!(
            status(&pair.b, call("GET", &format!("/v1/jobs/{bad}"), alice)).await,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn a_failed_job_keeps_the_error_its_request_would_have_had() {
    let pair = two_replicas(|_| {});
    let id = store_template(&pair.a, None).await;
    // Accepted, since nothing is wrong with the request; the template then
    // fails without its data.
    let res = pair
        .a
        .clone()
        .oneshot(async_render(serde_json::json!({ "templateId": id }), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let location = res.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string();
    let ended = job_until_ended(&pair.a, &location, None).await;
    assert_eq!(ended["status"], "failed");
    assert_eq!(ended["error"]["error"], "template_error");
    assert_eq!(ended["error"]["status"], 422);
    let res = pair
        .a
        .clone()
        .oneshot(call("GET", &format!("{location}/result"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(json(res).await["error"], "job_failed");

    // A request that is wrong is refused at once, not queued.
    let res = pair
        .a
        .clone()
        .oneshot(async_render(
            serde_json::json!({ "templateId": id, "steps": "teleport" }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_job_that_has_not_ended_says_so_and_a_lost_one_fails() {
    let pair = two_replicas(|_| {});
    let store = pair.state_a.store.clone().unwrap();
    let scope = rusticdoc_core::store::scope_key("public");
    let waiting = rusticdoc_core::store::new_job_id();
    let record = rusticdoc_core::store::JobRecord::queued(
        waiting.clone(),
        "-".into(),
        Duration::from_secs(60),
    );
    store.put_job(&scope, &record).await.unwrap();
    let res = pair
        .b
        .clone()
        .oneshot(call("GET", &format!("/v1/jobs/{waiting}"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().contains_key(header::RETRY_AFTER));
    assert_eq!(json(res).await["status"], "queued");
    let res = pair
        .b
        .clone()
        .oneshot(call("GET", &format!("/v1/jobs/{waiting}/result"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(json(res).await["error"], "job_not_done");

    // A job past its time never ended: the replica running it went away.
    let lost = rusticdoc_core::store::new_job_id();
    let record = rusticdoc_core::store::JobRecord::queued(lost.clone(), "-".into(), Duration::ZERO);
    store.put_job(&scope, &record).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let job = json(
        pair.b
            .clone()
            .oneshot(call("GET", &format!("/v1/jobs/{lost}"), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(job["status"], "failed");
    assert_eq!(job["error"]["error"], "job_lost");
}

#[tokio::test]
async fn jobs_expire_and_are_removed_from_the_store() {
    let pair = two_replicas(|c| c.job_ttl = Duration::from_millis(600));
    let id = store_template(&pair.a, None).await;
    let res = pair
        .a
        .clone()
        .oneshot(async_render(
            serde_json::json!({ "templateId": id, "data": { "name": "Ada" } }),
            None,
        ))
        .await
        .unwrap();
    let location = res.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        job_until_ended(&pair.a, &location, None).await["status"],
        "done"
    );
    let jobs_dir = pair.store.path().join("jobs");
    assert!(std::fs::read_dir(&jobs_dir).unwrap().next().is_some());

    let store = pair.state_b.store.clone().unwrap();
    assert_eq!(
        store
            .expire_jobs(pair.state_b.config.job_ttl)
            .await
            .unwrap(),
        0,
        "not yet"
    );
    tokio::time::sleep(Duration::from_millis(700)).await;
    // Gone for the caller as soon as it expires, before anything is removed.
    assert_eq!(
        status(&pair.b, call("GET", &location, None)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&pair.b, call("GET", &format!("{location}/result"), None)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .expire_jobs(pair.state_b.config.job_ttl)
            .await
            .unwrap(),
        1
    );
    let left: Vec<_> = walk(&jobs_dir);
    assert!(left.is_empty(), "{left:?}");
}

/// Every file under `dir`.
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|e| {
            let path = e.path();
            if path.is_dir() {
                walk(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

#[tokio::test]
async fn background_jobs_share_the_webhook_limit_and_exclude_webhooks() {
    let pair = two_replicas(|c| c.max_async_jobs_per_caller = 0);
    let id = store_template(&pair.a, None).await;
    let body = serde_json::json!({ "templateId": id, "data": { "name": "Ada" } });
    assert_eq!(
        status(&pair.a, async_render(body.clone(), None)).await,
        StatusCode::TOO_MANY_REQUESTS
    );

    let pair = two_replicas(|_| {});
    let mut both = async_render(body, None);
    both.headers_mut().insert(
        "x-rusticdoc-webhook-url",
        "https://hooks.invalid/x".parse().unwrap(),
    );
    let res = pair.a.clone().oneshot(both).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(
        String::from_utf8(bytes(res).await)
            .unwrap()
            .contains("not both")
    );
}

#[tokio::test]
async fn a_tenant_keeps_its_templates_aliases_and_jobs_when_its_key_is_replaced() {
    let pair = two_replicas(|c| {
        c.api_keys = vec!["solo".into()];
        c.tenant_keys = vec![
            ("acme".into(), "old-secret".into()),
            ("acme".into(), "new-secret".into()),
        ];
    });
    let (old, new) = (Some("old-secret"), Some("new-secret"));
    let id = store_template(&pair.a, old).await;
    assert_eq!(
        set_alias(&pair.a, "hello", &id, old).await.status(),
        StatusCode::CREATED
    );
    let res = pair
        .a
        .clone()
        .oneshot(async_render(
            serde_json::json!({ "templateId": "hello", "data": { "name": "Ada" } }),
            old,
        ))
        .await
        .unwrap();
    let location = res.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string();
    job_until_ended(&pair.a, &location, old).await;

    // The new key is the same caller.
    assert_eq!(render_text(&pair.b, "hello", new).await.1, "Hello Ada");
    assert_eq!(render_text(&pair.b, &id, new).await.1, "Hello Ada");
    assert_eq!(
        status(&pair.b, call("GET", &format!("{location}/result"), new)).await,
        StatusCode::OK
    );
    let listed = json(
        pair.b
            .clone()
            .oneshot(call("GET", "/v1/templates", new))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["templates"].as_array().unwrap().len(), 1);

    // An unnamed key is still a caller of its own.
    assert_eq!(
        render_text(&pair.b, "hello", Some("solo")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&pair.b, call("GET", &location, Some("solo"))).await,
        StatusCode::NOT_FOUND
    );
    // A key that was removed is no credential at all.
    assert_eq!(
        status(&pair.b, call("GET", "/v1/templates", Some("older-secret"))).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn json_bodies_carry_the_language_fields() {
    let pair = two_replicas(|_| {});
    let id = store_text(
        &pair.a,
        "label.txt",
        br#"{{ "total" | t }}: {{ 1500 | currency }} {{ when | date("%H:%M %B") }}"#,
        None,
    )
    .await;
    let body = serde_json::json!({
        "templateId": id,
        "data": {"when": "2026-01-15T12:00:00Z"},
        "lang": "pt-BR",
        "currency": "BRL",
        "timezone": "America/Sao_Paulo",
        "translations": {"pt": {"total": "Total geral"}, "pt-br": {"total": "Valor"}},
    });
    let res = pair
        .a
        .clone()
        .oneshot(post_json("/v1/render", body, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        String::from_utf8(bytes(res).await).unwrap(),
        "Valor: R$\u{a0}1.500,00 09:00 janeiro"
    );
    let bad = serde_json::json!({"templateId": id, "translations": {"pt": "Valor"}});
    let res = pair
        .a
        .clone()
        .oneshot(post_json("/v1/render", bad, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let message = json(res).await["message"].as_str().unwrap().to_string();
    assert!(
        message.contains("'translations.pt' must be an object"),
        "{message}"
    );
}
