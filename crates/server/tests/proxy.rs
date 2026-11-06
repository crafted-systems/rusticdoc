//! Proxy variables in the environment. In a file of its own, so setting them races
//! with nothing.

use std::time::Duration;

use rusticdoc_server::webhook::{Target, client_for};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A listener that counts connections and answers each with 200.
async fn counting() -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
            });
        }
    });
    (port, hits)
}

#[tokio::test]
async fn proxy_variables_do_not_undo_the_pinning() {
    use std::sync::atomic::Ordering;
    let (target_port, target_hits) = counting().await;
    let (proxy_port, proxy_hits) = counting().await;
    let proxy = format!("http://127.0.0.1:{proxy_port}");
    // SAFETY: this is the only test in this binary, and it sets these before any
    // client reads them.
    unsafe {
        std::env::set_var("HTTP_PROXY", &proxy);
        std::env::set_var("http_proxy", &proxy);
        std::env::set_var("NO_PROXY", "");
    }
    let target = Target {
        url: format!("http://receiver.test:{target_port}/x"),
        host: "receiver.test".into(),
        addrs: vec![format!("127.0.0.1:{target_port}").parse().unwrap()],
    };
    let client = client_for(&target, Duration::from_secs(5), None).unwrap();
    assert!(
        client
            .get(&target.url)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        target_hits.load(Ordering::SeqCst),
        1,
        "went straight to the target"
    );
    assert_eq!(proxy_hits.load(Ordering::SeqCst), 0);

    // Named on purpose, the proxy is used.
    let client = client_for(&target, Duration::from_secs(5), Some(&proxy)).unwrap();
    assert!(
        client
            .get(&target.url)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(target_hits.load(Ordering::SeqCst), 1);
    assert_eq!(proxy_hits.load(Ordering::SeqCst), 1);
}
