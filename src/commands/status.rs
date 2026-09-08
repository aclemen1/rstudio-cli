//! `rstudio status` — single-call snapshot of the CLI ↔ session wiring.
//!
//! Aggregates four kinds of info for the agent / user to land oriented at
//! the start of a session:
//!
//! - **CLI**: version, auto-detected mode (Server/Desktop).
//! - **Transport**: Unix socket path (Server) or TCP loopback address (Desktop).
//! - **Session**: user identity, session id (Server-derived from sources_dir),
//!   active client id, sources directory, active project.
//! - **R-side**: R version, RStudio version, ambient debugger state.
//! - **Documents**: open count only.
//!
//! Every rsession call `status` makes is CLIENT-INDEPENDENT — `execute_r_code`
//! (versions, project) and `get_environment_state` (debugger), both serviced by
//! rsession whether or not a browser tab is connected — plus a local sources-dir
//! listing for the open-document count. It deliberately does NOT read the active
//! document: `rstudioapi::documentId()` round-trips through the RStudio client
//! (`get_editor_context` → `waitForMethod`) and blocks the R console until a tab
//! answers, a call that cannot be cancelled once dispatched. So `status` — often
//! the first call of a session, before any tab is open — never hangs and never
//! wedges the console. Active-document reads live in `editor active-id` / `editor
//! context`, invoked when a client is present.

use std::fs;
use std::time::Duration;

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

/// Default bound on status's own (client-independent) R round-trips.
/// Generous: those calls are normally sub-second, so this only trips when
/// rsession is genuinely busy or stuck. Overridable with `--timeout`.
pub const DEFAULT_R_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(rpc: &RpcClient<'_>, session: &Session, r_timeout: Duration) -> Result<Reply, CliError> {
    // Bound status's own R round-trips. They are all client-INDEPENDENT
    // (see `collect_r_info` / `collect_debugger`), so a closed RStudio tab
    // never makes status hang; this only guards against a genuinely busy or
    // stuck rsession.
    let prev = rpc.set_timeout(Some(r_timeout));
    let r_info = collect_r_info(rpc)?;
    let debugger = collect_debugger(rpc);
    rpc.set_timeout(prev);
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
        "debugger": debugger,
    });

    // `status` reports only the open-document COUNT, read from the sources
    // directory with no RPC. The ACTIVE document id/path is deliberately
    // NOT probed here: `rstudioapi::documentId()` round-trips through the
    // RStudio client (`get_editor_context` → `waitForMethod`) and BLOCKS
    // the R console until a connected tab answers — a call that cannot be
    // cancelled once dispatched, so a timed-out probe would leave the
    // console wedged. Agents that need the active document call
    // `editor active-id` / `editor context` deliberately, when a client is
    // present. This keeps `status` — the first call of a session, often
    // made before any tab is open — non-blocking and side-effect-free.
    let documents = json!({
        "open_count": open_count,
        "active_note": "run `editor active-id` (needs a connected RStudio client)",
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
         documents open  {open_count} (active: `editor active-id`, needs a client)\n\
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
/// `browse_level` (the N of `Browse[N]>`) is recovered via the companion
/// package's optional native helper (shared with `debug status`); it is
/// `null` with `browse_level_source: "unavailable"` when that helper can't
/// be built (no C toolchain) — see `debug::native_browse_level`.
fn collect_debugger(rpc: &RpcClient<'_>) -> Value {
    let Ok(state) = rpc.environment_state() else {
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
    // `function` resolution (incl. skipping `do.call`/`browser`/`.rs.*`
    // instrumentation frames so it's non-null even under an overridden
    // browser()) is shared with `debug status` via debug::debugged_function.
    let function = crate::commands::debug::debugged_function(&state);
    let (browse_level, browse_level_source) = crate::commands::debug::native_browse_level(rpc);
    json!({
        "in_browser": true,
        "browse_level": browse_level,
        "browse_level_source": browse_level_source,
        "function": function,
        "captured_at_unix_ms": crate::commands::debug::now_unix_ms(),
    })
}

/// Single R round-trip that collects everything the rsession can answer
/// without a connected client: R / RStudio version, active project.
fn collect_r_info(rpc: &RpcClient<'_>) -> Result<serde_json::Map<String, Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/status.R`.
    let r_code = r#"cat(jsonlite::toJSON(
        rstudiocli::status_snapshot(),
        auto_unbox = TRUE, null = "null"
    ))"#;
    let raw = r_eval::run(rpc, r_code)?;
    parse_object(&raw)
}

fn parse_object(raw: &str) -> Result<serde_json::Map<String, Value>, CliError> {
    let parsed: Value = serde_json::from_str(raw).map_err(|e| {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The `documents` block must carry only the client-independent
    /// open-count and a pointer to `editor active-id`; it must never carry
    /// an active-document field that would imply a UI round-trip.
    #[test]
    fn documents_block_has_no_ui_probe_fields() {
        let v = json!({
            "cli": {"version": "0.0.0", "mode": "server"},
            "transport": {"type": "unix-socket", "path": "/s"},
            "session": {"active_project": null},
            "rsession": {"r_version": "R version 4.5.0", "debugger": null},
            "documents": {"open_count": 2, "active_note": "run `editor active-id` (needs a connected RStudio client)"},
        });
        let text = format_as_text(&v);
        assert!(text.contains("documents open  2"), "{text}");
        assert!(text.contains("editor active-id"), "{text}");
        // No claim about client connectivity or an active document path.
        assert!(!text.contains("client "), "{text}");
        assert!(!text.contains("active: none"), "{text}");
    }

    #[test]
    fn text_rendering_shows_debugger_when_active() {
        let v = json!({
            "cli": {"version": "0.0.0", "mode": "server"},
            "transport": {"type": "unix-socket", "path": "/s"},
            "session": {},
            "rsession": {"debugger": {"function": "f"}},
            "documents": {"open_count": 0},
        });
        let text = format_as_text(&v);
        assert!(
            text.contains("debugger        active (Browse> inside f())"),
            "{text}"
        );
    }
}
