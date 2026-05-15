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
use rstudio_cli::commands::job::{self as job_cmd, JobCmd};
use rstudio_cli::commands::pane::{self as pane_cmd, PaneCmd};
use rstudio_cli::commands::pref::{self as pref_cmd, PrefCmd};
use rstudio_cli::commands::project::{self as project_cmd, ProjectCmd};
use rstudio_cli::commands::r::{self as r_cmd, RCmd};
use rstudio_cli::commands::session::{self as session_cmd, SessionCmd};
use rstudio_cli::commands::status as status_cmd;
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
    let fixture = env::current_dir()
        .expect("cwd")
        .join("editor-read-fixture.toml");
    fs::write(&fixture, "[package]\nname = \"rstudio-cli\"\n").expect("write fixture");
    let result = editor_cmd::run(
        &EditorCmd::Read {
            path: fixture.clone(),
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
    let _ = fs::remove_file(&fixture);
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
// status
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn status_returns_full_payload() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let reply = status_cmd::run(&rpc, &session).expect("status");
    let value = match reply {
        rstudio_cli::output::Reply::Adaptive { value, .. } => value,
        rstudio_cli::output::Reply::Wrapped(v) => v.expect("some"),
    };

    // CLI block: version + mode.
    let cli = value.get("cli").expect("cli block");
    assert!(cli.get("version").and_then(|v| v.as_str()).is_some());
    let mode = cli.get("mode").and_then(|v| v.as_str()).expect("mode");
    assert!(mode == "server" || mode == "desktop", "mode={mode}");

    // rsession block: r_version + rstudio_version (live calls).
    let rsession = value.get("rsession").expect("rsession block");
    assert!(
        rsession
            .get("r_version")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.starts_with("R version"))
    );
    assert!(
        rsession
            .get("rstudio_version")
            .and_then(|v| v.as_str())
            .is_some()
    );

    // Transport block: shape depends on session mode.
    let transport = value.get("transport").expect("transport block");
    let kind = transport
        .get("type")
        .and_then(|v| v.as_str())
        .expect("transport type");
    assert!(
        kind == "unix-socket" || kind == "tcp-loopback",
        "type={kind}"
    );

    // Session block: id is mandatory, lock state is always present.
    let sess = value.get("session").expect("session block");
    assert!(sess.get("id").and_then(|v| v.as_str()).is_some());
    let lock_state = sess
        .get("lock")
        .and_then(|v| v.get("state"))
        .and_then(|v| v.as_str())
        .expect("lock.state");
    assert!(
        ["free", "held", "stale"].contains(&lock_state),
        "lock.state={lock_state}"
    );
}

// ---------------------------------------------------------------------------
// session
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn session_info_fields_present() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = session_cmd::run(&SessionCmd::Info, &rpc)
        .expect("session info")
        .expect("some");
    // session_info is delegated to rstudiocli::session_info() which wraps
    // rstudioapi::versionInfo(). The exact field set is determined by
    // rstudioapi; we assert on what's stable across releases.
    assert!(result.is_object(), "result should be an object: {result:?}");
    let version = result
        .get("version")
        .and_then(|v| v.as_str())
        .expect("version field");
    assert!(!version.is_empty(), "version should be non-empty");
}

#[test]
#[ignore = "requires a live RStudio session"]
fn session_restart_requires_confirm() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    // Calling session restart without --confirm must reject as a user error
    // before touching rsession (the wrapper guards against accidental
    // destruction of in-memory R state). We verify both the rejection AND
    // that rsession is untouched by sampling its PID before and after.
    let pid_before = r_eval::run(&rpc, "Sys.getpid()").expect("getpid before");
    let err = session_cmd::run(
        &SessionCmd::Restart {
            command: None,
            confirm: false,
        },
        &rpc,
    )
    .expect_err("restart without --confirm should fail");
    assert!(
        matches!(err.kind, rstudio_cli::error::ErrorKind::UserError),
        "expected UserError, got {:?}: {}",
        err.kind,
        err.message
    );
    let pid_after = r_eval::run(&rpc, "Sys.getpid()").expect("getpid after");
    assert_eq!(
        pid_before, pid_after,
        "rsession PID changed despite the rejection: before={pid_before:?}, after={pid_after:?}"
    );
}

