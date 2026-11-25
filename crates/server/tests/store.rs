//! Stored templates, fonts and assets over the router, with a store in a temporary
//! directory. Two `AppState`s on the same directory stand in for two replicas: they
//! share nothing else, as containers would not.

use std::io::Write as _;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use rusticdoc_core::Config;
use rusticdoc_server::{AppState, app, assetsync, fontsync};
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
        assets_dir: cache.path().join("mounted-assets"),
        asset_cache_dir: cache.path().join("asset-copy"),
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

/// Another service on the pair's store with a key of its own, so another
/// caller.
struct Other {
    router: Router,
    _cache: tempfile::TempDir,
}

fn other_caller(pair: &Pair, key: &str, configure: impl Fn(&mut Config)) -> Other {
    let cache = tempfile::tempdir().unwrap();
    let mut config = replica(&pair.store, &cache);
    configure(&mut config);
    config.api_key = Some(key.into());
    Other {
        router: app(AppState::new(config)),
        _cache: cache,
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
async fn templates_and_aliases_are_one_set_for_the_store() {
    // Two services on one store, as before and after the key is replaced.
    let pair = two_replicas(|c| c.api_key = Some("old-key".into()));
    let other = other_caller(&pair, "new-key", |_| {});
    let (old, new) = (Some("old-key"), Some("new-key"));
    let id = store_template(&pair.a, old).await;
    assert_eq!(
        set_alias(&pair.a, "invoice", &id, old).await.status(),
        StatusCode::CREATED
    );

    for path in [
        format!("/v1/templates/{id}"),
        format!("/v1/templates/{id}/content"),
        "/v1/templates/invoice".to_string(),
        "/v1/aliases/invoice".to_string(),
    ] {
        assert_eq!(
            status(&other.router, call("GET", &path, new)).await,
            StatusCode::OK,
            "{path}"
        );
    }
    assert_eq!(
        render_text(&other.router, "invoice", new).await.0,
        StatusCode::OK
    );
    let listed = json(
        other
            .router
            .clone()
            .oneshot(call("GET", "/v1/templates", new))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["templates"].as_array().unwrap().len(), 1);

    // Nothing at all without the key, or with the other service's.
    for key in [None, old] {
        assert_eq!(
            status(&other.router, call("GET", "/v1/templates", key)).await,
            StatusCode::UNAUTHORIZED,
            "{key:?}"
        );
    }
    assert_eq!(
        status(
            &other.router,
            call("DELETE", &format!("/v1/templates/{id}"), new)
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(&pair.b, call("GET", &format!("/v1/templates/{id}"), old)).await,
        StatusCode::NOT_FOUND
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
    let scope = rusticdoc_core::store::SHARED_SCOPE;
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
async fn quotas_hold_for_the_store() {
    let pair = two_replicas(|c| {
        c.api_key = Some("alice".into());
        c.max_templates = 1;
    });
    let bobs = other_caller(&pair, "bob", |c| c.max_templates = 1);
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
    // Whichever service asks: the limit is the store's.
    assert_eq!(
        status(&bobs.router, post_as("/v1/templates", parts, Some("bob"))).await,
        StatusCode::FORBIDDEN
    );

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
async fn fonts_change_with_any_api_key() {
    let pair = two_replicas(|c| c.api_key = Some("caller".into()));
    let other = other_caller(&pair, "other", |_| {});
    let font = minimal_font("Guarded");
    let parts: &[Part<'_>] = &[("file", Some("g.ttf"), &font)];
    assert_eq!(
        status(&pair.a, post_as("/v1/fonts", parts, None)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&pair.a, post_as("/v1/fonts", parts, Some("not-a-key"))).await,
        StatusCode::UNAUTHORIZED
    );
    let res = pair
        .a
        .clone()
        .oneshot(post_as("/v1/fonts", parts, Some("caller")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let id = json(res).await["fonts"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/v1/fonts/{id}");
    assert_eq!(
        status(&pair.a, call("DELETE", &path, None)).await,
        StatusCode::UNAUTHORIZED
    );
    // Fonts are the store's, so a service with another key deletes them too.
    assert_eq!(
        status(&other.router, call("DELETE", &path, Some("other"))).await,
        StatusCode::NO_CONTENT
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
async fn a_replica_is_not_ready_until_its_fonts_and_assets_are_in_hand() {
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
        StatusCode::SERVICE_UNAVAILABLE
    );
    assetsync::run_once(&pair.state_a).await.unwrap();
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

    assetsync::run_once(&pair.state_b).await.unwrap();
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
    let pair = two_replicas(|c| c.api_key = Some("alice-key".into()));
    let bobs = other_caller(&pair, "bob-key", |_| {});
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

    // Jobs are one set for the store: a service on it with another key reads
    // the same job, and a request with no key reads nothing.
    let bob = Some("bob-key");
    assert_eq!(
        status(&bobs.router, call("GET", &location, bob)).await,
        StatusCode::OK
    );
    let res = bobs
        .router
        .clone()
        .oneshot(call("GET", &format!("{location}/result"), bob))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(bytes(res).await, b"Hello Ada");
    assert_eq!(
        status(&bobs.router, call("GET", &location, None)).await,
        StatusCode::UNAUTHORIZED
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
    let scope = rusticdoc_core::store::SHARED_SCOPE;
    let waiting = rusticdoc_core::store::new_job_id();
    let record = rusticdoc_core::store::JobRecord::queued(
        waiting.clone(),
        "-".into(),
        Duration::from_secs(60),
    );
    store.put_job(scope, &record).await.unwrap();
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
    store.put_job(scope, &record).await.unwrap();
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
    let pair = two_replicas(|c| c.max_async_jobs = 0);
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

#[tokio::test]
async fn job_results_stop_at_the_limit_for_the_store() {
    let pair = two_replicas(|c| {
        c.api_key = Some("alice-key".into());
        c.max_job_storage = 6_000;
    });
    let bobs = other_caller(&pair, "bob-key", |c| c.max_job_storage = 6_000);
    let run = |router: &Router, key: &'static str| {
        let router = router.clone();
        async move {
            let id = store_text(&router, "hello.txt", b"Hello {{ name }}", Some(key)).await;
            let res = router
                .clone()
                .oneshot(async_render(
                    serde_json::json!({ "templateId": id, "data": { "name": "a".repeat(4_000) } }),
                    Some(key),
                ))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::ACCEPTED);
            let location = res.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .to_string();
            job_until_ended(&router, &location, Some(key)).await
        }
    };
    let first = run(&pair.a, "alice-key").await;
    assert_eq!(first["status"], "done", "{first}");
    // The next would take the results past what the store may hold.
    let second = run(&pair.a, "alice-key").await;
    assert_eq!(second["status"], "failed", "{second}");
    assert_eq!(second["error"]["error"], "quota_exceeded", "{second}");
    // A service on the store with another key counts the same results.
    let other = run(&bobs.router, "bob-key").await;
    assert_eq!(other["status"], "failed", "{other}");
    assert_eq!(other["error"]["error"], "quota_exceeded", "{other}");
}

#[tokio::test]
async fn health_says_where_the_store_is_to_a_caller_with_a_key() {
    let pair = two_replicas(|c| c.api_key = Some("alice-key".into()));
    let store_of = |key: Option<&'static str>| {
        let router = pair.a.clone();
        async move {
            let res = router.oneshot(call("GET", "/health", key)).await.unwrap();
            json(res).await["details"]["store"].clone()
        }
    };
    assert!(store_of(None).await.is_null());
    let seen = store_of(Some("alice-key")).await;
    assert_eq!(seen["fontsSynced"], false);
    assert!(seen["location"].as_str().unwrap().starts_with("file://"));
}

/// A 1x1 PNG.
const DOT: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

async fn asset_names(router: &Router) -> Vec<(String, bool)> {
    let res = router
        .clone()
        .oneshot(call("GET", "/v1/assets", None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    json(res)
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["name"].as_str().unwrap().to_string(),
                a["mounted"].as_bool().unwrap(),
            )
        })
        .collect()
}

/// What a text template that names an asset renders to, or its status.
async fn rendered(router: &Router, name: &str) -> Result<String, StatusCode> {
    let template = format!("{{{{ asset('{name}') }}}}");
    let parts: &[Part<'_>] = &[("template", Some("t.html"), template.as_bytes())];
    let res = router
        .clone()
        .oneshot(post("/v1/render", parts))
        .await
        .unwrap();
    match res.status() {
        StatusCode::OK => Ok(String::from_utf8(bytes(res).await).unwrap()),
        status => Err(status),
    }
}

fn pdf_has_image(pdf: &[u8]) -> bool {
    let text = String::from_utf8_lossy(pdf);
    text.contains("/Subtype /Image") || text.contains("/Subtype/Image")
}

async fn page_has_image(router: &Router, src: &str) -> bool {
    let html = format!("<img src=\"{src}\" width=\"40\" height=\"40\">");
    let parts: &[Part<'_>] = &[("files", Some("p.html"), html.as_bytes())];
    let res = router
        .clone()
        .oneshot(post("/v1/convert", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    pdf_has_image(&bytes(res).await)
}

#[tokio::test]
async fn a_zip_of_assets_reaches_the_other_replica_and_its_documents_and_leaves_with_a_delete() {
    let pair = two_replicas(|_| {});
    let zip = zip_of(&[
        ("brand/logo.png", DOT),
        ("page.css", b"p { color: red }"),
        ("LICENSE.txt", b"terms"),
        ("__MACOSX/._page.css", b"junk"),
    ]);
    let parts: &[Part<'_>] = &[("file", Some("pack.zip"), &zip)];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/assets", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let stored = json(res).await;
    assert_eq!(stored["assets"][0]["name"], "brand/logo.png");
    assert_eq!(stored["assets"][0]["type"], "image");
    assert_eq!(stored["assets"][1]["type"], "stylesheet");
    assert_eq!(stored["skipped"][0]["file"], "LICENSE.txt");
    assert_eq!(stored["syncIntervalSeconds"], 3600);

    // In use where it was uploaded, and not yet where it has not synced, though
    // every replica lists and serves it.
    assert!(page_has_image(&pair.a, "brand/logo.png").await);
    assert!(!page_has_image(&pair.b, "brand/logo.png").await);
    assert_eq!(
        rendered(&pair.b, "brand/logo.png").await,
        Err(StatusCode::UNPROCESSABLE_ENTITY)
    );
    assert_eq!(asset_names(&pair.b).await.len(), 2);
    assetsync::run_once(&pair.state_b).await.unwrap();
    assert!(page_has_image(&pair.b, "brand/logo.png").await);
    assert_eq!(
        rendered(&pair.b, "brand/logo.png").await.as_deref(),
        Ok("brand/logo.png")
    );
    // Filled and laid out in one request, with a name from the data.
    let parts: &[Part<'_>] = &[
        (
            "template",
            Some("t.html"),
            b"<img src=\"{{ logo }}\" width=\"40\" height=\"40\">",
        ),
        ("data", None, br#"{"logo":"brand/logo.png"}"#),
        ("steps", None, b"convert"),
    ];
    let res = pair
        .b
        .clone()
        .oneshot(post("/v1/render", parts))
        .await
        .unwrap();
    assert!(pdf_has_image(&bytes(res).await));

    // Its folder may be written as it is or percent-encoded.
    for path in ["/v1/assets/brand/logo.png", "/v1/assets/brand%2Flogo.png"] {
        let res = pair
            .b
            .clone()
            .oneshot(call("GET", path, None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()[header::CONTENT_DISPOSITION],
            "attachment; filename=\"logo.png\""
        );
        assert_eq!(
            res.headers()[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert_eq!(bytes(res).await, DOT);
    }

    let delete = || call("DELETE", "/v1/assets/brand/logo.png", None);
    assert_eq!(status(&pair.b, delete()).await, StatusCode::NO_CONTENT);
    assert!(!page_has_image(&pair.b, "brand/logo.png").await);
    assert!(page_has_image(&pair.a, "brand/logo.png").await);
    assetsync::run_once(&pair.state_a).await.unwrap();
    assert!(!page_has_image(&pair.a, "brand/logo.png").await);
    assert_eq!(
        asset_names(&pair.a).await,
        [("page.css".to_string(), false)]
    );
    assert_eq!(status(&pair.b, delete()).await, StatusCode::NOT_FOUND);
    assert_eq!(
        status(&pair.b, call("GET", "/v1/assets/brand/logo.png", None)).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_asset_stored_again_replaces_the_old_one_on_every_replica() {
    let pair = two_replicas(|_| {});
    let store = |css: &'static [u8]| {
        let parts: &[Part<'_>] = &[
            ("file", Some("upload.css"), css),
            ("name", None, b"brand/page.css"),
        ];
        status(&pair.a, post("/v1/assets", parts))
    };
    let width = |router: Router| async move {
        let html = b"<link rel=\"stylesheet\" href=\"brand/page.css\"><p>x</p>";
        let parts: &[Part<'_>] = &[("files", Some("p.html"), html)];
        let res = router.oneshot(post("/v1/convert", parts)).await.unwrap();
        let pdf = String::from_utf8_lossy(&bytes(res).await).into_owned();
        let at = pdf.find("/MediaBox").unwrap();
        pdf[at..at + 40].contains("300")
    };
    assert_eq!(
        store(b"@page { size: 200pt 400pt }").await,
        StatusCode::CREATED
    );
    assetsync::run_once(&pair.state_b).await.unwrap();
    assert!(!width(pair.b.clone()).await);
    assert_eq!(
        store(b"@page { size: 300pt 400pt }").await,
        StatusCode::CREATED
    );
    assert!(width(pair.a.clone()).await);
    assert!(!width(pair.b.clone()).await);
    assetsync::run_once(&pair.state_b).await.unwrap();
    assert!(width(pair.b.clone()).await);
    assert_eq!(asset_names(&pair.a).await.len(), 1);
}

#[tokio::test]
async fn mounted_assets_need_no_store_and_no_reload_and_win_over_stored_ones() {
    let pair = two_replicas(|_| {});
    let mount = pair.state_a.config.assets_dir.clone();
    std::fs::create_dir_all(mount.join("brand")).unwrap();
    std::fs::write(mount.join("brand/logo.png"), DOT).unwrap();
    std::fs::write(mount.join("notes.txt"), "not an asset").unwrap();
    assert_eq!(
        asset_names(&pair.a).await,
        [("brand/logo.png".to_string(), true)]
    );
    assert!(page_has_image(&pair.a, "brand/logo.png").await);

    // The name is taken here, so an upload that could never be used is refused.
    let parts: &[Part<'_>] = &[
        ("file", Some("logo.png"), DOT),
        ("name", None, b"brand/logo.png"),
    ];
    let res = pair
        .a
        .clone()
        .oneshot(post("/v1/assets", parts))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(json(res).await["error"], "asset_mounted");
    assert_eq!(
        status(&pair.a, call("DELETE", "/v1/assets/brand/logo.png", None)).await,
        StatusCode::NOT_FOUND
    );
    // A replica without the mount stores it, and then both are listed.
    assert_eq!(
        status(&pair.b, post("/v1/assets", parts)).await,
        StatusCode::CREATED
    );
    assert_eq!(
        asset_names(&pair.a).await,
        [
            ("brand/logo.png".to_string(), true),
            ("brand/logo.png".to_string(), false)
        ]
    );
    std::fs::remove_file(mount.join("brand/logo.png")).unwrap();
    assert!(!page_has_image(&pair.a, "brand/logo.png").await);
}

#[tokio::test]
async fn without_a_store_mounted_assets_work_and_uploads_are_501() {
    let mount = tempfile::tempdir().unwrap();
    std::fs::write(mount.path().join("logo.png"), DOT).unwrap();
    let router = app(AppState::new(Config {
        libreoffice_bin: None,
        assets_dir: mount.path().to_path_buf(),
        ..Config::default()
    }));
    assert!(page_has_image(&router, "logo.png").await);
    assert_eq!(
        rendered(&router, "logo.png").await.as_deref(),
        Ok("logo.png")
    );
    assert_eq!(
        status(&router, call("GET", "/v1/assets/logo.png", None)).await,
        StatusCode::OK
    );
    for name in ["gone.png", "..%2Fsecret.png", "%2Fetc%2Fpasswd"] {
        assert_eq!(
            status(&router, call("GET", &format!("/v1/assets/{name}"), None)).await,
            StatusCode::NOT_FOUND,
            "{name}"
        );
    }
    let parts: &[Part<'_>] = &[("file", Some("a.png"), DOT)];
    assert_eq!(
        status(&router, post("/v1/assets", parts)).await,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        status(&router, call("DELETE", "/v1/assets/logo.png", None)).await,
        StatusCode::NOT_IMPLEMENTED
    );
    // No such directory is no assets.
    let router = app(AppState::new(Config {
        libreoffice_bin: None,
        assets_dir: mount.path().join("missing"),
        ..Config::default()
    }));
    assert!(asset_names(&router).await.is_empty());
}

#[tokio::test]
async fn bad_asset_uploads_are_refused_and_store_nothing() {
    let pair = two_replicas(|c| {
        c.max_stored_assets = 2;
        c.max_asset_storage = 200;
    });
    let zip = zip_of(&[("a.css", b"p {}"), ("bad.png", b"not a png")]);
    let cases: &[&[Part<'_>]] = &[
        &[],
        &[("file", Some("script.js"), b"alert(1)")],
        &[("file", Some("logo.png"), b"<svg/>")],
        &[("file", Some("pack.zip"), &zip)],
        &[("file", Some("a.css"), b""), ("name", None, b"../a.css")],
        &[("file", Some("a.css"), b""), ("file", Some("b.css"), b"")],
    ];
    for parts in cases {
        assert_eq!(
            status(&pair.a, post("/v1/assets", parts)).await,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(asset_names(&pair.a).await.is_empty());

    let upload = |name: &'static str, css: Vec<u8>| {
        let router = pair.a.clone();
        async move {
            let parts: &[Part<'_>] = &[("file", Some(name), &css)];
            status(&router, post("/v1/assets", parts)).await
        }
    };
    assert_eq!(upload("a.css", vec![b' '; 150]).await, StatusCode::CREATED);
    assert_eq!(upload("b.css", vec![b' '; 51]).await, StatusCode::FORBIDDEN);
    // A replacement counts once.
    assert_eq!(upload("a.css", vec![b' '; 190]).await, StatusCode::CREATED);
    assert_eq!(upload("b.css", vec![b' '; 5]).await, StatusCode::CREATED);
    assert_eq!(upload("c.css", vec![]).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn assets_need_the_key() {
    let pair = two_replicas(|c| c.api_key = Some("key".into()));
    let parts: &[Part<'_>] = &[("file", Some("a.css"), b"")];
    for req in [
        post("/v1/assets", parts),
        call("GET", "/v1/assets", None),
        call("GET", "/v1/assets/a.css", None),
        call("DELETE", "/v1/assets/a.css", None),
    ] {
        assert_eq!(status(&pair.a, req).await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        status(&pair.a, post_as("/v1/assets", parts, Some("key"))).await,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn a_corrupt_asset_is_left_out_and_an_asset_sync_touches_only_what_it_wrote() {
    let pair = two_replicas(|_| {});
    for name in ["good.png", "bad.png"] {
        let parts: &[Part<'_>] = &[("file", Some(name), DOT)];
        assert_eq!(
            status(&pair.a, post("/v1/assets", parts)).await,
            StatusCode::CREATED
        );
    }
    std::fs::write(pair.store.path().join("assets/bad.png"), b"forged").unwrap();
    std::fs::write(pair.store.path().join("assets/notes.txt"), b"stray").unwrap();
    let copy = pair.state_b.config.asset_cache_dir.clone();
    let theirs = copy.join("someone-elses.png");
    std::fs::write(&theirs, b"keep me").unwrap();
    assetsync::run_once(&pair.state_b).await.unwrap();
    assert!(theirs.exists());
    assert!(page_has_image(&pair.b, "good.png").await);
    assert!(rendered(&pair.b, "bad.png").await.is_err());
    // Nothing changed, so nothing is fetched again but the one left out.
    let written = |dir: &std::path::Path| -> Vec<_> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().modified().unwrap())
            .collect()
    };
    let before = written(&copy);
    assetsync::run_once(&pair.state_b).await.unwrap();
    assert_eq!(written(&copy), before);
}

fn no_store(configure: impl Fn(&mut Config)) -> (Router, tempfile::TempDir) {
    let mount = tempfile::tempdir().unwrap();
    let mut config = Config {
        libreoffice_bin: None,
        assets_dir: mount.path().to_path_buf(),
        ..Config::default()
    };
    configure(&mut config);
    (app(AppState::new(config)), mount)
}

fn sha256(bytes: &[u8]) -> String {
    rusticdoc_core::template::template_hash(bytes)
}

/// The width of a page's linked `page.css`, so a test can tell libraries apart.
async fn page_width(router: &Router, extra: &[Part<'_>], key: Option<&str>) -> Option<u32> {
    let html = b"<link rel=\"stylesheet\" href=\"brand/page.css\"><p>x</p>";
    let mut parts: Vec<Part<'_>> = vec![("files", Some("p.html"), html)];
    parts.extend_from_slice(extra);
    let res = router
        .clone()
        .oneshot(post_as("/v1/convert", &parts, key))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pdf = String::from_utf8_lossy(&bytes(res).await).into_owned();
    let at = pdf.find("/MediaBox").unwrap();
    [200, 300, 400]
        .into_iter()
        .find(|w| pdf[at..at + 24].contains(&format!(" {w}")))
}

fn css(width: u32) -> Vec<u8> {
    zip_of(&[(
        "brand/page.css",
        format!("@page {{ size: {width}pt 500pt }}").as_bytes(),
    )])
}

#[tokio::test]
async fn a_request_brings_its_own_asset_library_and_no_other_request_sees_it() {
    let (router, mount) = no_store(|_| {});
    std::fs::create_dir_all(mount.path().join("brand")).unwrap();
    std::fs::write(
        mount.path().join("brand/page.css"),
        "@page { size: 200pt 500pt }",
    )
    .unwrap();
    let zips = [css(300), css(400)];
    fn with(zip: &[u8]) -> [Part<'_>; 1] {
        [("assetLibrary", Some("library.zip"), zip)]
    }
    assert_eq!(page_width(&router, &[], None).await, Some(200));
    // At the same moment, each gets its own, over the mounted one.
    let (a, b) = (with(&zips[0]), with(&zips[1]));
    let (first, second) =
        tokio::join!(page_width(&router, &a, None), page_width(&router, &b, None));
    assert_eq!((first, second), (Some(300), Some(400)));
    assert_eq!(page_width(&router, &[], None).await, Some(200));
    // A stylesheet sent as `assets` still comes last, so it wins.
    let sent = [
        with(&zips[0])[0],
        ("assets", Some("own.css"), b"@page { size: 400pt 500pt }"),
    ];
    assert_eq!(page_width(&router, &sent, None).await, Some(400));
    // It belongs to the request, not to the service.
    assert_eq!(
        asset_names(&router).await,
        [("brand/page.css".to_string(), true)]
    );
}

#[tokio::test]
async fn an_asset_library_reaches_templates_batches_validation_and_stored_templates() {
    let (router, _mount) = no_store(|_| {});
    let zip = zip_of(&[("brand/logo.png", DOT)]);
    let library: Part<'_> = ("assetLibrary", Some("library.zip"), &zip);
    let template: Part<'_> = (
        "template",
        Some("t.html"),
        b"<img src=\"{{ asset('brand/logo.png') }}\" width=\"40\" height=\"40\">{{ n }}",
    );
    let data: Part<'_> = ("data", None, br#"{"n":1,"rows":[{"a":1},{"a":2}]}"#);

    let without: &[Part<'_>] = &[template, data];
    assert_eq!(
        status(&router, post("/v1/render", without)).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Wherever the part comes in the form, it is not the template.
    let steps: Part<'_> = ("steps", None, b"convert");
    for parts in [
        &[library, template, data, steps],
        &[template, data, steps, library],
    ] {
        let res = router
            .clone()
            .oneshot(post("/v1/render", parts))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(pdf_has_image(&bytes(res).await));
    }
    let res = router
        .clone()
        .oneshot(post("/v1/templates/validate", &[template, data, library]))
        .await
        .unwrap();
    assert_eq!(json(res).await["ok"], true);

    let split: Part<'_> = ("splitBy", None, b"rows");
    let res = router
        .clone()
        .oneshot(post(
            "/v1/render/batch",
            &[template, data, split, steps, library],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes(res).await)).unwrap();
    assert_eq!(zip.len(), 2);
    for i in 0..2 {
        let mut pdf = Vec::new();
        std::io::Read::read_to_end(&mut zip.by_index(i).unwrap(), &mut pdf).unwrap();
        assert!(pdf_has_image(&pdf), "document {i}");
    }

    // With a stored template, from JSON, by the library's hash.
    let pair = two_replicas(|_| {});
    let id = store_text(&pair.a, "t.html", template.2, None).await;
    let by_id: &[Part<'_>] = &[("templateId", None, id.as_bytes()), data, library];
    assert_eq!(
        status(&pair.a, post("/v1/render", by_id)).await,
        StatusCode::OK
    );
    let body = |zip: &[u8]| serde_json::json!({"templateId": id, "data": {"n": 1}, "assetLibraryHash": sha256(zip)});
    let sent = zip_of(&[("brand/logo.png", DOT)]);
    assert_eq!(
        status(&pair.a, post_json("/v1/render", body(&sent), None)).await,
        StatusCode::OK
    );
    // Another replica has not seen it.
    let res = pair
        .b
        .clone()
        .oneshot(post_json("/v1/render", body(&sent), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(json(res).await["error"], "asset_library_not_cached");
    let file = serde_json::json!({"templateId": id, "assetLibrary": "library.zip"});
    assert_eq!(
        status(&pair.a, post_json("/v1/render", file, None)).await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn an_asset_library_outlives_its_request_while_a_background_job_needs_it() {
    let pair = two_replicas(|c| c.max_asset_library_space = 16 * 1024);
    let zip = zip_of(&[("brand/logo.png", DOT)]);
    let parts: &[Part<'_>] = &[
        (
            "template",
            Some("t.html"),
            b"<img src=\"{{ asset('brand/logo.png') }}\" width=\"40\" height=\"40\">",
        ),
        ("data", None, br#"{"n":1}"#),
        ("steps", None, b"convert"),
        ("assetLibrary", Some("library.zip"), &zip),
    ];
    let mut req = post("/v1/render", parts);
    req.headers_mut()
        .insert("prefer", "respond-async".parse().unwrap());
    let res = pair.a.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let location = res.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string();
    // Others may come and push it out of the cache meanwhile.
    for width in [200, 300, 400] {
        let other = css(width);
        let parts: &[Part<'_>] = &[
            ("files", Some("p.html"), b"<p>x</p>"),
            ("assetLibrary", Some("library.zip"), &other),
        ];
        let status = status(&pair.a, post("/v1/convert", parts)).await;
        assert!(
            status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
            "{status}"
        );
    }
    let job = job_until_ended(&pair.a, &location, None).await;
    assert_eq!(job["status"], "done", "{job}");
    let res = pair
        .a
        .clone()
        .oneshot(call("GET", &format!("{location}/result"), None))
        .await
        .unwrap();
    assert!(pdf_has_image(&bytes(res).await));
}

#[tokio::test]
async fn an_asset_library_is_recalled_by_hash_and_sent_once() {
    let (router, _mount) = no_store(|_| {});
    let zip = css(300);
    let hash = sha256(&zip);
    let by_hash = [("assetLibraryHash", None, hash.as_bytes())];
    let refused = |extra: Vec<Part<'_>>| {
        let mut parts: Vec<Part<'_>> = vec![("files", Some("p.html"), b"<p>x</p>")];
        parts.extend(extra);
        post("/v1/convert", &parts)
    };
    assert_eq!(
        status(&router, refused(by_hash.to_vec())).await,
        StatusCode::NOT_FOUND
    );
    let library = [("assetLibrary", Some("library.zip"), &zip[..])];
    assert_eq!(page_width(&router, &library, None).await, Some(300));
    assert_eq!(page_width(&router, &by_hash, None).await, Some(300));
    // One or the other, once, and a file.
    for extra in [
        vec![library[0], by_hash[0]],
        vec![library[0], library[0]],
        vec![("assetLibrary", None, b"library.zip")],
        vec![("assetLibrary", Some("logo.png"), DOT)],
    ] {
        assert_eq!(
            status(&router, refused(extra)).await,
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn asset_libraries_stay_within_their_disk() {
    let (router, _mount) = no_store(|c| c.max_asset_library_space = 16 * 1024);
    let send = |zip: Vec<u8>| {
        let router = router.clone();
        async move {
            let parts: &[Part<'_>] = &[
                ("files", Some("p.html"), b"<p>x</p>"),
                ("assetLibrary", Some("library.zip"), &zip),
            ];
            let res = router.oneshot(post("/v1/convert", parts)).await.unwrap();
            (res.status(), json_or_null(res).await)
        }
    };
    // A small ZIP that expands past it is refused as it expands.
    let (code, body) = send(zip_of(&[("a.css", &vec![b' '; 4 * 1024 * 1024])])).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        body["message"].as_str().unwrap().contains("bytes of disk"),
        "{body}"
    );
    // More than fit, one after another: the older ones make room.
    for width in [200, 300, 400] {
        assert_eq!(send(css(width)).await.0, StatusCode::OK);
    }
    let hash = sha256(&css(200));
    let parts: &[Part<'_>] = &[
        ("files", Some("p.html"), b"<p>x</p>"),
        ("assetLibraryHash", None, hash.as_bytes()),
    ];
    assert_eq!(
        status(&router, post("/v1/convert", parts)).await,
        StatusCode::NOT_FOUND
    );
}

async fn json_or_null(res: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&bytes(res).await).unwrap_or_default()
}
