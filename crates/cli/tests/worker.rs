//! The real binary, serving through its real render worker.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Service {
    child: Child,
    base: String,
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(env: &[(&str, &str)]) -> Service {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rusticdoc"));
    cmd.arg("serve")
        .env_clear()
        .env("RUSTICDOC_BIND", format!("127.0.0.1:{port}"))
        .env("RUSTICDOC_LIBREOFFICE_BIN", "none")
        .env("RUSTICDOC_QPDF_BIN", "none")
        .env("RUSTICDOC_PDFCPU_BIN", "none")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in env {
        cmd.env(key, value);
    }
    Service {
        child: cmd.spawn().unwrap(),
        base: format!("http://127.0.0.1:{port}"),
    }
}

async fn wait_until_up(service: &Service) -> reqwest::Client {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        if let Ok(res) = client.get(format!("{}/health", service.base)).send().await
            && res.status().is_success()
        {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the service did not come up");
}

async fn render(client: &reqwest::Client, base: &str, template: &str) -> (u16, String) {
    let boundary = "b0undary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"template\"; filename=\"t.txt\"\r\n\r\n{template}\r\n--{boundary}--\r\n"
    );
    let res = client
        .post(format!("{base}/v1/render"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    (res.status().as_u16(), res.text().await.unwrap())
}

#[tokio::test]
async fn renders_go_through_the_worker() {
    let service = start(&[]);
    let client = wait_until_up(&service).await;
    let (status, body) = render(&client, &service.base, "{{ 1 + 1 }}").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, "2");
    let (status, body) = render(&client, &service.base, "{{ nobody.home }}").await;
    assert_eq!(status, 422, "{body}");
    assert!(body.contains("template_error"), "{body}");
}

#[test]
fn a_worker_that_cannot_start_stops_the_service() {
    let mut service = start(&[("RUSTICDOC_RENDER_WORKER_BIN", "/nonexistent/rusticdoc")]);
    let status = service.child.wait().unwrap();
    assert!(!status.success());
}

#[test]
fn an_unknown_render_mode_stops_the_service() {
    let mut service = start(&[("RUSTICDOC_RENDER_MODE", "nonsense")]);
    let status = service.child.wait().unwrap();
    assert!(!status.success());
}

/// `RLIMIT_AS` means nothing on macOS, so this one only runs on Linux.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_template_that_doubles_a_string_is_refused_and_the_service_stays_up() {
    let mut template = String::from(r#"{% set x = "a" %}"#);
    template.push_str(&"{% set x = x ~ x %}".repeat(40));
    template.push_str("{{ x | length }}");
    let service = start(&[("RUSTICDOC_RENDER_MEMORY", "256MB")]);
    let client = wait_until_up(&service).await;
    let (status, body) = render(&client, &service.base, &template).await;
    assert_eq!(status, 413, "{body}");
    assert!(body.contains("memory"), "{body}");
    let (status, body) = render(&client, &service.base, "{{ 1 + 1 }}").await;
    assert_eq!((status, body.as_str()), (200, "2"));
    let health = client
        .get(format!("{}/health", service.base))
        .send()
        .await
        .unwrap();
    assert!(health.status().is_success());
}

/// Posts `parts` as a multipart form: (field, file name, bytes).
async fn post_parts(
    client: &reqwest::Client,
    url: &str,
    parts: &[(&str, &str, &[u8])],
) -> (u16, Vec<u8>) {
    let boundary = "b0undary";
    let mut body = Vec::new();
    for (field, name, bytes) in parts {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{name}\"\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let res = client
        .post(url)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    (res.status().as_u16(), res.bytes().await.unwrap().to_vec())
}

/// A 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

#[tokio::test]
async fn html_becomes_a_pdf_in_the_worker_with_no_libreoffice() {
    let service = start(&[]);
    let client = wait_until_up(&service).await;
    let html = br#"<!doctype html><html><body><h1>Invoice</h1><img src="logo.png" width="40" height="40"></body></html>"#;
    let url = format!("{}/v1/convert", service.base);
    let (status, body) = post_parts(
        &client,
        &url,
        &[("files", "invoice.html", html), ("assets", "logo.png", PNG)],
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(body.starts_with(b"%PDF-"));
    assert!(
        body.windows(b"/Image".len()).any(|w| w == b"/Image"),
        "the logo is in the PDF"
    );
    // The HTML parser prints a line to standard output for each malformed
    // tag; none of it may reach the worker's replies.
    let broken = b"<p><b>unclosed <div></span></i> & <table><td>cell</p>";
    let (status, body) = post_parts(&client, &url, &[("files", "broken.html", broken)]).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(body.starts_with(b"%PDF-"));
}

/// `RLIMIT_AS` means nothing on macOS, so this one only runs on Linux.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_hostile_page_is_refused_and_the_service_stays_up() {
    let mut html = String::from("<!doctype html><html><body>");
    html.push_str(&"<div style=\"display:flex\">".repeat(20_000));
    html.push_str("deep");
    let service = start(&[
        ("RUSTICDOC_RENDER_MEMORY", "256MB"),
        ("RUSTICDOC_API_TIMEOUT", "10s"),
    ]);
    let client = wait_until_up(&service).await;
    let url = format!("{}/v1/convert", service.base);
    let (status, _) = post_parts(&client, &url, &[("files", "deep.html", html.as_bytes())]).await;
    assert!(
        status == 200 || (400..600).contains(&status),
        "the request ends, whatever it ends with"
    );
    let (status, body) = post_parts(&client, &url, &[("files", "ok.html", b"<p>fine</p>")]).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
}

#[test]
fn webhook_secret_prints_the_key_a_receiver_verifies_with() {
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_rusticdoc"))
            .arg("webhook-secret")
            .args(args)
            .env_clear()
            .env("RUSTICDOC_WEBHOOK_SECRET", "master")
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let key = rusticdoc_server::webhook::signing_key(
        "master",
        &rusticdoc_server::auth::key_principal("tenant-a").0,
    );
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(run(&["--api-key", "tenant-a"]), hex);
    assert_ne!(run(&["--api-key", "tenant-b"]), hex);
    let key = rusticdoc_server::webhook::signing_key(
        "master",
        &rusticdoc_server::auth::basic_principal("ada").0,
    );
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(run(&["--basic-user", "ada"]), hex);
    let key = rusticdoc_server::webhook::signing_key(
        "master",
        &rusticdoc_server::auth::tenant_principal("acme").0,
    );
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(run(&["--tenant", "acme"]), hex);
    assert_eq!(run(&[]), "6d6173746572", "the master secret, in hex");
}

#[tokio::test]
async fn a_stored_templates_variables_are_read_in_the_worker() {
    let store = std::env::temp_dir().join(format!("rusticdoc-worker-store-{}", std::process::id()));
    let url = format!("file://{}", store.display());
    let service = start(&[
        ("RUSTICDOC_STORE", url.as_str()),
        (
            "RUSTICDOC_FONT_CACHE_DIR",
            store.join("fonts").to_str().unwrap(),
        ),
    ]);
    let client = wait_until_up(&service).await;
    let template = b"{% for item in items %}{{ item.name }}{% endfor %}{{ total }}";
    let (status, body) = post_parts(
        &client,
        &format!("{}/v1/templates", service.base),
        &[("template", "list.txt", template)],
    )
    .await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&body));
    let id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = client
        .get(format!("{}/v1/templates/{id}/variables", service.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let found: serde_json::Value = res.json().await.unwrap();
    assert_eq!(
        found["example"],
        serde_json::json!({"items": [{"name": ""}], "total": ""})
    );
    drop(service);
    let _ = std::fs::remove_dir_all(&store);
}
