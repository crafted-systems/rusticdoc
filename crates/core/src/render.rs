//! Evaluating client template code, in a child process when it matters.
//!
//! A template can build values it never prints: `{% set x = x ~ x %}` doubles a
//! string for a few instructions, so neither fuel nor a cap on output bounds its
//! memory, and an allocation failure aborts the whole process. In a service,
//! every evaluation therefore runs in `rusticdoc __render-worker`, a child with
//! no environment and its own limits on address space, CPU, core files and file
//! descriptors. When it dies, the request fails and the service carries on.
//!
//! The CLI renders the user's own templates and stays in-process.
//!
//! The parent and the worker speak frames on stdin and stdout: one byte of kind,
//! a `u32` count of blobs, then each blob as a `u64` length and its bytes, all
//! little-endian. The first blob of a frame is JSON; the rest are raw.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::{Error, Result};
use crate::job::Document;
use crate::pdfform::{self, FillLimits, FillOptions};
use crate::pdfops::{Deadline, blocking};
use crate::template;

/// The argument that makes the binary a worker.
pub const WORKER_COMMAND: &str = "__render-worker";

/// Longest name a batch's `batchName` may render to, in bytes.
pub const MAX_BATCH_NAME_BYTES: usize = 4096;

/// File descriptors a worker may hold: it only uses its standard streams.
const WORKER_NOFILE: u64 = 16;

const FRAME_REQUEST: u8 = 1;
const FRAME_REPLY: u8 = 2;
const FRAME_ERROR: u8 = 3;

