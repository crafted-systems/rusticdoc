//! One warm LibreOffice: a `python3 unobridge.py` child that owns a
//! `soffice` and speaks JSON lines on its stdin and stdout.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::error::{Error, Result};

/// What a worker needs to start.
#[derive(Debug, Clone)]
pub(super) struct Launch {
    pub python: PathBuf,
    pub script: PathBuf,
    pub soffice: PathBuf,
    /// An initialised profile to copy, or `None` to start from an empty one.
    pub seed_profile: Option<PathBuf>,
    /// The mounted fonts directory, added to the worker's fontconfig.
    pub fonts_dir: Option<PathBuf>,
    /// Fonts for this worker alone, written to its private directory. A
    /// worker with these serves one job and is thrown away.
    pub extra_fonts: Vec<(String, Vec<u8>)>,
}

/// One conversion request, as the bridge reads it.
#[derive(Debug, Serialize)]
pub(super) struct Request<'a> {
    pub id: u64,
    pub input: &'a Path,
    pub output: &'a Path,
    /// An export filter name, or a bare target extension the bridge
    /// resolves against the application that opened the document.
    pub filter: &'a str,
    pub filter_options: &'a Map<String, Value>,
    pub password: Option<&'a str>,
    pub landscape: bool,
}

/// The bridge's answer to a request.
#[derive(Debug, Deserialize)]
pub(super) struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub pages: Option<u64>,
    #[serde(default)]
    pub ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Ready {
    ready: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    soffice_pid: Option<u32>,
}

pub(super) struct Worker {
    pub id: u64,
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    /// Profile and job files; removed with the worker.
    dir: tempfile::TempDir,
    /// The pool's font generation when this worker started.
    pub generation: u64,
    /// Conversions served so far.
    pub jobs: u64,
    pub last_used: Instant,
    next_request: u64,
    /// The `soffice` the bridge reported, in its own session; killed by pid.
    soffice_pid: Option<u32>,
}

