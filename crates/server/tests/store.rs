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
