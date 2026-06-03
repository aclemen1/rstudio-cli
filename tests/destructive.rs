//! Destructive integration tests against a live RStudio **Server**
//! session in a disposable container.
//!
//! Unlike `tests/live.rs`, these tests assume the container has been
//! deliberately set up with a missing CRAN dependency BEFORE they run.
//! The orchestration (uninstall + container respawn) lives in
//! `scripts/bridge-up.sh test-destructive`, which:
//!   1. brings the container down + up to a clean state,
//!   2. uninstalls a specific package via `R -e ...`,
//!   3. invokes ONE test from this file with the target package
//!      hinted via `RSTUDIO_CLI_DESTRUCTIVE_MISSING_PKG=<name>`,
//!   4. tears the container down so nothing leaks between tests.
//!
//! Why this split: respawning rsession from inside a Rust test process
//! races against rserver's client_init handshake with Chromium and is
//! brittle. A full container down/up is slower (~30 s) but rock-solid
//! and matches the disposable-container philosophy.
//!
//! Tests are gated by `RSTUDIO_CLI_DESTRUCTIVE_TESTS=1` (set by the
//! shell driver). Without it, each test prints `SKIP: ...` and exits 0.

use std::path::PathBuf;
use std::{env, fs};

use rstudio_cli::r_package;
use rstudio_cli::rpc::RpcClient;
use rstudio_cli::session::{Mode, Session, SessionOverrides};

/// Discover a live session, mirroring `tests/live.rs::live_session()`:
/// try the env-driven default first, then fall back to scanning the
/// rserver socket directory directly. The container's test harness does
/// not export `RSTUDIO_CLI_*` to the test binary, so the fallback path
/// is the one that actually succeeds in CI.
fn detect_session() -> Option<Session> {
    if let Ok(s) = Session::detect(SessionOverrides::default()) {
        return Some(s);
    }
    let dir = PathBuf::from("/var/run/rstudio-server/rstudio-rsession");
    let entries = fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().ends_with(".pid") {
            continue;
        }
        let socket = entry.path();
        if !socket.exists() {
            continue;
        }
        let user = env::var("USER").or_else(|_| env::var("LOGNAME")).ok()?;
        let overrides = SessionOverrides {
            socket: Some(socket),
            user: Some(user),
            ..Default::default()
        };
        if let Ok(session) = Session::detect(overrides) {
            return Some(session);
        }
    }
    None
}

/// Common guard: opt-in env var + live Server session. Returns the
/// session, or `None` (with a SKIP message) when prerequisites aren't met.
fn require_destructive() -> Option<Session> {
    if env::var("RSTUDIO_CLI_DESTRUCTIVE_TESTS").as_deref() != Ok("1") {
        eprintln!("SKIP: set RSTUDIO_CLI_DESTRUCTIVE_TESTS=1 to run destructive tests");
        return None;
    }
    let session = match detect_session() {
        Some(s) => s,
        None => {
            eprintln!("SKIP: no live RStudio session available");
            return None;
        }
    };
    if session.mode != Mode::Server {
        eprintln!("SKIP: destructive tests require RStudio Server (got Desktop)");
        return None;
    }
    Some(session)
}

/// Shared body for every destructive pre-check test. The orchestrator
/// (`bridge-up.sh test-destructive`) pre-uninstalled exactly one
/// package; this function verifies the end-to-end behaviour an agent
/// would observe: calling `ensure_installed` (which is what every
/// CLI/MCP invocation goes through) returns the actionable error with
/// the leading no-side-effect guard.
///
/// We intentionally don't call `check_dependencies` directly to inspect
/// the raw probe output: that would short-circuit inside `RpcClient::rpc`
/// (which itself invokes `ensure_installed` before any RPC), and the
/// result would be the pre-check error, not the probe vector. Testing
/// `ensure_installed` directly is the right level — it's the public
/// chokepoint every consumer hits.
fn assert_missing_dep_is_surfaced(expected_missing: &str) {
    let session = match require_destructive() {
        Some(s) => s,
        None => return,
    };
    let rpc = RpcClient::new(&session);

    let err = r_package::ensure_installed(&rpc)
        .expect_err("ensure_installed must fail when a hard dep is missing");
    let msg = format!("{err}");
    assert!(
        msg.starts_with("Nothing was opened, run, or modified in RStudio."),
        "guard sentence must lead the error message: {msg}"
    );
    assert!(
        msg.contains(&format!("- {expected_missing}")),
        "missing-package list must mention '{expected_missing}': {msg}"
    );
    let expected_cmd = format!("install.packages(\"{expected_missing}\")");
    assert!(
        msg.contains(&expected_cmd),
        "actionable install command must be present: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "destructive: requires bridge-up.sh test-destructive jsonlite"]
fn destructive_precheck_reports_missing_jsonlite() {
    assert_missing_dep_is_surfaced("jsonlite");
}

#[test]
#[ignore = "destructive: requires bridge-up.sh test-destructive rstudioapi"]
fn destructive_precheck_reports_missing_rstudioapi() {
    assert_missing_dep_is_surfaced("rstudioapi");
}

#[test]
#[ignore = "destructive: requires bridge-up.sh test-destructive callr"]
fn destructive_precheck_reports_missing_callr() {
    // `callr` joined `R_HARD_DEPS` in 0.18.0 once it was acknowledged as
    // an officially mandatory dependency (already in DESCRIPTION's
    // Imports:, but the precheck previously excluded it, leaving users
    // with an opaque "had non-zero exit status" tarball install error
    // when callr was missing). This test guards the new contract.
    assert_missing_dep_is_surfaced("callr");
}
