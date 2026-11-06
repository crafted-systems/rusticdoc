//! Conversion with LibreOffice.
//!
//! A pool of warm `soffice` processes, each driven over UNO by a small
//! Python bridge (`bridge/unobridge.py`, embedded in the binary) that
//! reads JSON requests on stdin and answers on stdout. Workers start
//! lazily, in parallel, from a seed profile that is initialised once;
//! they are replaced after a number of jobs, after a timeout, or when they
//! die. The interface is the one the per-process version had.

mod options;
pub mod protection;
mod sheets;
mod worker;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Map;
use tokio::sync::{Mutex, OnceCell, OwnedSemaphorePermit, Semaphore};

use crate::error::{Error, Result};
use crate::job::{Document, Format, PdfaLevel};
use crate::process;

pub use options::{ConvertOptions, IMAGE_RESOLUTIONS};
use worker::{Launch, Request, Worker};

/// The bridge script, written next to the seed profile at first use.
const BRIDGE: &str = include_str!("../../bridge/unobridge.py");

/// LibreOffice's exit code when it has just created a profile and wants a
/// second start.
const EXIT_RESTART_REQUESTED: i32 = 81;

/// Jobs a worker serves before it is replaced, unless
/// `RUSTICDOC_LIBREOFFICE_MAX_JOBS` says otherwise.
const DEFAULT_MAX_JOBS: u64 = 10;

/// How long a worker may take to start and accept its first connection.
const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(60);

/// Settings written into the seed profile: no macros, no link updates, no
/// links from untrusted documents, so a crafted document cannot make
/// LibreOffice fetch a URL or read a local file during conversion.
///
/// The link-update enumerations differ per application: Writer's
/// `Content/Update/Link` is 0 = never, 1 = on request, 2 = always, while
/// Calc's is 0 = always, 1 = never, 2 = on request.
const PROFILE_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="BlockUntrustedRefererLinks" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Writer/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>0</value></prop></item>
<item oor:path="/org.openoffice.Office.Calc/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>1</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Misc"><prop oor:name="FirstRun" oor:op="fuse"><value>false</value></prop></item>
</oor:items>
"#;

/// A pool of warm LibreOffice workers. Cheap to clone; clones share the
/// pool.
#[derive(Clone)]
pub struct LibreOffice {
    inner: Arc<Inner>,
}

/// What the builders set; fixed once a worker exists.
#[derive(Debug, Clone)]
struct Settings {
    bin: PathBuf,
    python: PathBuf,
    /// A bridge script to run instead of the embedded one; it also skips
    /// the profile warm-up. For tests.
    bridge: Option<PathBuf>,
    workers: usize,
    max_jobs: u64,
    start_timeout: Duration,
    /// Added to every worker's fontconfig.
    fonts_dir: Option<PathBuf>,
}

struct Inner {
    settings: Settings,
    /// One permit per worker: holding one entitles the holder to a worker.
    slots: Arc<Semaphore>,
    idle: Mutex<Vec<Worker>>,
    seed: OnceCell<Seed>,
    next_id: AtomicU64,
    /// Bumped when fonts change; workers from older generations retire.
    generation: AtomicU64,
    started: AtomicU64,
    restarts: AtomicU64,
    /// Conversions waiting for a worker.
    queued: AtomicU64,
}

/// The seed directory: the bridge script and the initialised profile.
struct Seed {
    _dir: tempfile::TempDir,
    launch: Launch,
}

impl fmt::Debug for LibreOffice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibreOffice")
            .field("settings", &self.inner.settings)
            .finish()
    }
}

impl LibreOffice {
    /// A pool of `workers` LibreOffice processes started from `bin`. Reads
    /// `RUSTICDOC_LIBREOFFICE_MAX_JOBS` (jobs per worker before a restart,
    /// default 10) and `RUSTICDOC_PYTHON` (the interpreter that runs the
    /// bridge, default `python3`) from the environment.
    pub fn new(bin: PathBuf, workers: usize) -> Self {
        let workers = workers.max(1);
        let max_jobs = std::env::var("RUSTICDOC_LIBREOFFICE_MAX_JOBS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_JOBS);
        let python = std::env::var_os("RUSTICDOC_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        Self::from_settings(Settings {
            bin,
            python,
            bridge: None,
            workers,
            max_jobs,
            start_timeout: DEFAULT_START_TIMEOUT,
            fonts_dir: None,
        })
    }

    fn from_settings(settings: Settings) -> Self {
        let workers = settings.workers;
        Self {
            inner: Arc::new(Inner {
                settings,
                slots: Arc::new(Semaphore::new(workers)),
                idle: Mutex::new(Vec::with_capacity(workers)),
                seed: OnceCell::new(),
                next_id: AtomicU64::new(1),
                generation: AtomicU64::new(0),
                started: AtomicU64::new(0),
                restarts: AtomicU64::new(0),
                queued: AtomicU64::new(0),
            }),
        }
    }