/// One evaluation, as the worker receives it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Op {
    /// Answers with the limit it runs under, to prove it can start.
    Ping,
    /// A template file. Blobs: the file, the data, the context.
    Render { name: String, limit: usize },
    /// Blobs: the file, the data, the context.
    Validate { name: String, limit: usize },
    /// A template string, for `batchName`. Blobs: the source, the context.
    Text { limit: usize },
    /// A PDF form. Blobs: the file, the data.
    Fill {
        flatten: bool,
        strip_tag_annotations: bool,
        limit: usize,
        timeout_ms: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct ReplyMeta {
    /// The name of a rendered document.
    #[serde(default)]
    name: String,
    /// The address-space limit a pinged worker runs under.
    #[serde(default)]
    address_space: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireError {
    kind: String,
    message: String,
    #[serde(default)]
    status: u16,
}

fn to_wire(e: &Error) -> WireError {
    let (kind, status) = match e {
        Error::InvalidRequest(_) => ("invalid", 0),
        Error::Template(_) => ("template", 0),
        Error::Unsupported(_) => ("unsupported", 0),
        Error::Status(code, _) => ("status", *code),
        Error::Timeout(_) => ("timeout", 0),
        _ => ("engine", 0),
    };
    let message = match e {
        Error::InvalidRequest(m) | Error::Template(m) | Error::Unsupported(m) => m.clone(),
        Error::Status(_, m) => m.clone(),
        other => other.to_string(),
    };
    WireError {
        kind: kind.into(),
        message,
        status,
    }
}

fn from_wire(e: WireError, timeout: Duration) -> Error {
    match e.kind.as_str() {
        "invalid" => Error::InvalidRequest(e.message),
        "template" => Error::Template(e.message),
        "unsupported" => Error::Unsupported(e.message),
        "status" => Error::Status(e.status, e.message),
        "timeout" => Error::Timeout(timeout),
        _ => Error::Engine {
            engine: "template",
            message: e.message,
        },
    }
}

fn write_frame(out: &mut impl Write, kind: u8, blobs: &[&[u8]]) -> std::io::Result<()> {
    out.write_all(&[kind])?;
    out.write_all(&(blobs.len() as u32).to_le_bytes())?;
    for blob in blobs {
        out.write_all(&(blob.len() as u64).to_le_bytes())?;
        out.write_all(blob)?;
    }
    out.flush()
}

/// One frame, or `None` at the end of the stream.
fn read_frame(input: &mut impl Read) -> std::io::Result<Option<(u8, Vec<Vec<u8>>)>> {
    let mut kind = [0u8; 1];
    if input.read(&mut kind)? == 0 {
        return Ok(None);
    }
    let mut count = [0u8; 4];
    input.read_exact(&mut count)?;
    let mut blobs = Vec::new();
    for _ in 0..u32::from_le_bytes(count) {
        let mut len = [0u8; 8];
        input.read_exact(&mut len)?;
        let mut blob = vec![0u8; u64::from_le_bytes(len) as usize];
        input.read_exact(&mut blob)?;
        blobs.push(blob);
    }
    Ok(Some((kind[0], blobs)))
}

/// What the binary does as a worker: answers frames until its stdin closes.
/// Runs without a runtime, and never reads the environment.
pub fn worker_main() -> i32 {
    #[cfg(target_os = "linux")]
    {
        // Under memory pressure the kernel takes this process first, not the
        // service.
        let _ = std::fs::write("/proc/self/oom_score_adj", "1000");
    }
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let (kind, blobs) = match read_frame(&mut input) {
            Ok(Some(frame)) => frame,
            Ok(None) => return 0,
            Err(_) => return 2,
        };
        if kind != FRAME_REQUEST {
            return 2;
        }
        let written = match evaluate(&blobs) {
            Ok((meta, payload)) => match serde_json::to_vec(&meta) {
                Ok(meta) => write_frame(&mut output, FRAME_REPLY, &[&meta, &payload]),
                Err(_) => return 2,
            },
            Err(e) => match serde_json::to_vec(&to_wire(&e)) {
                Ok(error) => write_frame(&mut output, FRAME_ERROR, &[&error]),
                Err(_) => return 2,
            },
        };
        if written.is_err() {
            return 2;
        }
    }
}

fn blob_json(blobs: &[Vec<u8>], at: usize) -> Result<serde_json::Value> {
    serde_json::from_slice(blobs.get(at).map(Vec::as_slice).unwrap_or(b"null"))
        .map_err(|e| Error::InvalidRequest(format!("the render request is malformed: {e}")))
}

fn blob_bytes(blobs: &[Vec<u8>], at: usize) -> Vec<u8> {
    blobs.get(at).cloned().unwrap_or_default()
}

fn evaluate(blobs: &[Vec<u8>]) -> Result<(ReplyMeta, Vec<u8>)> {
    let op: Op = serde_json::from_slice(blobs.first().map(Vec::as_slice).unwrap_or(b"{}"))
        .map_err(|e| Error::InvalidRequest(format!("the render request is malformed: {e}")))?;
    let plain = |payload: Vec<u8>| ReplyMetaAnd {
        meta: ReplyMeta {
            name: String::new(),
            address_space: None,
        },
        payload,
    };
    let done = match op {
        Op::Ping => ReplyMetaAnd {
            meta: ReplyMeta {
                name: String::new(),
                address_space: address_space_limit(),
            },
            payload: Vec::new(),
        },
        Op::Render { name, limit } => {
            let doc = Document::new(name, blob_bytes(blobs, 1));
            let rendered =
                template::render(&doc, &blob_json(blobs, 2)?, &blob_json(blobs, 3)?, limit)?;
            ReplyMetaAnd {
                meta: ReplyMeta {
                    name: rendered.name,
                    address_space: None,
                },
                payload: rendered.bytes.to_vec(),
            }
        }
        Op::Validate { name, limit } => {
            let doc = Document::new(name, blob_bytes(blobs, 1));
            let found =
                template::validate(&doc, &blob_json(blobs, 2)?, &blob_json(blobs, 3)?, limit);
            plain(serde_json::to_vec(&found).map_err(|e| Error::Engine {
                engine: "template",
                message: e.to_string(),
            })?)
        }
        Op::Text { limit } => {
            let source = String::from_utf8_lossy(blobs.get(1).map(Vec::as_slice).unwrap_or(b""));
            let ctx = blob_json(blobs, 2)?;
            let env = template::environment();
            let text = template::render_str(
                &env,
                &source,
                minijinja::Value::from(minijinja::value::Serde(&ctx)),
                limit,
            )?;
            plain(text.into_bytes())
        }
        Op::Fill {
            flatten,
            strip_tag_annotations,
            limit,
            timeout_ms,
        } => {
            let env = template::environment();
            let options = FillOptions {
                flatten,
                strip_tag_annotations,
            };
            let limits = FillLimits {
                max_render_bytes: limit,
                deadline: Deadline::new(Duration::from_millis(timeout_ms)),
            };
            let pdf = blobs.get(1).map(Vec::as_slice).unwrap_or(b"");
            plain(pdfform::fill_within(
                pdf,
                &blob_json(blobs, 2)?,
                &env,
                &options,
                limits,
            )?)
        }
    };
    Ok((done.meta, done.payload))
}

struct ReplyMetaAnd {
    meta: ReplyMeta,
    payload: Vec<u8>,
}

/// The address-space limit this process runs under, when it has one.
fn address_space_limit() -> Option<u64> {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit fills in the struct it is handed.
        let status = unsafe { libc::getrlimit(libc::RLIMIT_AS as _, &mut limit) };
        if status == 0 && limit.rlim_cur != libc::RLIM_INFINITY {
            return Some(limit.rlim_cur as u64);
        }
    }
    None
}