// ---------------------------------------------------------------------------
// pref
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn pref_read_write_user_roundtrip() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let name = ".smoke_pref_user".to_string();
    // Write then read back; finally clear the pref (writing NULL removes it).
    pref_cmd::run(
        &PrefCmd::Write {
            name: name.clone(),
            value_json: "\"hello-smoke\"".into(),
        },
        &rpc,
    )
    .expect("pref write");
    let result = pref_cmd::run(
        &PrefCmd::Read {
            name: name.clone(),
            default_json: "null".into(),
        },
        &rpc,
    )
    .expect("pref read")
    .expect("some");
    let value = result.get("value").and_then(|v| v.as_str()).expect("value");
    assert_eq!(value, "hello-smoke");
    // Teardown: clear the pref by writing JSON null.
    let _ = pref_cmd::run(
        &PrefCmd::Write {
            name,
            value_json: "null".into(),
        },
        &rpc,
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn pref_read_default_when_missing() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = pref_cmd::run(
        &PrefCmd::Read {
            name: ".smoke_pref_definitely_missing".into(),
            default_json: "42".into(),
        },
        &rpc,
    )
    .expect("pref read missing")
    .expect("some");
    // rstudioapi::readPreference returns the default when the pref is unset.
    let value = result.get("value").and_then(|v| v.as_i64()).expect("i64");
    assert_eq!(value, 42);
}

#[test]
#[ignore = "requires a live RStudio session"]
fn pref_get_set_persistent_roundtrip() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let name = ".smoke_persistent_value".to_string();
    pref_cmd::run(
        &PrefCmd::SetPersistent {
            name: name.clone(),
            value_json: "\"persistent-smoke\"".into(),
        },
        &rpc,
    )
    .expect("pref set-persistent");
    let result = pref_cmd::run(&PrefCmd::GetPersistent { name }, &rpc)
        .expect("pref get-persistent")
        .expect("some");
    let value = result.get("value").and_then(|v| v.as_str()).expect("value");
    assert_eq!(value, "persistent-smoke");
    // Persistent values cannot be cleanly cleared (no API for it); we leave
    // the key behind — it's namespaced under .smoke_ so it won't collide.
}

// ---------------------------------------------------------------------------
// pane
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn pane_viewer_displays_url() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    // pane_viewer is side-effect-only on the rsession side but the CLI
    // echoes back the resolved target as confirmation. The call succeeds
    // iff rstudioapi::viewer() did not raise.
    let result = pane_cmd::run(
        &PaneCmd::Viewer {
            target: "https://example.com/".into(),
        },
        &rpc,
    )
    .expect("pane viewer")
    .expect("some");
    assert_eq!(
        result.get("target").and_then(|v| v.as_str()),
        Some("https://example.com/"),
    );
}

#[test]
#[ignore = "requires a live RStudio session"]
fn pane_files_navigate() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = pane_cmd::run(
        &PaneCmd::Files {
            path: PathBuf::from("/tmp"),
        },
        &rpc,
    )
    .expect("pane files")
    .expect("some");
    // The CLI canonicalises the path before sending — on macOS /tmp
    // resolves to /private/tmp, so just assert the trailing 'tmp'.
    let path = result
        .get("path")
        .and_then(|v| v.as_str())
        .expect("path field");
    assert!(path.ends_with("/tmp"), "unexpected path: {path:?}");
}

#[test]
#[ignore = "requires a live RStudio session"]
fn pane_markers_inline_json() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    // pane_markers reads the file referenced by each marker (to extract
    // the offending line for the Markers pane). Use an existing file so
    // the call succeeds end-to-end.
    let fixture = env::temp_dir().join("pane-markers-fixture.R");
    fs::write(&fixture, "smoke <- TRUE\n").expect("write fixture");
    let markers = format!(
        r#"[{{"file":"{}","line":1,"column":1,"type":"info","message":"smoke marker"}}]"#,
        fixture.display()
    );
    let _result = pane_cmd::run(
        &PaneCmd::Markers {
            name: "rstudio-cli-smoke".into(),
            markers: Some(markers),
            auto_select: "none".into(),
        },
        &rpc,
    )
    .expect("pane markers");
    let _ = fs::remove_file(&fixture);
}

