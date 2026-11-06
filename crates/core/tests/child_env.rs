//! What child engines see of the service's environment. In a file of its own, so
//! setting a variable races with nothing.

use std::path::Path;
use std::time::Duration;

use rusticdoc_core::convert::LibreOffice;
use rusticdoc_core::{Document, Format, process};

#[tokio::test]
async fn secrets_do_not_reach_child_processes() {
    // SAFETY: this is the only test in this binary, and it has not started threads
    // that read the environment yet.
    unsafe {
        std::env::set_var("RUSTICDOC_API_KEYS", "the-api-key");
        std::env::set_var("RUSTICDOC_WEBHOOK_SECRET", "the-secret");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "another");
    }
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("engine-env");
    let script = format!("env > {}", dump.display());
    process::run(
        "test",
        Path::new("/bin/sh"),
        &["-c".to_string(), script],
        dir.path(),
        &[("HOME", "/somewhere".to_string())],
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    let seen = std::fs::read_to_string(&dump).unwrap();
    assert!(!seen.contains("the-api-key"), "{seen}");
    assert!(!seen.contains("the-secret"), "{seen}");
    assert!(!seen.contains("another"), "{seen}");
    assert!(seen.contains("HOME=/somewhere"), "{seen}");

    // A LibreOffice worker's bridge sees no more. This one writes what it sees as
    // the converted file.
    let bridge = dir.path().join("bridge.py");
    std::fs::write(
        &bridge,
        r#"
import json, os, sys
print(json.dumps({"ready": True, "soffice_pid": None}), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    with open(req["output"], "w") as f:
        f.write("\n".join(sorted(os.environ)))
    print(json.dumps({"id": req["id"], "ok": True, "pages": 1, "ms": 1}), flush=True)
"#,
    )
    .unwrap();
    let office = LibreOffice::new("/nonexistent/soffice".into(), 1).with_bridge(bridge);
    let doc = Document::new("a.txt", b"x".to_vec());
    let out = office
        .convert(&doc, Format::Pdf, None, Duration::from_secs(20))
        .await
        .unwrap();
    let names = String::from_utf8_lossy(&out.bytes);
    assert!(!names.contains("RUSTICDOC_"), "{names}");
    assert!(!names.contains("AWS_"), "{names}");
    assert!(names.contains("TMPDIR"), "{names}");
}
