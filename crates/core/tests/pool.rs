//! The LibreOffice worker pool, driven against a fake bridge: a Python
//! script that speaks the bridge's JSON-lines protocol and writes the
//! request it received as the "converted" file. Needs `python3` on PATH,
//! not LibreOffice.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rusticdoc_core::convert::{ConvertOptions, LibreOffice};
use rusticdoc_core::{Document, Error, Format};

/// A fake bridge in `mode`: `ok` answers every request; `fail` answers
/// `ok:false`; `hang` never answers and keeps a `sleep` child in its own
/// session, reported as the soffice pid; `slow` answers after 300 ms;
/// `die-after:N` exits after N answers. It appends its own pid and its
/// child's to `pids-<mode>` next to the script, and echoes each request,
/// plus its `TMPDIR` and profile, as the "converted" file.
fn fake_bridge(dir: &tempfile::TempDir, mode: &str) -> PathBuf {
    let script = format!(
        r#"
import json, os, subprocess, sys, time
MODE = {mode:?}
args = sys.argv[1:]
assert "--socket-path" in args and "--profile" in args and "--soffice" in args, args
profile = args[args.index("--profile") + 1]
assert os.path.isdir(profile), "profile directory missing"
child = os.getpid()
if MODE == "hang":
    child = subprocess.Popen(["sleep", "3600"], start_new_session=True).pid
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "pids-" + MODE), "a") as f:
    f.write("%d %d\n" % (os.getpid(), child))
print(json.dumps({{"ready": True, "soffice_pid": child}}), flush=True)
served = 0
for line in sys.stdin:
    req = json.loads(line)
    req["tmpdir"] = os.environ.get("TMPDIR")
    req["profile"] = profile
    if MODE == "hang":
        time.sleep(3600)
    if MODE == "slow":
        time.sleep(0.3)
    if MODE == "fail":
        print(json.dumps({{"id": req["id"], "ok": False, "error": "no filter for that"}}), flush=True)
        continue
    with open(req["output"], "w") as f:
        json.dump(req, f)
    print(json.dumps({{"id": req["id"], "ok": True, "pages": 1, "ms": 1}}), flush=True)
    served += 1
    if MODE.startswith("die-after:") and served >= int(MODE.split(":")[1]):
        sys.exit(3)
"#
    );
    let path = dir.path().join(format!("bridge-{mode}.py"));
    std::fs::write(&path, script).unwrap();
    path
}

fn pool(dir: &tempfile::TempDir, mode: &str, workers: usize) -> LibreOffice {
    LibreOffice::new(PathBuf::from("/nonexistent/soffice"), workers)
        .with_bridge(fake_bridge(dir, mode))
        .with_start_timeout(Duration::from_secs(20))
}

fn docx() -> Document {
    Document::new("report.docx", b"PK\x03\x04 not really".to_vec())
}

/// The request the fake bridge saw, read back from the "converted" file.
fn request(out: &Document) -> serde_json::Value {
    serde_json::from_slice(&out.bytes).unwrap()
}

