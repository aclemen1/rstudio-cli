//! Integration tests against a live RStudio Server session.
//!
//! These tests require:
//! - a running RStudio Server `rsession` socket reachable by the current user
//! - a browser tab attached (so `active-client-id` is on disk)
//!
//! They are marked `#[ignore]` so they don't run on `cargo test`. Run them
//! explicitly:
//!
//!     cargo test --test live -- --ignored
//!
//! If no live session is reachable, each test prints `SKIP: ...` and exits 0.
//!
//! The tests intentionally avoid actions that disrupt the user (no
//! `exec send`, no `term send/exec`, no editor mutation). They only
//! exercise read-only paths that the CLI can do silently.

use std::env;
use std::fs;
use std::path::PathBuf;

use rstudio_cli::commands::editor::EditorCmd;
use rstudio_cli::commands::env::EnvCmd;
use rstudio_cli::commands::term::TermCmd;
use rstudio_cli::commands::{editor, env as env_cmd, term};
use rstudio_cli::r_eval::{self, EvalTimeout};
use rstudio_cli::rpc::RpcClient;
use rstudio_cli::session::{Session, SessionOverrides};

/// Returns Some(Session) if a live rsession socket exists for the current user,
/// None otherwise. Used by every #[ignore] test to skip silently when the
/// runner isn't inside an RStudio Server session.
fn live_session() -> Option<Session> {
    if let Ok(s) = Session::detect(SessionOverrides::default()) {
        return Some(s);
    }
    // Fallback: scan the standard rsession directory for any non-.pid socket.
    let dir = PathBuf::from("/var/run/rstudio-server/rstudio-rsession");
    let entries = fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".pid") {
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

macro_rules! require_live {
    () => {
        match live_session() {
            Some(s) => s,
            None => {
                eprintln!("SKIP: no live RStudio session available");
                return;
            }
        }
    };
}

#[test]
#[ignore = "requires a live RStudio Server session"]
fn exec_run_basic() {
    let session = require_live!();
    let rpc = RpcClient::new(&session);
    let out = r_eval::run(&rpc, "1 + 1").expect("exec run 1+1");
    assert!(out.contains("2"), "expected output to contain '2', got: {out:?}");
}

#[test]
#[ignore = "requires a live RStudio Server session"]
fn exec_run_r_error_surfaces_as_r_error() {
    let session = require_live!();
    let rpc = RpcClient::new(&session);
    let err = r_eval::run(&rpc, "stop('boom')").expect_err("should be an R error");
    assert!(
        matches!(err.kind, rstudio_cli::error::ErrorKind::RError),
        "expected RError, got {:?}",
        err.kind
    );
    assert!(err.message.contains("boom"));
}

#[test]
#[ignore = "requires a live RStudio Server session"]
fn exec_run_timeout_surfaces_as_timeout() {
    let session = require_live!();
    let rpc = RpcClient::new(&session);
    let err = r_eval::run_with_timeout(&rpc, "Sys.sleep(3)", EvalTimeout::Limit(1.0))
        .expect_err("should hit the 1s limit");
    assert!(
        matches!(err.kind, rstudio_cli::error::ErrorKind::Timeout),
        "expected Timeout, got {:?}: {}",
        err.kind,
        err.message
    );
}

#[test]
#[ignore = "requires a live RStudio Server session"]
fn editor_read_returns_content() {
    let session = require_live!();
    let rpc = RpcClient::new(&session);
    let cwd = env::current_dir().expect("cwd");
    let cargo_toml = cwd.join("Cargo.toml");
    let cmd = EditorCmd::Read {
        path: cargo_toml.clone(),
        encoding: "UTF-8".into(),
    };
    let result = editor::run(&cmd, &rpc, &session)
        .expect("editor read")
        .expect("some");
    let contents = result
        .get("contents")
        .and_then(|v| v.as_str())
        .expect("contents string");
    assert!(contents.contains("[package]"));
    assert!(contents.contains("name = \"rstudio-cli\""));
}

#[test]
#[ignore = "requires a live RStudio Server session"]
fn env_list_returns_array() {
    let session = require_live!();
    let rpc = RpcClient::new(&session);
    let cmd = EnvCmd::List { pattern: None };
    let result = env_cmd::run(&cmd, &rpc).expect("env list").expect("some");
    let vars = result.get("vars").and_then(|v| v.as_array()).expect("vars array");
    // The session may or may not have user variables, but the call must
    // succeed and the field must be an array (possibly empty).
    eprintln!("env list returned {} variables", vars.len());
}

#[test]
#[ignore = "requires a live RStudio Server session"]
fn term_list_returns_array() {
    let session = require_live!();
    let rpc = RpcClient::new(&session);
    let cmd = TermCmd::List;
    let result = term::run(&cmd, &rpc).expect("term list").expect("some");
    let terminals = result
        .get("terminals")
        .and_then(|v| v.as_array())
        .expect("terminals array");
    eprintln!("term list returned {} terminals", terminals.len());
}

#[test]
#[ignore = "requires a live RStudio Server session"]
fn schema_introspection_works_offline() {
    // schema is offline (no socket needed) but lives in the lib so we
    // assert here that the registry is populated.
    let actions = rstudio_cli::schema::registry();
    assert!(actions.len() >= 20, "expected >=20 actions, got {}", actions.len());
    let categories: std::collections::HashSet<_> =
        actions.iter().map(|a| a.category).collect();
    for required in ["editor", "r", "console", "term", "env", "pane", "skill"] {
        assert!(categories.contains(required), "missing category {required}");
    }
}
