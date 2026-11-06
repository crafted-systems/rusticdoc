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
    let mut cmd = Command::new(bin);
    clean_env(&mut cmd);
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

    let child = cmd.spawn().map_err(|e| Error::Engine {
        engine,
        message: format!("cannot start {}: {e}", bin.display()),
    })?;

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(result) => result?,
        Err(_) => return Err(Error::Timeout(timeout)),
    };

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
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
            output.status.code().unwrap_or(-1),
            if tail.is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            }
        ),
    })
}