/// A fontconfig file for one worker: the system's configuration, plus the
/// mounted directory and any fonts for this worker alone. File names are
/// generated, never taken from the upload.
async fn write_fontconfig(dir: &Path, launch: &Launch) -> Result<Option<PathBuf>> {
    if launch.fonts_dir.is_none() && launch.extra_fonts.is_empty() {
        return Ok(None);
    }
    let mut conf = String::from(
        "<?xml version=\"1.0\"?>\n<fontconfig>\n<include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include>\n",
    );
    if let Some(fonts_dir) = &launch.fonts_dir {
        conf.push_str(&format!(
            "<dir>{}</dir>\n",
            xml_escape(&fonts_dir.display().to_string())
        ));
    }
    if !launch.extra_fonts.is_empty() {
        let extra = dir.join("fonts");
        tokio::fs::create_dir(&extra).await?;
        for (i, (_, bytes)) in launch.extra_fonts.iter().enumerate() {
            tokio::fs::write(extra.join(format!("font-{i}.ttf")), bytes).await?;
        }
        conf.push_str(&format!(
            "<dir>{}</dir>\n",
            xml_escape(&extra.display().to_string())
        ));
    }
    conf.push_str("</fontconfig>\n");
    let path = dir.join("fonts.conf");
    tokio::fs::write(&path, conf).await?;
    Ok(Some(path))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl Worker {
    /// Starts the bridge and waits for its ready line.
    pub async fn start(id: u64, launch: &Launch, start_timeout: Duration) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("rusticdoc-worker-")
            .tempdir()?;
        let profile = dir.path().join("profile");
        match &launch.seed_profile {
            Some(seed) => {
                let (from, to) = (seed.clone(), profile.clone());
                tokio::task::spawn_blocking(move || super::copy_dir(&from, &to))
                    .await
                    .map_err(|e| engine(format!("profile copy: {e}")))??;
            }
            None => tokio::fs::create_dir(&profile).await?,
        }
        // LibreOffice honours TMPDIR for its temporary files; the private
        // worker directory keeps them away from other local users.
        let tmp = dir.path().join("tmp");
        tokio::fs::create_dir(&tmp).await?;
        let pipe = format!(
            "rusticdoc-{}-{id}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        let mut cmd = Command::new(&launch.python);
        if let Some(conf) = write_fontconfig(dir.path(), launch).await? {
            cmd.env("FONTCONFIG_FILE", conf);
        }
        cmd.arg(&launch.script)
            .arg("--socket-path")
            .arg(&pipe)
            .arg("--profile")
            .arg(&profile)
            .arg("--soffice")
            .arg(&launch.soffice)
            .env("HOME", dir.path())
            .env("TMPDIR", &tmp)
            .current_dir(dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            engine(format!(
                "cannot start the bridge {}: {e}",
                launch.python.display()
            ))
        })?;
        let stdin = child.stdin.take().ok_or_else(|| engine("bridge stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| engine("bridge stdout"))?;
        let mut worker = Self {
            id,
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            dir,
            generation: 0,
            jobs: 0,
            last_used: Instant::now(),
            next_request: 1,
            soffice_pid: None,
        };
        let line = match tokio::time::timeout(start_timeout, worker.stdout.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) | Ok(Err(_)) => {
                worker.kill().await;
                return Err(engine("the bridge exited before it was ready"));
            }
            Err(_) => {
                worker.kill().await;
                return Err(engine(format!(
                    "LibreOffice did not start within {start_timeout:?}"
                )));
            }
        };
        let ready: Ready = serde_json::from_str(&line)
            .map_err(|e| engine(format!("unexpected bridge greeting '{line}': {e}")))?;
        worker.soffice_pid = ready.soffice_pid;
        if !ready.ready {
            worker.kill().await;
            return Err(engine(format!(
                "LibreOffice did not start: {}",
                ready.error.unwrap_or_default()
            )));
        }
        Ok(worker)
    }

    /// Where this worker's job files go.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// False once the bridge process has exited.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn next_request_id(&mut self) -> u64 {
        let id = self.next_request;
        self.next_request += 1;
        id
    }

    /// Sends one request and reads its answer. An error means the bridge
    /// is gone; a `Response` with `ok: false` means the document failed
    /// and the worker is still usable.
    pub async fn call(&mut self, req: &Request<'_>) -> Result<Response> {
        let mut line = serde_json::to_vec(req).map_err(|e| engine(e.to_string()))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|e| engine(format!("the bridge went away: {e}")))?;
        let reply = self
            .stdout
            .next_line()
            .await
            .map_err(|e| engine(format!("the bridge went away: {e}")))?
            .ok_or_else(|| engine("the bridge exited during the conversion"))?;
        let resp: Response = serde_json::from_str(&reply)
            .map_err(|e| engine(format!("unexpected bridge reply '{reply}': {e}")))?;
        if resp.id != req.id {
            return Err(engine(format!(
                "bridge answered request {} instead of {}",
                resp.id, req.id
            )));
        }
        Ok(resp)
    }

    /// Kills the bridge's process group and its `soffice`, and reaps the
    /// bridge.
    pub async fn kill(&mut self) {
        self.kill_tree();
        let _ = self.child.wait().await;
    }

    /// SIGKILLs the bridge's process group (the bridge leads it) and the
    /// `soffice` it reported, which runs in a session of its own.
    fn kill_tree(&mut self) {
        // The bridge's process group is ours to signal: the bridge stays a
        // zombie until this side reaps it, so its group id cannot be
        // reused. The soffice pid is only signalled while the bridge is
        // still running; once the bridge has exited, PDEATHSIG and the
        // bridge's own cleanup have dealt with soffice, and the pid could
        // already belong to someone else.
        let bridge_alive = matches!(self.child.try_wait(), Ok(None));
        let mut targets: Vec<i32> = Vec::with_capacity(2);
        if let Some(pid) = self.child.id() {
            targets.push(-(pid as i32));
        }
        if let Some(pid) = self.soffice_pid.take()
            && bridge_alive
        {
            targets.push(pid as i32);
        }
        for target in targets {
            // SAFETY: kill(2) takes two integers and touches no memory.
            unsafe {
                libc::kill(target, libc::SIGKILL);
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.kill_tree();
    }
}

fn engine(message: impl Into<String>) -> Error {
    Error::Engine {
        engine: "libreoffice",
        message: message.into(),
    }
}
