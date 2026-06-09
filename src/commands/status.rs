//! `rstudio status` — single-call snapshot of the CLI ↔ session wiring.
//!
//! Aggregates four kinds of info for the agent / user to land oriented at
//! the start of a session:
//!
//! - **CLI**: version, auto-detected mode (Server/Desktop).
//! - **Transport**: Unix socket path (Server) or TCP loopback address (Desktop).
//! - **Session**: user identity, session id (Server-derived from sources_dir),
//!   active client id, sources directory, active project.
//! - **R-side**: R version, RStudio version.
//! - **Documents**: open count and active doc id/path.
//!
//! One R round-trip pulls everything that needs the rsession; the rest
//! comes from `Session` and the local sources directory listing.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::VERSION;
use crate::client_id;
use crate::commands::editor::is_document_id;
use crate::error::CliError;
use crate::output::Reply;
use crate::r_eval;
use crate::rpc::RpcClient;
use crate::session::{Mode, Session};
use crate::transport::Backend;

pub fn run(rpc: &RpcClient<'_>, session: &Session) -> Result<Reply, CliError> {
    let r_info = collect_r_info(rpc)?;
    let open_count = count_open_docs(session);

    let cli = json!({
        "version": VERSION,
        "mode": match session.mode {
            Mode::Server => "server",
            Mode::Desktop => "desktop",
        },
    });

    let transport = match &session.transport {
        Backend::Unix(path) => json!({
            "type": "unix-socket",
            "path": path.display().to_string(),
        }),
        Backend::Tcp(addr) => json!({
            "type": "tcp-loopback",
            "address": addr.to_string(),
        }),
    };

    let session_id = derive_session_id(session);
    let client_id = client_id::resolve_client_id(session).ok();
    let lock_block = lock_block(session_id.as_deref());

    let session_block = json!({
        "id": session_id,
        "client_id": client_id,
        "sources_dir": session.sources_dir.as_ref().map(|p| p.display().to_string()),
        "state_path": session.state_path.as_ref().map(|p| p.display().to_string()),
        "active_project": r_info.get("active_project").cloned().unwrap_or(Value::Null),
        "lock": lock_block,
    });

    let rsession = json!({
        "r_version": r_info.get("r_version").cloned().unwrap_or(Value::Null),
        "rstudio_version": r_info.get("rstudio_version").cloned().unwrap_or(Value::Null),
        // Debugger awareness at the start of a session — `null` when R is
        // at the top-level prompt, populated when a `browser()` / `debug()` /
        // `recover()` frame is active. Cheap: one RPC, no R eval.
        "debugger": collect_debugger(rpc),
    });

    let documents = json!({
        "open_count": open_count,
        "active_id": r_info.get("active_doc_id").cloned().unwrap_or(Value::Null),
        "active_path": r_info.get("active_doc_path").cloned().unwrap_or(Value::Null),
    });

    let update_available =
        crate::update_check::check(VERSION).map(|u| serde_json::json!({"latest": u.latest}));

    let value = json!({
        "cli": cli,
        "transport": transport,
        "user": session.user,
        "session": session_block,
        "rsession": rsession,
        "documents": documents,
        "update_available": update_available,
    });
    let text = format_as_text(&value);
    // Default to JSON for `status`: agents call this at the start of a
    // session and want the structured payload. Humans get the polished
    // text rendering with `--format text`.
    Ok(Reply::Adaptive {
        value,
        text,
        default_text: false,
    })
}

/// Compact human-readable rendering of the status payload, used in
/// `--format text` mode. JSON mode keeps the full envelope.
fn format_as_text(v: &Value) -> String {
    fn s<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
        v.pointer(ptr).and_then(|x| x.as_str())
    }
    fn or_dash<'a>(v: &'a Value, ptr: &str) -> &'a str {
        s(v, ptr).unwrap_or("—")
    }

    let cli_version = s(v, "/cli/version").unwrap_or("?");
    let mode_label = match s(v, "/cli/mode") {
        Some("server") => "Server",
        Some("desktop") => "Desktop",
        _ => "?",
    };
    let transport_str = match s(v, "/transport/type") {
        Some("unix-socket") => format!("unix://{}", s(v, "/transport/path").unwrap_or("?")),
        Some("tcp-loopback") => format!("tcp://{}", s(v, "/transport/address").unwrap_or("?")),
        Some(other) => other.to_string(),
        None => "?".to_string(),
    };
    let user = or_dash(v, "/user");
    let session_id = or_dash(v, "/session/id");
    let client_id = or_dash(v, "/session/client_id");
    let project = s(v, "/session/active_project").unwrap_or("(none)");
    let r_version_full = s(v, "/rsession/r_version").unwrap_or("");
    let r_version = r_version_full
        .split_whitespace()
        .nth(2)
        .unwrap_or(r_version_full);
    let rstudio_version = or_dash(v, "/rsession/rstudio_version");
    let open_count = v
        .pointer("/documents/open_count")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let active_basename = s(v, "/documents/active_path")
        .and_then(|p| Path::new(p).file_name().and_then(|n| n.to_str()))
        .unwrap_or("none");

    // Debugger line: only shown when active, to avoid noise in the
    // common idle case. Format mirrors the JSON projection. We don't print
    // a Browse[N] number because N is not retrievable (see collect_debugger);
    // when a user function is identified we name it, otherwise we say the
    // browser is at the top level.
    let debugger_line = match v.pointer("/rsession/debugger") {
        Some(Value::Object(_)) => {
            let where_ = match v
                .pointer("/rsession/debugger/function")
                .and_then(Value::as_str)
            {
                Some(fn_) => format!("inside {fn_}()"),
                None => "at top level".to_string(),
            };
            format!(
                "debugger        active (Browse> {where_}) — call `debug status` for the full picture\n"
            )
        }
        _ => String::new(),
    };

    format!(
        "rstudio-cli {cli_version} — {mode_label} ({transport_str})\n\
         user            {user}\n\
         session         {session_id}\n\
         client_id       {client_id}\n\
         project         {project}\n\
         R / RStudio     {r_version} / {rstudio_version}\n\
         documents open  {open_count} (active: {active_basename})\n\
         {debugger_line}"
    )
}