#[derive(Debug)]
struct Isolation {
    bin: PathBuf,
    memory: u64,
    permits: Arc<Semaphore>,
}

/// Where template code runs. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct Renderer {
    isolation: Option<Arc<Isolation>>,
}

impl Renderer {
    /// Evaluates in this process, for templates the user wrote themselves.
    pub fn in_process() -> Self {
        Self::default()
    }

    /// Evaluates in `bin __render-worker`, at most `workers` at once, each
    /// limited to `memory` bytes of address space.
    pub fn isolated(bin: PathBuf, memory: u64, workers: usize) -> Self {
        Self {
            isolation: Some(Arc::new(Isolation {
                bin,
                memory,
                permits: Arc::new(Semaphore::new(workers.max(1))),
            })),
        }
    }

    pub fn is_isolated(&self) -> bool {
        self.isolation.is_some()
    }

    /// Starts a worker and checks that it runs under its limit. In-process, there
    /// is nothing to check.
    pub async fn check(&self) -> Result<()> {
        let Some(isolation) = &self.isolation else {
            return Ok(());
        };
        let timeout = Duration::from_secs(10);
        let mut session = self.session(timeout).await?;
        let (meta, _) = session.call(&Op::Ping, &[]).await?;
        if cfg!(target_os = "linux") && meta.address_space != Some(isolation.memory) {
            return Err(Error::Engine {
                engine: "template",
                message: format!(
                    "the render worker runs with an address-space limit of {:?}, not {}",
                    meta.address_space, isolation.memory
                ),
            });
        }
        Ok(())
    }

    /// A worker for several evaluations of one job, or nothing in-process. It
    /// holds its permit until dropped.
    pub async fn session(&self, timeout: Duration) -> Result<Session> {
        let deadline = Instant::now() + timeout;
        let Some(isolation) = &self.isolation else {
            return Ok(Session {
                worker: None,
                deadline,
                timeout,
            });
        };
        let permit = tokio::time::timeout(timeout, isolation.permits.clone().acquire_owned())
            .await
            .map_err(|_| Error::Timeout(timeout))?
            .map_err(|_| Error::EngineUnavailable("the render workers are closed".into()))?;
        let worker = Worker::start(isolation, timeout, permit)?;
        Ok(Session {
            worker: Some(worker),
            deadline,
            timeout,
        })
    }

    /// Fills a template.
    pub async fn render(
        &self,
        doc: &Document,
        data: &serde_json::Value,
        ctx: &serde_json::Value,
        limit: usize,
        timeout: Duration,
    ) -> Result<Document> {
        self.session(timeout)
            .await?
            .render(doc, data, ctx, limit)
            .await
    }

    /// Checks a template against sample data.
    pub async fn validate(
        &self,
        doc: &Document,
        data: &serde_json::Value,
        ctx: &serde_json::Value,
        limit: usize,
        timeout: Duration,
    ) -> Result<Vec<rusticdoc_template::Diagnostic>> {
        let mut session = self.session(timeout).await?;
        if session.worker.is_none() {
            let (doc, data, ctx) = (doc.clone(), data.clone(), ctx.clone());
            return blocking(timeout, move |_| {
                Ok(template::validate(&doc, &data, &ctx, limit))
            })
            .await;
        }
        let op = Op::Validate {
            name: doc.name.clone(),
            limit,
        };
        let (data, ctx) = encode_pair(data, ctx).await?;
        let (_, payload) = session.call(&op, &[&doc.bytes, &data, &ctx]).await?;
        serde_json::from_slice(&payload).map_err(|e| Error::Engine {
            engine: "template",
            message: format!("unexpected validation reply: {e}"),
        })
    }

