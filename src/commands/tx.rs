//! `rstudio tx -- <cmd>` — hold the session lock across a child
//! process for atomic multi-call sequences.
//!
//! Modelled on `flock(1)` from util-linux: acquire the per-session
//! exclusive lock, set `RSTUDIO_TX_HELD=1` in the child environment,
//! exec the child, propagate its exit code. Every `rstudio`
//! invocation inside the child detects the env var and skips its
//! own per-call lock acquisition (the parent already holds it).
//! The lock dies with the parent process — kernel cleanup — so
//! crashes, SIGKILLs and orphans never leave a stale lock.
//!
//! With no command, defaults to `$SHELL` (or `bash`), giving a free
//! interactive REPL inside an atomic transaction.

use std::os::unix::process::ExitStatusExt;
use std::process::Command as StdCommand;
use std::time::Duration;

use clap::Args;

use crate::error::CliError;
use crate::lock::{SessionLock, TX_ENV};
use crate::session::Session;

#[derive(Args, Debug)]
pub struct TxCmd {
    /// Command and args to run with the session lock held. Pass after
    /// `--`. With no args, defaults to `$SHELL` (or `bash` if unset),
    /// giving an interactive REPL inside the transaction.
    ///
    /// Examples:
    ///   rstudio tx -- bash -c 'editor read X | jq ... | editor write X ...'
    ///   rstudio tx -- bash         # interactive shell
    ///   rstudio tx                 # same as above (no args defaults to $SHELL)
    ///   rstudio tx -- python3 my_agent.py
    #[arg(last = true)]
    pub argv: Vec<String>,
}

/// Acquire the session lock, exec the child, return the child's exit
/// code. The caller is expected to `std::process::exit(code)` to
/// propagate — we don't do that here so this stays unit-testable.
///
/// When `acquire_lock` is false (top-level `--no-lock`), the env var
/// is still set so that nested rstudio commands inside the child
/// behave consistently (skip their own per-call mutex). Tx becomes a
/// pure env-wrapper without enforcement. This is the natural meaning
/// of `--no-lock tx`: all CLI-level locking off.
pub fn run(
    cmd: &TxCmd,
    session: &Session,
    lock_timeout: Duration,
    acquire_lock: bool,
) -> Result<i32, CliError> {
    let session_id = session.session_id().ok_or_else(|| {
        CliError::session(
            "tx: cannot derive session id (no session_dir / sources_dir). \
             Open RStudio in your browser first, or pass --session-id.",
        )
    })?;

    let argv: Vec<String> = if cmd.argv.is_empty() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".into());
        vec![shell]
    } else {
        cmd.argv.clone()
    };

    // Sidecar attribution: record what we are wrapping. Useful for
    // competing agents reading the lock holder file on timeout.
    let label = format!("tx -- {}", argv.join(" "));
    let _lock = if acquire_lock {
        Some(SessionLock::acquire(&session_id, lock_timeout, &label)?)
    } else {
        None
    };

    let mut child = StdCommand::new(&argv[0]);
    if argv.len() > 1 {
        child.args(&argv[1..]);
    }
    child.env(TX_ENV, "1");

    let status = child
        .status()
        .map_err(|e| CliError::user(format!("tx: failed to spawn {:?}: {e}", argv[0])))?;

    // Rust ExitStatus → Unix exit code. If terminated by signal, follow
    // the standard `128 + signum` convention used by shells.
    let code = status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(1);
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_defaults_to_shell_when_empty() {
        let cmd = TxCmd { argv: vec![] };
        assert!(cmd.argv.is_empty());
        // The actual default lookup happens inside run(); we test it via
        // integration tests since it depends on the env var.
    }
}