async fn settle(lo: &LibreOffice, idle: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while lo.health().await["idle"] != idle {
        assert!(
            Instant::now() < deadline,
            "pool never settled: {}",
            lo.health().await
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn converts_through_the_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let lo = pool(&dir, "ok", 2);
    let out = lo
        .convert(&docx(), Format::Pdf, None, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(out.name, "report.pdf");
    let req = request(&out);
    assert_eq!(req["filter"], "pdf");
    assert_eq!(req["landscape"], false);
    assert!(req["password"].is_null());
    assert!(req["input"].as_str().unwrap().ends_with("input.docx"));
    // LibreOffice's temporary files go under the worker's private directory.
    let worker_dir = PathBuf::from(req["profile"].as_str().unwrap());
    let tmpdir = PathBuf::from(req["tmpdir"].as_str().unwrap());
    assert!(tmpdir.starts_with(worker_dir.parent().unwrap()), "{req}");
    assert!(tmpdir.is_dir());
    assert_eq!(req["filter_options"]["ExportFormFields"], true);
    assert!(req["filter_options"].get("SelectPdfVersion").is_none());

    let health = lo.health().await;
    assert_eq!(health["workers"], 2);
    assert_eq!(health["idle"], 1);
    assert_eq!(health["busy"], 0);
    assert_eq!(health["started"], 1);
    assert_eq!(health["restarts"], 0);
}

#[tokio::test]
async fn options_become_filter_properties() {
    let dir = tempfile::tempdir().unwrap();
    let lo = pool(&dir, "ok", 1);
    let options = ConvertOptions {
        pdfa: Some(rusticdoc_core::job::PdfaLevel::A2b),
        pdfua: true,
        landscape: true,
        native_page_ranges: Some("1-2".into()),
        single_page_sheets: true,
        quality: 50,
        max_image_resolution: 150,
        password: Some("secret".into()),
        ..Default::default()
    };
    let out = lo
        .convert_with(&docx(), Format::Pdf, &options, Duration::from_secs(10))
        .await
        .unwrap();
    let req = request(&out);
    let data = &req["filter_options"];
    assert_eq!(data["SelectPdfVersion"], 2);
    assert_eq!(data["PDFUACompliance"], true);
    assert_eq!(data["PageRange"], "1-2");
    assert_eq!(data["SinglePageSheets"], true);
    assert_eq!(data["Quality"], 50);
    assert_eq!(data["MaxImageResolution"], 150);
    assert_eq!(req["landscape"], true);
    assert_eq!(req["password"], "secret");

    // Non-PDF targets carry no PDF filter data.
    let out = lo
        .convert_with(&docx(), Format::Odt, &options, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(out.name, "report.odt");
    let req = request(&out);
    assert_eq!(req["filter"], "odt");
    assert_eq!(req["filter_options"], serde_json::json!({}));

    let bad = ConvertOptions {
        max_image_resolution: 299,
        ..Default::default()
    };
    let err = lo
        .convert_with(&docx(), Format::Pdf, &bad, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
}

#[tokio::test]
async fn rejected_documents_keep_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let lo = pool(&dir, "fail", 1);
    for _ in 0..2 {
        let err = lo
            .convert(&docx(), Format::Pdf, None, Duration::from_secs(10))
            .await
            .unwrap_err();
        match err {
            Error::Engine { engine, message } => {
                assert_eq!(engine, "libreoffice");
                assert_eq!(message, "no filter for that");
            }
            other => panic!("unexpected {other}"),
        }
    }
    let health = lo.health().await;
    assert_eq!(health["started"], 1);
    assert_eq!(health["restarts"], 0);
    assert_eq!(health["idle"], 1);
}

#[tokio::test]
async fn timeout_kills_and_replaces_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let lo = pool(&dir, "hang", 1);
    let started = Instant::now();
    let err = lo
        .convert(&docx(), Format::Pdf, None, Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
    // The replacement is started in the background while the slot stays
    // taken, so the pool never grows past its size.
    settle(&lo, 1).await;
    let health = lo.health().await;
    assert_eq!(health["restarts"], 1);
    assert_eq!(health["started"], 2);
    assert_eq!(health["busy"], 0);
}

/// True while a process with `pid` exists (zombies included).
fn process_exists(pid: i32) -> bool {
    // SAFETY: kill(2) with signal 0 only checks for the process.
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn wait_gone(pid: i32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_exists(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    !process_exists(pid)
}

#[tokio::test]
async fn a_timed_out_worker_leaves_no_process_behind() {
    let dir = tempfile::tempdir().unwrap();
    let lo = pool(&dir, "hang", 1);
    let err = lo
        .convert(&docx(), Format::Pdf, None, Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err}");
    settle(&lo, 1).await;
    let pids = std::fs::read_to_string(dir.path().join("pids-hang")).unwrap();
    let rows: Vec<Vec<i32>> = pids
        .lines()
        .map(|l| l.split(' ').map(|n| n.parse().unwrap()).collect())
        .collect();
    assert_eq!(rows.len(), 2, "the worker and its replacement: {pids}");
    // The timed-out bridge and its "soffice" (a sleep in its own session,
    // which a group kill would miss) are both gone.
    assert!(
        wait_gone(rows[0][0]).await,
        "bridge {} survived",
        rows[0][0]
    );
    assert!(
        wait_gone(rows[0][1]).await,
        "soffice {} survived",
        rows[0][1]
    );
    // The replacement is alive until the pool is dropped, then its
    // "soffice" is killed too.
    assert!(process_exists(rows[1][1]));
    drop(lo);
    assert!(
        wait_gone(rows[1][1]).await,
        "soffice {} outlived the pool",
        rows[1][1]
    );
}

#[tokio::test]
async fn workers_are_recycled_after_max_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let lo = pool(&dir, "ok", 1).with_max_jobs(2);
    for _ in 0..5 {
        lo.convert(&docx(), Format::Pdf, None, Duration::from_secs(10))
            .await
            .unwrap();
    }
    settle(&lo, 1).await;
    let health = lo.health().await;
    assert_eq!(health["max_jobs"], 2);
    assert_eq!(health["restarts"], 2);
    assert_eq!(health["started"], 3);
}

#[tokio::test]
async fn a_crashed_worker_is_replaced_on_the_next_job() {
    let dir = tempfile::tempdir().unwrap();
    let lo = pool(&dir, "die-after:1", 1).with_max_jobs(100);
    lo.convert(&docx(), Format::Pdf, None, Duration::from_secs(10))
        .await
        .unwrap();
    // Let the bridge exit before it is picked again.
    tokio::time::sleep(Duration::from_millis(300)).await;
    lo.convert(&docx(), Format::Pdf, None, Duration::from_secs(10))
        .await
        .unwrap();
    settle(&lo, 1).await;
    let health = lo.health().await;
    assert!(health["restarts"].as_u64().unwrap() >= 1, "{health}");
    assert!(health["started"].as_u64().unwrap() >= 2, "{health}");
}

#[tokio::test]
async fn warm_up_starts_every_worker() {
    let dir = tempfile::tempdir().unwrap();
    let lo = pool(&dir, "ok", 3);
    lo.warm_up().await.unwrap();
    let health = lo.health().await;
    assert_eq!(health["idle"], 3);
    assert_eq!(health["started"], 3);
    // A second warm-up starts nothing.
    lo.warm_up().await.unwrap();
    assert_eq!(lo.health().await["started"], 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_run_in_parallel_up_to_the_pool_size() {
    let dir = tempfile::tempdir().unwrap();
    let lo = pool(&dir, "slow", 2);
    lo.warm_up().await.unwrap();
    let started = Instant::now();
    let jobs: Vec<_> = (0..4)
        .map(|_| {
            let lo = lo.clone();
            tokio::spawn(async move {
                lo.convert(&docx(), Format::Pdf, None, Duration::from_secs(10))
                    .await
                    .unwrap()
            })
        })
        .collect();
    for job in jobs {
        job.await.unwrap();
    }
    let elapsed = started.elapsed();
    // Four 300 ms jobs on two workers: two rounds, not four.
    assert!(elapsed >= Duration::from_millis(600), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(1100), "{elapsed:?}");
    assert_eq!(lo.health().await["started"], 2);
}

#[tokio::test]
async fn missing_bridge_is_an_engine_error() {
    let lo = LibreOffice::new(PathBuf::from("/nonexistent/soffice"), 1)
        .with_bridge(PathBuf::from("/nonexistent/bridge.py"));
    let err = lo
        .convert(&docx(), Format::Pdf, None, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Engine { .. }), "{err}");
    assert_eq!(lo.health().await["busy"], 0);
}