    /// Fills a PDF form under one budget of fuel, output and time.
    pub async fn fill(
        &self,
        pdf: &bytes::Bytes,
        data: &serde_json::Value,
        options: &FillOptions,
        limit: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let mut session = self.session(timeout).await?;
        if session.worker.is_none() {
            let (pdf, data, options) = (pdf.clone(), data.clone(), options.clone());
            return blocking(timeout, move |deadline| {
                let env = template::environment();
                let limits = FillLimits {
                    max_render_bytes: limit,
                    deadline,
                };
                pdfform::fill_within(&pdf, &data, &env, &options, limits)
            })
            .await;
        }
        let op = Op::Fill {
            flatten: options.flatten,
            strip_tag_annotations: options.strip_tag_annotations,
            limit,
            timeout_ms: timeout.as_millis() as u64,
        };
        let (data, _) = encode_pair(data, &serde_json::Value::Null).await?;
        let (_, payload) = session.call(&op, &[pdf, &data]).await?;
        Ok(payload)
    }
}

/// Serializes off the async threads: the data can be as large as the body.
async fn encode_pair(
    first: &serde_json::Value,
    second: &serde_json::Value,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let (first, second) = (first.clone(), second.clone());
    tokio::task::spawn_blocking(move || {
        let encode = |v: &serde_json::Value| {
            serde_json::to_vec(v).map_err(|e| Error::InvalidRequest(e.to_string()))
        };
        Ok::<_, Error>((encode(&first)?, encode(&second)?))
    })
    .await
    .map_err(|e| Error::Engine {
        engine: "template",
        message: e.to_string(),
    })?
}

/// One running worker. Dropping it kills the process.
struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    /// Replies past this are refused: a worker cannot hold more.
    memory: u64,
    _permit: OwnedSemaphorePermit,
}