// ---------------------------------------------------------------------------
// job
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn job_add_lifecycle() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);

    // Add a job — disable auto_remove so the job survives across the
    // SetProgress/SetState/Remove calls below.
    let added = job_cmd::run(
        &JobCmd::Add {
            name: "smoke-job".into(),
            status: "starting".into(),
            progress_units: 100,
            running: true,
            auto_remove: false,
            show: true,
        },
        &rpc,
    )
    .expect("job add")
    .expect("some");
    let job_id = added
        .get("id")
        .and_then(|v| v.as_str())
        .expect("job id")
        .to_string();

    // Drive the job through a few state transitions.
    job_cmd::run(
        &JobCmd::SetProgress {
            id: job_id.clone(),
            units: 50,
        },
        &rpc,
    )
    .expect("job set-progress");
    job_cmd::run(
        &JobCmd::SetState {
            id: job_id.clone(),
            state: "succeeded".into(),
        },
        &rpc,
    )
    .expect("job set-state succeeded");

    // The job should now appear in the Jobs pane list. `jobs` is an object
    // keyed by id (not an array), mirroring rstudioapi::jobList()'s shape.
    let listed = job_cmd::run(&JobCmd::List, &rpc)
        .expect("job list")
        .expect("some");
    let jobs = listed
        .get("jobs")
        .and_then(|v| v.as_object())
        .expect("jobs object");
    assert!(
        jobs.contains_key(&job_id),
        "job {job_id} not present in `job list`: {jobs:?}"
    );

    // Teardown: explicit remove (auto_remove was disabled).
    job_cmd::run(&JobCmd::Remove { id: job_id }, &rpc).expect("job remove");
}

#[test]
#[ignore = "requires a live RStudio session"]
fn job_is_active_returns_bool() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = job_cmd::run(&JobCmd::IsActive, &rpc)
        .expect("job is-active")
        .expect("some");
    // The R wrapper returns `is_job`: true iff the current R execution is
    // a callr-spawned background job. Live tests run in the foreground.
    let active = result
        .get("is_job")
        .and_then(|v| v.as_bool())
        .expect("is_job");
    assert!(!active, "live tests should not run as a background job");
}

// ---------------------------------------------------------------------------
// term
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn term_full_lifecycle() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);

    // Create a terminal. Use a per-run unique name so leftover terminals
    // from a previously panicked run don't collide on caption.
    let unique_name = format!("smoke-term-{}", std::process::id());
    let created = term_cmd::run(
        &TermCmd::Create {
            name: Some(unique_name),
            shell_type: None,
            show: false,
        },
        &rpc,
    )
    .expect("term create")
    .expect("some");
    let id = created
        .get("id")
        .and_then(|v| v.as_str())
        .expect("term id")
        .to_string();

    // Exec a deterministic command. The terminal echoes the input + output.
    term_cmd::run(
        &TermCmd::Exec {
            id: id.clone(),
            text: "echo smoke-out".into(),
        },
        &rpc,
    )
    .expect("term exec");

    // Poll the buffer for up to 5 s, waiting for the command's output.
    // The wrapper returns `lines` as a JSON array of strings (one per
    // terminal line, including the prompt and the echoed command).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut joined = String::new();
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let buf = term_cmd::run(
            &TermCmd::Buffer {
                id: id.clone(),
                limit: None,
                ansi: false,
            },
            &rpc,
        )
        .expect("term buffer")
        .expect("some");
        joined = buf
            .get("lines")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if joined.contains("smoke-out") {
            break;
        }
    }
    assert!(
        joined.contains("smoke-out"),
        "expected 'smoke-out' in buffer, got: {joined:?}"
    );

    // Teardown.
    term_cmd::run(&TermCmd::Kill { id }, &rpc).expect("term kill");
}

#[test]
#[ignore = "requires a live RStudio session"]
fn term_context_returns_metadata() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let created = term_cmd::run(
        &TermCmd::Create {
            name: Some(format!("smoke-ctx-{}", std::process::id())),
            shell_type: None,
            show: false,
        },
        &rpc,
    )
    .expect("term create")
    .expect("some");
    let id = created
        .get("id")
        .and_then(|v| v.as_str())
        .expect("term id")
        .to_string();

    let ctx = term_cmd::run(&TermCmd::Context { id: id.clone() }, &rpc)
        .expect("term context")
        .expect("some");
    assert!(
        ctx.get("caption").is_some(),
        "caption field missing: {ctx:?}"
    );

    term_cmd::run(&TermCmd::Kill { id }, &rpc).expect("term kill");
}

