//! PID 1 in a container, for `serve`.
//!
//! Something has to reap the orphans LibreOffice leaves and forward signals, and an
//! init like tini does that with the service's whole environment readable at
//! `/proc/1/environ` by the same user LibreOffice runs as. Doing it here lets PID 1
//! close its own `/proc` entries first.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

/// Runs this same command line as a child, forwards signals to it, reaps whatever
/// reparents here, and exits with the child's status.
pub fn run() -> anyhow::Result<ExitCode> {
    // SAFETY: these calls only set a process flag and this thread's signal mask,
    // on structures local to this function.
    let signals = unsafe {
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in [
            libc::SIGCHLD,
            libc::SIGTERM,
            libc::SIGINT,
            libc::SIGHUP,
            libc::SIGQUIT,
        ] {
            libc::sigaddset(&mut set, signal);
        }
        if libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        set
    };
    let mut command = Command::new(std::env::current_exe()?);
    command.args(std::env::args_os().skip(1));
    // SAFETY: the closure only calls functions that are async-signal-safe. A child
    // inherits the signal mask, and the service must not start with these blocked.
    unsafe {
        command.pre_exec(|| {
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            if libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    let service = child.id() as libc::pid_t;
    loop {
        let mut signal = 0;
        // SAFETY: sigwait fills in the one int it is handed.
        if unsafe { libc::sigwait(&signals, &mut signal) } != 0 {
            continue;
        }
        if signal != libc::SIGCHLD {
            // SAFETY: kill takes two integers and touches no memory.
            unsafe { libc::kill(service, signal) };
            continue;
        }
        loop {
            let mut status = 0;
            // SAFETY: waitpid fills in the one int it is handed.
            let reaped = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if reaped <= 0 {
                break;
            }
            if reaped == service {
                let code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else {
                    128 + libc::WTERMSIG(status)
                };
                return Ok(ExitCode::from(code as u8));
            }
        }
    }
}
