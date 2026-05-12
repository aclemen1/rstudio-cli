//! Integration tests against a live RStudio session (Desktop or Server).
//!
//! These tests require a running rsession socket reachable by the current user.
//! They are marked `#[ignore]` so they don't run on plain `cargo test`.
//! Run them explicitly:
//!
//!     cargo test --test live -- --ignored
//!
//! If no live session is reachable, each test prints `SKIP: ...` and exits 0.
//!
//! The tests intentionally avoid persistent side-effects: any variable created
//! in R is removed in the same test. The `r send` tests appear briefly in the
//! user's console but leave the session in the same state they found it.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use rstudio_cli::commands::console::{self as console_cmd, ConsoleCmd};
use rstudio_cli::commands::editor::{self as editor_cmd, EditorCmd};
use rstudio_cli::commands::env::{self as env_cmd, EnvCmd};
use rstudio_cli::commands::project::{self as project_cmd, ProjectCmd};
use rstudio_cli::commands::r::{self as r_cmd, RCmd};
use rstudio_cli::commands::term::{self as term_cmd, TermCmd};
use rstudio_cli::r_eval::{self, EvalTimeout};
use rstudio_cli::rpc::RpcClient;
use rstudio_cli::session::{Session, SessionOverrides};

// All tests that hit the live rsession must be serialised: Desktop rsession
// processes requests one at a time and returns async handles for concurrent
// calls that the CLI does not poll. Running tests in parallel causes most of
// them to fail with SessionUnavailable / kAsyncCompletion errors.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// Returns Some(Session) if a live rsession socket exists, None otherwise.
fn live_session() -> Option<Session> {
    if let Ok(s) = Session::detect(SessionOverrides::default()) {
        return Some(s);
    }
    // Fallback: scan the standard RStudio Server rsession directory.
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

// require_live!() expands to two bindings so that the guard lives for the
// entire test function, not just until the end of the macro expression:
//
//     let (session, _guard) = require_live!();
//
// The guard must stay alive until the test returns; if it were bound inside
// the macro block it would be dropped immediately.
macro_rules! require_live {
    () => {{
        let guard = serial();
        let session = match live_session() {
            Some(s) => s,
            None => {
                eprintln!("SKIP: no live RStudio session available");
                return;
            }
        };
        (session, guard)
    }};
}

// ---------------------------------------------------------------------------
// r exec
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn r_exec_basic() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let out = r_eval::run(&rpc, "1 + 1").expect("exec run 1+1");
    assert!(out.contains("2"), "expected '2' in output, got: {out:?}");
}