/// Minimal projection of `get_environment_state` for ambient debugger
/// awareness in the `status` envelope. Returns `null` at the top-level
/// prompt; otherwise `{in_browser: true, browse_level, function}`. For the
/// full frame / locals / call-stack picture, agents should call `debug
/// status`.
///
/// Detection uses BOTH `context_depth` AND `call_frames.length`:
/// rsession increments `context_depth` only when the IDE-side debugger
/// hook fires (i.e. the user's function was entered via debug() / a
/// breakpoint / explicit step). A top-level `browser()` — or a
/// `browser()` invoked through a side channel such as `console_input`,
/// which is exactly what `r send 'browser()'` does — leaves
/// `context_depth` at 0 but populates `call_frames` with the active
/// stack. Treating only `context_depth` as a signal misses that case
/// and reports `null` while the interpreter is in fact suspended at a
/// Browse prompt. We accept either signal.
///
/// `browse_level` (the N of `Browse[N]>`) is always `null`: R does not
/// expose it (see `debug::status` for the full rationale). It is kept as
/// an explicit field so its shape is stable if a future release populates
/// it via a native helper.
fn collect_debugger(rpc: &RpcClient<'_>) -> Value {
    let Ok(state) = rpc.rpc("get_environment_state", vec![]) else {
        return Value::Null;
    };
    let depth = state
        .get("context_depth")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let frames_len = state
        .get("call_frames")
        .and_then(Value::as_array)
        .map(|a| a.len() as i64)
        .unwrap_or(0);
    if depth <= 0 && frames_len <= 0 {
        return Value::Null;
    }
    // `environment_name` is "fn()" for a function-debug session (strip the
    // parens → "fn"); for a top-level browser it's ".GlobalEnv", in which
    // case there is no user function being debugged and we report null
    // rather than leaking an evaluator-wrapper name.
    let env_name = state
        .get("environment_name")
        .and_then(|n| n.as_str())
        .unwrap_or("");
    let function: Option<String> = if env_name != ".GlobalEnv" && !env_name.is_empty() {
        Some(env_name.trim_end_matches("()").to_string())
    } else {
        None
    };
    json!({
        "in_browser": true,
        "browse_level": Value::Null,
        "function": function,
    })
}

/// Single R round-trip that collects everything we need from the rsession.
fn collect_r_info(rpc: &RpcClient<'_>) -> Result<serde_json::Map<String, Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/status.R`.
    let r_code = r#"cat(jsonlite::toJSON(
        rstudiocli::status_snapshot(),
        auto_unbox = TRUE, null = "null"
    ))"#;
    let raw = r_eval::run(rpc, r_code)?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!(
            "status: invalid JSON from rsession: {e}; raw: {raw}"
        ))
    })?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err(CliError::internal(format!(
            "status: rsession returned non-object: {parsed}"
        ))),
    }
}

/// Extract the session id from `~/.local/share/rstudio/.../session-<id>/`.
/// Server only — Desktop's id (the launcher-token) lives in the same place
/// pattern, so this works for both modes when the path is set.
fn derive_session_id(session: &Session) -> Option<String> {
    let dir = session
        .session_dir
        .as_ref()
        .or(session.sources_dir.as_ref())?;
    let name = dir.file_name()?.to_str()?;
    name.strip_prefix("session-").map(str::to_string)
}

/// Snapshot of the per-session writer lock. Information-only: a holder
/// shown here may have released by the time the agent acts on it. The
/// real protection is the per-call mutex (Phase 1) and `rstudio tx`
/// for multi-call atomicity. Use this field to debug timeouts, audit
/// who's currently active, or signal awareness — never to gate logic.
fn lock_block(session_id: Option<&str>) -> Value {
    let inside = crate::lock::SessionLock::inside_tx();
    let Some(id) = session_id else {
        return json!({ "state": "unknown", "holder": null, "inside_tx": inside });
    };
    let state = crate::lock::inspect(id);
    let (state_label, holder) = match state.holder {
        Some(h) => (
            "held",
            json!({
                "pid": h.pid,
                "command": h.command,
                "started_ms": h.started_ms,
            }),
        ),
        None => ("free", Value::Null),
    };
    json!({
        "state": state_label,
        "holder": holder,
        "inside_tx": inside,
    })
}

/// Count documents currently open in the Source pane by enumerating the
/// sources directory (cheap, no RPC). Returns 0 if the dir is unreachable.
/// Uses `resolve_sources_dir` so the count stays consistent when a project
/// is open and the dir has relocated to `<project>/.Rproj.user/<hash>/sources/`.
fn count_open_docs(session: &Session) -> usize {
    let Ok(dir) = session.resolve_sources_dir() else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            is_document_id(&name)
        })
        .count()
}