    /// Builders apply before any worker exists, so the pool is rebuilt.
    fn rebuild(self, change: impl FnOnce(&mut Settings)) -> Self {
        let mut settings = self.inner.settings.clone();
        change(&mut settings);
        Self::from_settings(settings)
    }

    /// Jobs a worker serves before it is replaced.
    pub fn with_max_jobs(self, max_jobs: u64) -> Self {
        self.rebuild(|s| s.max_jobs = max_jobs.max(1))
    }

    /// How long a worker may take to start.
    pub fn with_start_timeout(self, timeout: Duration) -> Self {
        self.rebuild(|s| s.start_timeout = timeout)
    }

    /// Runs `script` as the bridge instead of the embedded one and skips
    /// the LibreOffice profile warm-up, so the pool can be exercised
    /// without LibreOffice. Meant for tests.
    pub fn with_bridge(self, script: PathBuf) -> Self {
        self.rebuild(|s| s.bridge = Some(script))
    }

    /// Adds a directory of mounted fonts to every worker's fontconfig.
    pub fn with_fonts_dir(self, dir: PathBuf) -> Self {
        self.rebuild(|s| s.fonts_dir = Some(dir))
    }

    /// Makes workers see a changed fonts directory: idle workers go now and
    /// restart, and busy ones are replaced when their job ends.
    pub async fn reload_fonts(&self) -> Result<()> {
        self.inner.generation.fetch_add(1, Ordering::Relaxed);
        let idle: Vec<Worker> = self.inner.idle.lock().await.drain(..).collect();
        let dropped = idle.len() as u64;
        for mut worker in idle {
            worker.kill().await;
        }
        self.inner.restarts.fetch_add(dropped, Ordering::Relaxed);
        self.warm_up().await
    }