impl Worker {
    fn start(
        isolation: &Isolation,
        timeout: Duration,
        permit: OwnedSemaphorePermit,
    ) -> Result<Self> {
        let mut cmd = Command::new(&isolation.bin);
        cmd.arg(WORKER_COMMAND)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let memory = isolation.memory;
        let cpu = timeout.as_secs().saturating_add(1);
        #[cfg(unix)]
        // SAFETY: the closure only calls setrlimit, which is async-signal-safe,
        // and allocates nothing.
        unsafe {
            cmd.pre_exec(move || {
                let set = |resource, soft: u64, hard: u64| {
                    let limit = libc::rlimit {
                        rlim_cur: soft as _,
                        rlim_max: hard as _,
                    };
                    if libc::setrlimit(resource as _, &limit) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                };
                set(libc::RLIMIT_CORE, 0, 0)?;
                set(libc::RLIMIT_NOFILE, WORKER_NOFILE, WORKER_NOFILE)?;
                set(libc::RLIMIT_CPU, cpu, cpu.saturating_add(1))?;
                let address_space = set(libc::RLIMIT_AS, memory, memory);
                // macOS accepts or refuses this as it likes, and does not
                // enforce it. On Linux it is the point of the exercise.
                if cfg!(target_os = "linux") {
                    address_space?;
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| Error::Engine {
            engine: "template",
            message: format!(
                "cannot start the render worker {}: {e}",
                isolation.bin.display()
            ),
        })?;
        let unavailable = || Error::Engine {
            engine: "template",
            message: "the render worker has no standard streams".into(),
        };
        let stdin = child.stdin.take().ok_or_else(unavailable)?;
        let stdout = child.stdout.take().ok_or_else(unavailable)?;
        Ok(Self {
            child,
            stdin,
            stdout,
            memory,
            _permit: permit,
        })
    }

    async fn send(&mut self, op: &Op, blobs: &[&[u8]]) -> std::io::Result<()> {
        let meta = serde_json::to_vec(op).map_err(std::io::Error::other)?;
        let count = blobs.len() as u32 + 1;
        let mut head = vec![FRAME_REQUEST];
        head.extend_from_slice(&count.to_le_bytes());
        head.extend_from_slice(&(meta.len() as u64).to_le_bytes());
        head.extend_from_slice(&meta);
        self.stdin.write_all(&head).await?;
        for blob in blobs {
            self.stdin
                .write_all(&(blob.len() as u64).to_le_bytes())
                .await?;
            self.stdin.write_all(blob).await?;
        }
        self.stdin.flush().await
    }

    async fn receive(&mut self) -> std::io::Result<(u8, Vec<Vec<u8>>)> {
        let kind = self.stdout.read_u8().await?;
        let count = self.stdout.read_u32_le().await?;
        if count > 4 {
            return Err(std::io::Error::other("too many blobs in a reply"));
        }
        let mut blobs = Vec::new();
        for _ in 0..count {
            let len = self.stdout.read_u64_le().await?;
            if len > self.memory {
                return Err(std::io::Error::other("a reply larger than the worker"));
            }
            let mut blob = vec![0u8; len as usize];
            self.stdout.read_exact(&mut blob).await?;
            blobs.push(blob);
        }
        Ok((kind, blobs))
    }

    /// What became of a worker that stopped talking, as the client should hear
    /// it. The log has the rest.
    async fn failure(&mut self) -> Error {
        let status = match tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await {
            Ok(Ok(status)) => status,
            _ => {
                let _ = self.child.start_kill();
                return died();
            }
        };
        tracing::warn!(%status, "the render worker died");
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            match status.signal() {
                // An allocation that failed, or the kernel's choice under pressure.
                Some(libc::SIGABRT) | Some(libc::SIGKILL) | Some(libc::SIGBUS) => {
                    return Error::Status(
                        413,
                        "The template needs more memory than one render may use.".into(),
                    );
                }
                Some(libc::SIGXCPU) => {
                    return Error::Status(
                        422,
                        "The template used more CPU time than one render may use.".into(),
                    );
                }
                _ => {}
            }
        }
        died()
    }
}

fn died() -> Error {
    Error::Status(422, "The template could not be rendered.".into())
}

/// One job's evaluations, on one worker when isolated.
pub struct Session {
    worker: Option<Worker>,
    deadline: Instant,
    timeout: Duration,
}

impl Session {
    fn remaining(&self) -> Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or(Error::Timeout(self.timeout))
    }

    async fn call(&mut self, op: &Op, blobs: &[&[u8]]) -> Result<(ReplyMeta, Vec<u8>)> {
        let left = self.remaining()?;
        let timeout = self.timeout;
        let Some(worker) = self.worker.as_mut() else {
            return Err(Error::Engine {
                engine: "template",
                message: "no render worker in this session".into(),
            });
        };
        let exchange = async {
            worker.send(op, blobs).await?;
            worker.receive().await
        };
        let reply = tokio::time::timeout(left, exchange)
            .await
            .map_err(|_| Error::Timeout(timeout))?;
        let (kind, mut frame) = match reply {
            Ok(reply) => reply,
            Err(e) => {
                tracing::debug!(error = %e, "the render worker stopped answering");
                return Err(worker.failure().await);
            }
        };
        match kind {
            FRAME_REPLY if frame.len() == 2 => {
                let payload = frame.pop().unwrap_or_default();
                let meta = serde_json::from_slice(&frame[0]).map_err(|e| Error::Engine {
                    engine: "template",
                    message: format!("unexpected worker reply: {e}"),
                })?;
                Ok((meta, payload))
            }
            FRAME_ERROR if frame.len() == 1 => {
                let error = serde_json::from_slice(&frame[0]).map_err(|e| Error::Engine {
                    engine: "template",
                    message: format!("unexpected worker error: {e}"),
                })?;
                Err(from_wire(error, timeout))
            }
            _ => Err(Error::Engine {
                engine: "template",
                message: "unexpected frame from the render worker".into(),
            }),
        }
    }

    /// Fills a template.
    pub async fn render(
        &mut self,
        doc: &Document,
        data: &serde_json::Value,
        ctx: &serde_json::Value,
        limit: usize,
    ) -> Result<Document> {
        if self.worker.is_none() {
            let (doc, data, ctx) = (doc.clone(), data.clone(), ctx.clone());
            return blocking(self.remaining()?, move |_| {
                template::render(&doc, &data, &ctx, limit)
            })
            .await;
        }
        let op = Op::Render {
            name: doc.name.clone(),
            limit,
        };
        let (data, ctx) = encode_pair(data, ctx).await?;
        let (meta, payload) = self.call(&op, &[&doc.bytes, &data, &ctx]).await?;
        Ok(Document::new(meta.name, payload))
    }

    /// Renders a template string, such as a batch's file names.
    pub async fn text(
        &mut self,
        source: &str,
        ctx: &serde_json::Value,
        limit: usize,
    ) -> Result<String> {
        if self.worker.is_none() {
            let (source, ctx) = (source.to_string(), ctx.clone());
            return blocking(self.remaining()?, move |_| {
                let env = template::environment();
                template::render_str(
                    &env,
                    &source,
                    minijinja::Value::from(minijinja::value::Serde(&ctx)),
                    limit,
                )
            })
            .await;
        }
        let (ctx, _) = encode_pair(ctx, &serde_json::Value::Null).await?;
        let (_, payload) = self
            .call(&Op::Text { limit }, &[source.as_bytes(), &ctx])
            .await?;
        Ok(String::from_utf8_lossy(&payload).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(20);

    /// A stand-in for the binary: `body` runs instead of a worker.
    fn fake(dir: &tempfile::TempDir, body: &str) -> Renderer {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join("worker.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Renderer::isolated(path, 1 << 34, 1)
    }

    fn doc() -> Document {
        Document::new("t.txt", b"hello".to_vec())
    }

    async fn render(renderer: &Renderer, timeout: Duration) -> Result<Document> {
        let empty = serde_json::json!({});
        renderer
            .render(&doc(), &empty, &empty, 1 << 20, timeout)
            .await
    }

    #[tokio::test]
    async fn an_aborted_worker_is_a_client_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = render(&fake(&dir, "kill -ABRT $$"), LONG)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Status(413, _)), "{err}");
    }

    #[tokio::test]
    async fn a_killed_worker_is_a_client_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = render(&fake(&dir, "kill -KILL $$"), LONG)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Status(413, _)), "{err}");
    }

    #[tokio::test]
    async fn a_worker_out_of_cpu_is_a_client_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = render(&fake(&dir, "kill -XCPU $$"), LONG)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Status(422, _)), "{err}");
    }

    #[tokio::test]
    async fn a_worker_that_exits_is_a_client_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = render(&fake(&dir, "exit 101"), LONG).await.unwrap_err();
        assert!(matches!(err, Error::Status(422, _)), "{err}");
    }

    #[tokio::test]
    async fn a_worker_that_hangs_runs_into_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let err = render(&fake(&dir, "exec sleep 60"), Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Timeout(_)), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_worker_that_cannot_start_fails_the_check() {
        let renderer = Renderer::isolated("/nonexistent/rusticdoc".into(), 1 << 30, 1);
        assert!(renderer.check().await.is_err());
    }

    #[tokio::test]
    async fn the_workers_get_no_environment() {
        let dir = tempfile::tempdir().unwrap();
        // Dies by signal only when something but PATH-free defaults is present.
        let renderer = fake(
            &dir,
            "if env | grep -q -v -E '^(PWD|SHLVL|_|OLDPWD)='; then kill -KILL $$; else exit 101; fi",
        );
        let err = render(&renderer, LONG).await.unwrap_err();
        assert!(matches!(err, Error::Status(422, _)), "{err}");
    }

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, FRAME_REPLY, &[b"{}", b"abc", b""]).unwrap();
        let (kind, blobs) = read_frame(&mut buf.as_slice()).unwrap().unwrap();
        assert_eq!(kind, FRAME_REPLY);
        assert_eq!(blobs, vec![b"{}".to_vec(), b"abc".to_vec(), Vec::new()]);
        assert!(read_frame(&mut [].as_slice()).unwrap().is_none());
    }

    #[test]
    fn errors_keep_their_kind_across_the_pipe() {
        for e in [
            Error::InvalidRequest("a".into()),
            Error::Template("b".into()),
            Error::Status(413, "c".into()),
        ] {
            let back = from_wire(to_wire(&e), LONG);
            assert_eq!(back.to_string(), e.to_string());
        }
    }
}