#[test]
#[ignore = "requires a live RStudio session"]
fn r_exec_r_error_surfaces_as_r_error() {
    let (session, _guard) = require_live!();
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
#[ignore = "requires a live RStudio session"]
fn r_exec_timeout_surfaces_as_timeout() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let err = r_eval::run_with_timeout(&rpc, "Sys.sleep(3)", EvalTimeout::Limit(1.0))
        .expect_err("should hit the 1 s limit");
    assert!(
        matches!(err.kind, rstudio_cli::error::ErrorKind::Timeout),
        "expected Timeout, got {:?}: {}",
        err.kind,
        err.message
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn r_exec_async_and_poll() {
    use std::time::{Duration, Instant};
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);

    let launched = r_cmd::run(
        &RCmd::Exec {
            code: "Sys.sleep(0.5); paste('async', 'ok')".into(),
            timeout: None,
            r#async: true,
        },
        &rpc,
    )
    .expect("r exec --async")
    .expect("some result");

    let job_id = launched
        .get("id")
        .and_then(|v| v.as_str())
        .expect("job id")
        .to_string();
    assert_eq!(
        launched.get("status").and_then(|v| v.as_str()),
        Some("running")
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let poll = r_cmd::run(&RCmd::Poll { id: job_id.clone() }, &rpc)
            .expect("r poll")
            .expect("some result");
        match poll.get("status").and_then(|v| v.as_str()) {
            Some("done") => {
                let output = poll.get("output").and_then(|v| v.as_str()).unwrap_or("");
                assert!(
                    output.contains("async") && output.contains("ok"),
                    "unexpected async output: {output:?}"
                );
                break;
            }
            Some("running") => {}
            other => panic!("unexpected poll status: {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "r exec --async timed out after 10 s"
        );
    }
}

// ---------------------------------------------------------------------------
// r send
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn r_send_captures_stdout() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = r_cmd::run(
        &RCmd::Send {
            code: "sqrt(144)".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("r send sqrt(144)")
    .expect("some result");
    let stdout = result
        .get("stdout")
        .and_then(|v| v.as_str())
        .expect("stdout");
    assert!(
        stdout.contains("12"),
        "expected '12' in stdout, got: {stdout:?}"
    );
    assert!(
        result.get("error").and_then(|v| v.as_str()).is_none(),
        "unexpected error field"
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn r_send_captures_message() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = r_cmd::run(
        &RCmd::Send {
            code: r#"message("hello from smoke test")"#.into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("r send message()")
    .expect("some result");
    let msgs = result
        .get("messages")
        .and_then(|v| v.as_array())
        .expect("messages array");
    assert!(
        msgs.iter()
            .any(|m| m.as_str().unwrap_or("").contains("hello from smoke test")),
        "expected message in messages, got: {msgs:?}"
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn r_send_surfaces_r_error_as_cli_error() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    // stop() inside r send is captured by tryCatch, written to JSON, then
    // re-raised by the CLI as a CliError::RError so callers can detect it.
    let err = r_cmd::run(
        &RCmd::Send {
            code: r#"stop("smoke error")"#.into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect_err("r send stop() should surface as CliError");
    assert!(
        matches!(err.kind, rstudio_cli::error::ErrorKind::RError),
        "expected RError, got {:?}",
        err.kind
    );
    assert!(
        err.message.contains("smoke error"),
        "expected 'smoke error', got: {:?}",
        err.message
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn r_send_multiline_code() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = r_cmd::run(
        &RCmd::Send {
            code: ".smoke_x <- 6\n.smoke_y <- 7\n.smoke_x * .smoke_y".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("r send multiline")
    .expect("some result");
    // r exec is synchronous — the rm is guaranteed to complete before we return.
    r_eval::run(&rpc, "rm(.smoke_x, .smoke_y)").expect("rm smoke vars");
    let stdout = result
        .get("stdout")
        .and_then(|v| v.as_str())
        .expect("stdout");
    assert!(stdout.contains("42"), "expected '42', got: {stdout:?}");
}

#[test]
#[ignore = "requires a live RStudio session"]
fn r_send_no_capture_returns_void() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = r_cmd::run(
        &RCmd::Send {
            code: "invisible(NULL)".into(),
            no_capture: true,
            timeout: None,
        },
        &rpc,
    )
    .expect("r send --no-capture");
    assert!(
        result.is_none(),
        "expected None for --no-capture, got: {result:?}"
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn r_send_in_attached_env() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);

    // Create the data.frame and attach it in a single r send so both run in
    // the same console expression — r exec evaluates silently and the variable
    // is visible to subsequent r send calls, but attach() needs to run in the
    // same expression to guarantee search-path ordering.
    r_cmd::run(
        &RCmd::Send {
            code: ".smoke_df <- data.frame(.smoke_col = 99L); attach(.smoke_df)".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("create and attach .smoke_df");
    rpc.rpc(
        "set_environment",
        vec![serde_json::Value::String(".smoke_df".into())],
    )
    .expect("set_environment");

    let result = r_cmd::run(
        &RCmd::Send {
            code: ".smoke_col".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("r send .smoke_col in attached env")
    .expect("some result");

    // Teardown: restore globalenv in the pane, then detach and rm silently.
    rpc.rpc(
        "set_environment",
        vec![serde_json::Value::String(".GlobalEnv".into())],
    )
    .ok();
    r_eval::run(&rpc, r#"detach(".smoke_df"); rm(.smoke_df)"#).expect("teardown .smoke_df");

    let stdout = result
        .get("stdout")
        .and_then(|v| v.as_str())
        .expect("stdout");
    assert!(
        stdout.contains("99"),
        "expected '99' from attached env, got: {stdout:?}"
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn r_send_invisible_value_produces_no_stdout() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = r_cmd::run(
        &RCmd::Send {
            code: "x <- 1".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("r send assignment")
    .expect("some result");
    let _ = r_eval::run(&rpc, "rm(x)");
    let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        stdout.is_empty(),
        "assignment should produce no stdout, got: {stdout:?}"
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn r_send_mixed_stdout_and_message() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = r_cmd::run(
        &RCmd::Send {
            code: r#"message("msg"); cat("out\n")"#.into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("r send mixed")
    .expect("some result");
    let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    let msgs = result
        .get("messages")
        .and_then(|v| v.as_array())
        .expect("messages");
    assert!(
        stdout.contains("out"),
        "expected 'out' in stdout, got: {stdout:?}"
    );
    assert!(
        msgs.iter()
            .any(|m| m.as_str().unwrap_or("").contains("msg")),
        "expected 'msg' in messages, got: {msgs:?}"
    );
}

// ---------------------------------------------------------------------------
// env
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn env_list_returns_array() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = env_cmd::run(&EnvCmd::List { pattern: None }, &rpc)
        .expect("env list")
        .expect("some");
    assert!(
        result.get("vars").and_then(|v| v.as_array()).is_some(),
        "expected vars array, got: {result:?}"
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn env_list_pattern_filter() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    // Use r send (with capture) so we know the expression has fully executed
    // before we query the environment — no-capture is fire-and-forget and the
    // variable may not be visible yet when get_environment_state runs.
    // Variable names starting with '.' are hidden by RStudio's environment
    // panel (like ls() with all.names=FALSE). Use a plain name here.
    r_cmd::run(
        &RCmd::Send {
            code: "smoke_filter_var_42L <- 42L".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("create var");
    let result = env_cmd::run(
        &EnvCmd::List {
            pattern: Some("^smoke_filter_var_42L$".into()),
        },
        &rpc,
    )
    .expect("env list --pattern")
    .expect("some");
    r_cmd::run(
        &RCmd::Send {
            code: "rm(smoke_filter_var_42L)".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("rm var");
    let vars = result.get("vars").and_then(|v| v.as_array()).expect("vars");
    assert_eq!(vars.len(), 1, "expected exactly 1 match, got: {vars:?}");
    assert_eq!(
        vars[0].get("name").and_then(|v| v.as_str()),
        Some("smoke_filter_var_42L")
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn env_info_returns_metadata() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    r_cmd::run(
        &RCmd::Send {
            code: "smoke_int_42L <- 42L".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("create var");
    let result = env_cmd::run(
        &EnvCmd::Info {
            name: "smoke_int_42L".into(),
        },
        &rpc,
    )
    .expect("env info")
    .expect("some");
    r_cmd::run(
        &RCmd::Send {
            code: "rm(smoke_int_42L)".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("rm var");
    // env info returns 'typeof' (R's typeof()), not 'type'.
    let typeof_ = result
        .get("typeof")
        .and_then(|v| v.as_str())
        .expect("typeof");
    assert_eq!(
        typeof_, "integer",
        "expected typeof=integer, got: {typeof_:?}"
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn env_contents_returns_lines() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    r_cmd::run(
        &RCmd::Send {
            code: ".smoke_df2 <- data.frame(a = 1:3)".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("create df");
    let result = env_cmd::run(
        &EnvCmd::Contents {
            name: ".smoke_df2".into(),
        },
        &rpc,
    )
    .expect("env contents")
    .expect("some");
    r_cmd::run(
        &RCmd::Send {
            code: "rm(.smoke_df2)".into(),
            no_capture: false,
            timeout: Some(10.0),
        },
        &rpc,
    )
    .expect("rm df");
    let contents = result
        .get("contents")
        .and_then(|v| v.as_array())
        .expect("contents");
    assert!(
        !contents.is_empty(),
        "expected non-empty contents for a data.frame"
    );
}

// ---------------------------------------------------------------------------
// console
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn console_history_returns_array() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = console_cmd::run(&ConsoleCmd::History { limit: 10 }, &rpc, &session)
        .expect("console history")
        .expect("some");
    assert!(
        result.get("commands").and_then(|v| v.as_array()).is_some(),
        "expected commands array, got: {result:?}"
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn console_context_returns_object() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = console_cmd::run(&ConsoleCmd::Context, &rpc, &session)
        .expect("console context")
        .expect("some");
    eprintln!("console context: {result}");
}

// ---------------------------------------------------------------------------
// editor
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn editor_read_returns_content() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let cargo_toml = env::current_dir().expect("cwd").join("Cargo.toml");
    let result = editor_cmd::run(
        &EditorCmd::Read {
            path: cargo_toml,
            encoding: "UTF-8".into(),
        },
        &rpc,
        &session,
    )
    .expect("editor read")
    .expect("some");
    let contents = result
        .get("contents")
        .and_then(|v| v.as_str())
        .expect("contents");
    assert!(contents.contains("[package]"));
    assert!(contents.contains("name = \"rstudio-cli\""));
}

#[test]
#[ignore = "requires a live RStudio session"]
fn editor_list_returns_array() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = editor_cmd::run(&EditorCmd::List, &rpc, &session)
        .expect("editor list")
        .expect("some");
    assert!(
        result.get("documents").and_then(|v| v.as_array()).is_some(),
        "expected documents array, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// term
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn term_list_returns_array() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = term_cmd::run(&TermCmd::List, &rpc)
        .expect("term list")
        .expect("some");
    let terminals = result
        .get("terminals")
        .and_then(|v| v.as_array())
        .expect("terminals");
    eprintln!("term list returned {} terminals", terminals.len());
}

// ---------------------------------------------------------------------------
// project
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn project_current_returns_path_or_null() {
    let (_session, _guard) = require_live!();
    let overrides = SessionOverrides::default();
    let result = project_cmd::run(&ProjectCmd::Current, overrides)
        .expect("project current")
        .expect("some");
    let path = result.get("path").expect("path field");
    assert!(
        path.is_null() || path.is_string(),
        "expected null or string for path, got: {path:?}"
    );
}

// ---------------------------------------------------------------------------
// schema (offline — no session needed, but grouped here for convenience)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn schema_registry_is_populated() {
    let actions = rstudio_cli::schema::registry();
    assert!(
        actions.len() >= 20,
        "expected >=20 actions, got {}",
        actions.len()
    );
    let categories: std::collections::HashSet<_> = actions.iter().map(|a| a.category).collect();
    for required in ["editor", "r", "console", "term", "env", "pane", "skill"] {
        assert!(categories.contains(required), "missing category {required}");
    }
}