    /// Conversions waiting for a worker, and workers replaced so far.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.inner.queued.load(Ordering::Relaxed),
            self.inner.restarts.load(Ordering::Relaxed),
        )
    }

    /// The `soffice` binary.
    pub fn bin(&self) -> &PathBuf {
        &self.inner.settings.bin
    }

    /// Converts `doc` to `to`. A `pdfa` level makes the PDF export a PDF/A
    /// export; it is ignored for other targets.
    pub async fn convert(
        &self,
        doc: &Document,
        to: Format,
        pdfa: Option<PdfaLevel>,
        timeout: Duration,
    ) -> Result<Document> {
        self.convert_with(doc, to, &ConvertOptions::pdfa(pdfa), timeout)
            .await
    }

    /// Converts `doc` to `to` with every option. The conversion itself
    /// must finish within `timeout`; a worker that does not answer in time
    /// is killed and [`Error::Timeout`] returned. A document LibreOffice
    /// rejects gives [`Error::Engine`] and keeps the worker.
    pub async fn convert_with(
        &self,
        doc: &Document,
        to: Format,
        options: &ConvertOptions,
        timeout: Duration,
    ) -> Result<Document> {
        let ext = doc.extension();
        if ext.is_empty() {
            return Err(Error::InvalidRequest(format!(
                "'{}' has no extension, so its format is unknown",
                doc.name
            )));
        }
        options.validate()?;
        self.inner.queued.fetch_add(1, Ordering::Relaxed);
        let permit = self.inner.slots.clone().acquire_owned().await;
        self.inner.queued.fetch_sub(1, Ordering::Relaxed);
        let permit =
            permit.map_err(|_| Error::EngineUnavailable("LibreOffice pool closed".into()))?;
        let dedicated = !options.fonts.is_empty();
        let mut worker = if dedicated {
            self.start_dedicated(&options.fonts).await?
        } else {
            self.checkout().await?
        };

        let job_dir = worker.dir().join(format!("job-{}", worker.jobs));
        tokio::fs::create_dir(&job_dir).await?;
        let input = job_dir.join(format!("input.{ext}"));
        let output = job_dir.join(format!("output.{}", to.extension()));
        if options.single_page_sheets {
            let (bytes, ext) = (doc.bytes.clone(), ext.clone());
            let bytes = tokio::task::spawn_blocking(move || sheets::reset_scroll(bytes, &ext))
                .await
                .map_err(|e| Error::Engine {
                    engine: "libreoffice",
                    message: e.to_string(),
                })?;
            tokio::fs::write(&input, &bytes).await?;
        } else {
            tokio::fs::write(&input, &doc.bytes).await?;
        }
        let empty = Map::new();
        let req = Request {
            id: worker.next_request_id(),
            input: &input,
            output: &output,
            filter: to.extension(),
            filter_options: if to == Format::Pdf {
                &options.filter_data()
            } else {
                &empty
            },
            password: options.password.as_deref(),
            landscape: options.landscape,
            update_indexes: options.update_indexes,
        };
        let started = Instant::now();
        let result = match tokio::time::timeout(timeout, worker.call(&req)).await {
            Err(_) => {
                if dedicated {
                    worker.kill().await;
                } else {
                    self.retire(worker, permit, "timed out");
                }
                return Err(Error::Timeout(timeout));
            }
            Ok(Err(e)) => {
                if dedicated {
                    worker.kill().await;
                } else {
                    self.retire(worker, permit, &e.to_string());
                }
                return Err(e);
            }
            Ok(Ok(resp)) if !resp.ok => Err(Error::Engine {
                engine: "libreoffice",
                message: resp.error.unwrap_or_else(|| "unknown error".into()),
            }),
            Ok(Ok(resp)) => {
                tracing::debug!(
                    worker = worker.id,
                    pages = resp.pages,
                    bridge_ms = resp.ms,
                    total_ms = started.elapsed().as_millis(),
                    "converted"
                );
                tokio::fs::read(&output)
                    .await
                    .map(|bytes| Document::new(doc.with_extension(to.extension()), bytes))
                    .map_err(|_| Error::Engine {
                        engine: "libreoffice",
                        message: format!("no {} was produced for {}", to.extension(), doc.name),
                    })
            }
        };
        let _ = tokio::fs::remove_dir_all(&job_dir).await;
        worker.jobs += 1;
        worker.last_used = Instant::now();
        if dedicated {
            worker.kill().await;
            drop(permit);
            return result;
        }
        if worker.generation != self.inner.generation.load(Ordering::Relaxed) {
            self.retire(worker, permit, "fonts changed");
        } else if !worker.is_alive() {
            self.retire(worker, permit, "the bridge exited");
        } else if worker.jobs >= self.inner.settings.max_jobs {
            self.retire(worker, permit, "served its jobs");
        } else {
            self.inner.idle.lock().await.push(worker);
            drop(permit);
        }
        result
    }

    /// Starts every worker that is not running yet, in parallel, so the
    /// first requests find them warm.
    pub async fn warm_up(&self) -> Result<()> {
        let mut permits = Vec::new();
        while let Ok(permit) = self.inner.slots.clone().try_acquire_owned() {
            permits.push(permit);
        }
        let missing = permits
            .len()
            .saturating_sub(self.inner.idle.lock().await.len());
        let mut starts = tokio::task::JoinSet::new();
        for _ in 0..missing {
            let pool = self.clone();
            starts.spawn(async move { pool.start_worker().await });
        }
        let mut first_error = None;
        while let Some(started) = starts.join_next().await {
            match started {
                Ok(Ok(worker)) => self.inner.idle.lock().await.push(worker),
                Ok(Err(e)) => first_error = first_error.or(Some(e)),
                Err(e) => {
                    first_error = first_error.or(Some(Error::Engine {
                        engine: "libreoffice",
                        message: format!("worker start: {e}"),
                    }))
                }
            }
        }
        drop(permits);
        first_error.map_or(Ok(()), Err)
    }

    /// Pool state for a health endpoint: `workers`, `idle`, `busy`,
    /// `started`, `restarts`, `max_jobs`.
    pub async fn health(&self) -> serde_json::Value {
        let i = &self.inner;
        serde_json::json!({
            "workers": i.settings.workers,
            "idle": i.idle.lock().await.len(),
            "busy": i.settings.workers - i.slots.available_permits(),
            "started": i.started.load(Ordering::Relaxed),
            "restarts": i.restarts.load(Ordering::Relaxed),
            "max_jobs": i.settings.max_jobs,
        })
    }

    /// An idle worker, or a new one. The caller holds a permit.
    async fn checkout(&self) -> Result<Worker> {
        loop {
            let Some(mut worker) = self.inner.idle.lock().await.pop() else {
                return self.start_worker().await;
            };
            if worker.is_alive() {
                return Ok(worker);
            }
            tracing::info!(worker = worker.id, "LibreOffice worker died; replacing it");
            self.inner.restarts.fetch_add(1, Ordering::Relaxed);
            worker.kill().await;
        }
    }

    /// A worker with extra fonts, for one job.
    async fn start_dedicated(&self, fonts: &[(String, Vec<u8>)]) -> Result<Worker> {
        let seed = self.seed().await?;
        let mut launch = seed.launch.clone();
        launch.extra_fonts = fonts.to_vec();
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        Worker::start(id, &launch, self.inner.settings.start_timeout).await
    }

    async fn start_worker(&self) -> Result<Worker> {
        let seed = self.seed().await?;
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let generation = self.inner.generation.load(Ordering::Relaxed);
        let mut worker = Worker::start(id, &seed.launch, self.inner.settings.start_timeout).await?;
        worker.generation = generation;
        self.inner.started.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            worker = id,
            ms = started.elapsed().as_millis(),
            "LibreOffice worker started"
        );
        Ok(worker)
    }

    /// Kills `worker` and starts its replacement in the background, holding
    /// the permit meanwhile so the pool never exceeds its size.
    fn retire(&self, mut worker: Worker, permit: OwnedSemaphorePermit, reason: &str) {
        tracing::info!(
            worker = worker.id,
            jobs = worker.jobs,
            reason,
            "LibreOffice worker restarting"
        );
        self.inner.restarts.fetch_add(1, Ordering::Relaxed);
        let pool = self.clone();
        tokio::spawn(async move {
            worker.kill().await;
            match pool.start_worker().await {
                Ok(fresh) => pool.inner.idle.lock().await.push(fresh),
                Err(e) => tracing::warn!(error = %e, "LibreOffice worker could not be replaced"),
            }
            drop(permit);
        });
    }

    /// The seed directory, made on first use: the bridge script and a
    /// profile initialised by starting LibreOffice once and letting it
    /// exit.
    async fn seed(&self) -> Result<&Seed> {
        let inner = &self.inner;
        let settings = &inner.settings;
        inner
            .seed
            .get_or_try_init(|| async {
                // A fresh directory with a random name and mode 0700, so no
                // other local user can plant a path or a symlink there.
                let dir = tempfile::Builder::new()
                    .prefix("rusticdoc-seed-")
                    .tempdir()?;
                let (script, seed_profile) = match &settings.bridge {
                    Some(script) => (script.clone(), None),
                    None => {
                        let script = dir.path().join("unobridge.py");
                        tokio::fs::write(&script, BRIDGE).await?;
                        let profile = dir.path().join("profile");
                        init_profile(&settings.bin, &profile, settings.start_timeout).await?;
                        (script, Some(profile))
                    }
                };
                Ok::<_, Error>(Seed {
                    launch: Launch {
                        python: settings.python.clone(),
                        script,
                        soffice: settings.bin.clone(),
                        seed_profile,
                        fonts_dir: settings.fonts_dir.clone(),
                        extra_fonts: Vec::new(),
                    },
                    _dir: dir,
                })
            })
            .await
    }
}