#[test]
#[ignore = "requires a live RStudio session"]
fn term_running_and_busy_initially_false() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let created = term_cmd::run(
        &TermCmd::Create {
            name: Some(format!("smoke-busy-{}", std::process::id())),
            shell_type: None,
            show: false,
        },
        &rpc,
    )
    .expect("term create")
    .expect("some");
    let id = created
        .get("id")
        .and_then(|v| v.as_str())
        .expect("term id")
        .to_string();

    // A freshly created terminal has a running shell but no foreground task.
    let running = term_cmd::run(&TermCmd::Running { id: id.clone() }, &rpc)
        .expect("term running")
        .expect("some");
    assert_eq!(
        running.get("running").and_then(|v| v.as_bool()),
        Some(true),
        "freshly created terminal should have a running shell"
    );

    let busy = term_cmd::run(&TermCmd::Busy { id: id.clone() }, &rpc)
        .expect("term busy")
        .expect("some");
    assert_eq!(
        busy.get("busy").and_then(|v| v.as_bool()),
        Some(false),
        "freshly created terminal should not be busy"
    );

    term_cmd::run(&TermCmd::Kill { id }, &rpc).expect("term kill");
}

#[test]
#[ignore = "requires a live RStudio session"]
fn term_exit_code_null_while_running() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let created = term_cmd::run(
        &TermCmd::Create {
            name: Some(format!("smoke-exit-{}", std::process::id())),
            shell_type: None,
            show: false,
        },
        &rpc,
    )
    .expect("term create")
    .expect("some");
    let id = created
        .get("id")
        .and_then(|v| v.as_str())
        .expect("term id")
        .to_string();

    let ec = term_cmd::run(&TermCmd::ExitCode { id: id.clone() }, &rpc)
        .expect("term exit-code")
        .expect("some");
    // While the shell is alive the exit code is null.
    assert!(
        ec.get("exit_code").is_some_and(|v| v.is_null()),
        "expected null exit_code for running term, got: {ec:?}"
    );

    term_cmd::run(&TermCmd::Kill { id }, &rpc).expect("term kill");
}

#[test]
#[ignore = "requires a live RStudio session"]
fn term_visible_returns_id_or_null() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);
    let result = term_cmd::run(&TermCmd::Visible, &rpc)
        .expect("term visible")
        .expect("some");
    let id = result.get("id").expect("id field");
    // Either a string (a terminal is visible) or null (none focused).
    assert!(
        id.is_string() || id.is_null(),
        "expected string or null, got: {id:?}"
    );
}

// ---------------------------------------------------------------------------
// editor (write/read cycle)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a live RStudio session"]
fn editor_open_set_read_close_cycle() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);

    // Open a fresh fixture in the editor.
    let fixture = env::temp_dir().join("editor-cycle-fixture.R");
    fs::write(&fixture, "# initial\n").expect("write fixture");
    let opened = editor_cmd::run(
        &EditorCmd::Open {
            path: fixture.clone(),
            line: None,
            col: None,
            no_cursor: false,
        },
        &rpc,
        &session,
    )
    .expect("editor open")
    .expect("some");
    let doc_id = opened
        .get("id")
        .and_then(|v| v.as_str())
        .expect("doc id")
        .to_string();

    // Replace the buffer's contents.
    editor_cmd::run(
        &EditorCmd::SetContents {
            text: "# replaced via test\n1 + 1\n".into(),
            id: Some(doc_id.clone()),
            path: None,
        },
        &rpc,
        &session,
    )
    .expect("editor set-contents");

    // setDocumentContents propagates asynchronously through rsession's
    // event channel; rsession returns the pre-modification buffer in
    // get_source_document for ~1 s after the mutation. A fixed wait is
    // friendlier to rsession than a poll loop, which can saturate its
    // event queue with repeated execute_r_code calls and leave it in a
    // wedged state for subsequent tests.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let buf = editor_cmd::run(
        &EditorCmd::ReadBuffer {
            id: Some(doc_id.clone()),
            path: None,
        },
        &rpc,
        &session,
    )
    .expect("editor read-buffer")
    .expect("some");
    let contents = buf
        .get("contents")
        .and_then(|v| v.as_str())
        .expect("contents");
    assert!(
        contents.contains("replaced via test"),
        "buffer not updated, got: {contents:?}"
    );

    // Don't close the document: editor_close blocks rsession for ~30 s on
    // this image (rocker/rstudio:4.5.2), which wedges every subsequent
    // execute_r_code call in the test process. The fixture is under /tmp
    // so simply unlinking the file is enough — the buffer survives in the
    // editor but doesn't interfere with other tests.
    let _ = doc_id;
    let _ = fs::remove_file(&fixture);
}

