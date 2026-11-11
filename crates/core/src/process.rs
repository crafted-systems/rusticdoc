//! Running an engine as a child process with a time limit.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::error::{Error, Result};

/// All a child engine gets of this process's environment. API keys and the webhook
/// secret live there, and engines work on documents from clients.
const PASSED_ENV: &[&str] = &[
    "PATH",
    "LANG",
    "LC_ALL",
    "HOME",
    "TMPDIR",
    "FONTCONFIG_FILE",
    "TZ",
    // Where python3-uno lives, on installs that keep it out of the default path.
    "PYTHONPATH",
    "UNO_PATH",
    "URE_BOOTSTRAP",
];

/// Replaces `cmd`'s environment with the allow-listed part of this one.
pub fn clean_env(cmd: &mut Command) {
    cmd.env_clear();
    for name in PASSED_ENV {
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        }
    }
}

/// Runs `bin` with `args`, kills it at `timeout`, and returns its stderr on
/// failure as an [`Error::Engine`].
pub async fn run(
    engine: &'static str,
    bin: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(&str, String)],
    timeout: Duration,
) -> Result<()> {
    run_limited(engine, bin, args, cwd, env, timeout, None).await
}

/// [`run`], with the child's address space capped at `memory` bytes on Linux,
/// for engines that parse a client's file and could be made to allocate
/// without bound. macOS does not enforce the cap.
pub async fn run_limited(
    engine: &'static str,
    bin: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(&str, String)],
    timeout: Duration,
    memory: Option<u64>,
) -> Result<()> {
    let mut cmd = Command::new(bin);
    clean_env(&mut cmd);
    #[cfg(target_os = "linux")]
    if let Some(memory) = memory {
        // SAFETY: the closure only calls setrlimit, which is async-signal-safe,
        // and allocates nothing.
        unsafe {
            cmd.pre_exec(move || {
                let limit = libc::rlimit {
                    rlim_cur: memory as _,
                    rlim_max: memory as _,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &limit) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = memory;
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    // Arguments are not logged: they can carry file names and, for qpdf's
    // argument files, point at passwords.
    tracing::debug!(engine, ?bin, args = args.len(), "running");

    let mut child = cmd.spawn().map_err(|e| Error::Engine {
        engine,
        message: format!("cannot start {}: {e}", bin.display()),
    })?;

    // Only the end of what it prints is used, so only the end is kept: an
    // engine can be made to print a line for every object of a file.
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let finished = async {
        let (status, stdout, stderr) = tokio::join!(child.wait(), tail(out), tail(err));
        status.map(|status| (status, stdout, stderr))
    };
    let (status, stdout, stderr) = match tokio::time::timeout(timeout, finished).await {
        Ok(result) => result?,
        Err(_) => return Err(Error::Timeout(timeout)),
    };

    if status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&stderr);
    let stdout = String::from_utf8_lossy(&stdout);
    let tail: String = stderr
        .lines()
        .chain(stdout.lines())
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ");
    Err(Error::Engine {
        engine,
        message: format!(
            "exit {}{}",
            status.code().unwrap_or(-1),
            if tail.is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            }
        ),
    })
}

/// How much of the end of a child's output is kept.
const KEPT_OUTPUT: usize = 64 * 1024;

/// Reads a child's output to its end and returns the last of it, at most
/// [`KEPT_OUTPUT`] bytes. All of it is read, so the child never blocks on a
/// full pipe.
async fn tail(stream: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    use tokio::io::AsyncReadExt as _;
    let Some(mut stream) = stream else {
        return Vec::new();
    };
    let mut kept = Vec::new();
    // On the heap: held across an await, it would otherwise be part of every
    // future that runs an engine, and those are moved about on the stack.
    let mut chunk = vec![0u8; 8192];
    while let Ok(n) = stream.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        kept.extend_from_slice(&chunk[..n]);
        if kept.len() > 2 * KEPT_OUTPUT {
            kept.drain(..kept.len() - KEPT_OUTPUT);
        }
    }
    if kept.len() > KEPT_OUTPUT {
        kept.drain(..kept.len() - KEPT_OUTPUT);
    }
    kept
}