/// Writes the settings into a new profile at `dir` and lets LibreOffice
/// finish it with `--terminate_after_init`.
async fn init_profile(bin: &Path, dir: &Path, timeout: Duration) -> Result<()> {
    tokio::fs::create_dir_all(dir.join("user")).await?;
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(dir.join("user/registrymodifications.xcu"))
        .await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, PROFILE_SETTINGS.as_bytes()).await?;
    drop(file);
    let args = vec![
        format!("-env:UserInstallation=file://{}", dir.display()),
        "--headless".into(),
        "--norestore".into(),
        "--nologo".into(),
        "--terminate_after_init".into(),
    ];
    let env = [("HOME", dir.display().to_string())];
    match process::run("libreoffice", bin, &args, dir, &env, timeout).await {
        Ok(()) => {}
        Err(Error::Engine { message, .. }) if is_restart_request(&message) => {}
        Err(e) => return Err(e),
    }
    tracing::info!(path = %dir.display(), "LibreOffice profile initialised");
    Ok(())
}

fn is_restart_request(message: &str) -> bool {
    let code = message.strip_prefix("exit ").map(|rest| {
        rest.split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap_or("")
    });
    code.and_then(|c| c.parse::<i32>().ok()) == Some(EXIT_RESTART_REQUESTED)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_requests_are_recognised() {
        assert!(is_restart_request("exit 81: some log line"));
        assert!(!is_restart_request("exit 1"));
        assert!(!is_restart_request("exit 811"));
    }

    #[tokio::test]
    async fn missing_binary_is_an_engine_error() {
        let lo = LibreOffice::new(PathBuf::from("/nonexistent/soffice"), 1);
        let doc = Document::new("a.txt", b"hi".to_vec());
        let err = lo
            .convert(&doc, Format::Pdf, None, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::Engine {
                    engine: "libreoffice",
                    ..
                }
            ),
            "{err}"
        );
        assert_eq!(lo.health().await["busy"], 0);
    }

    #[test]
    fn copy_dir_copies_nested_files() {
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("a/b")).unwrap();
        std::fs::write(src.path().join("a/b/c.txt"), b"x").unwrap();
        let dst = tempfile::tempdir().unwrap();
        copy_dir(src.path(), &dst.path().join("p")).unwrap();
        assert_eq!(std::fs::read(dst.path().join("p/a/b/c.txt")).unwrap(), b"x");
    }

    #[test]
    fn bridge_script_is_embedded() {
        assert!(BRIDGE.contains("--socket-path"));
        assert!(BRIDGE.lines().count() <= 275, "keep the bridge small");
    }
}