#[test]
#[ignore = "requires a live RStudio session"]
fn editor_modify_range_replaces_substring() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);

    let fixture = env::temp_dir().join("editor-modify-fixture.R");
    fs::write(&fixture, "one\ntwo\nthree\n").expect("write fixture");
    let opened = editor_cmd::run(
        &EditorCmd::Open {
            path: fixture.clone(),
            line: None,
            col: None,
            no_cursor: false,
        },
        &rpc,
        &session,
    )
    .expect("editor open")
    .expect("some");
    let doc_id = opened
        .get("id")
        .and_then(|v| v.as_str())
        .expect("doc id")
        .to_string();

    // Replace "two" on line 2 with "TWO".
    editor_cmd::run(
        &EditorCmd::ModifyRange {
            range: "2:1-2:4".into(),
            text: "TWO".into(),
            id: Some(doc_id.clone()),
            path: None,
        },
        &rpc,
        &session,
    )
    .expect("editor modify-range");

    // Same async propagation caveat as in editor_open_set_read_close_cycle.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let buf = editor_cmd::run(
        &EditorCmd::ReadBuffer {
            id: Some(doc_id.clone()),
            path: None,
        },
        &rpc,
        &session,
    )
    .expect("editor read-buffer")
    .expect("some");
    let contents = buf
        .get("contents")
        .and_then(|v| v.as_str())
        .expect("contents");
    assert!(
        contents.contains("TWO"),
        "expected 'TWO', got: {contents:?}"
    );
    assert!(
        !contents.contains("two\n"),
        "old 'two' still present, got: {contents:?}"
    );

    // See editor_open_set_read_close_cycle: avoid editor_close to keep
    // rsession responsive for the rest of the suite.
    let _ = doc_id;
    let _ = fs::remove_file(&fixture);
}

#[test]
#[ignore = "requires a live RStudio session"]
fn editor_set_cursor_moves_active_position() {
    let (session, _guard) = require_live!();
    let rpc = RpcClient::new(&session);

    let fixture = env::temp_dir().join("editor-cursor-fixture.R");
    fs::write(&fixture, "abc\ndef\nghi\n").expect("write fixture");
    let opened = editor_cmd::run(
        &EditorCmd::Open {
            path: fixture.clone(),
            line: None,
            col: None,
            no_cursor: false,
        },
        &rpc,
        &session,
    )
    .expect("editor open")
    .expect("some");
    let doc_id = opened
        .get("id")
        .and_then(|v| v.as_str())
        .expect("doc id")
        .to_string();

    // Move cursor to L2:C2.
    editor_cmd::run(
        &EditorCmd::SetCursor {
            position: "2:2".into(),
            id: Some(doc_id.clone()),
            path: None,
        },
        &rpc,
        &session,
    )
    .expect("editor set-cursor");

    std::thread::sleep(std::time::Duration::from_millis(200));

    // Read context: the selection should now be at L2:C2. The wrapper
    // returns `selections` keyed by index (a JSON object, not array),
    // each entry having start_row/start_col/end_row/end_col scalars.
    let ctx = editor_cmd::run(
        &EditorCmd::Context {
            id: Some(doc_id.clone()),
            include_console: false,
            include_contents: false,
        },
        &rpc,
        &session,
    )
    .expect("editor context")
    .expect("some");
    let first = ctx
        .get("selections")
        .and_then(|v| v.as_object())
        .and_then(|m| m.values().next())
        .expect("at least one selection");
    assert_eq!(
        first.get("start_row").and_then(|v| v.as_i64()),
        Some(2),
        "cursor row mismatch: {first:?}"
    );
    assert_eq!(
        first.get("start_col").and_then(|v| v.as_i64()),
        Some(2),
        "cursor column mismatch: {first:?}"
    );

    // See editor_open_set_read_close_cycle: avoid editor_close to keep
    // rsession responsive for the rest of the suite.
    let _ = doc_id;
    let _ = fs::remove_file(&fixture);
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
