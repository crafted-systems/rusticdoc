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
    assert_eq!(run(&[]), "6d6173746572", "the master secret, in hex");
}
