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

use serde_json::{Value, json};

use crate::VERSION;
use crate::client_id;
use crate::commands::editor::is_document_id;
use crate::error::CliError;
use crate::r_eval;
use crate::rpc::RpcClient;
use crate::session::{Mode, Session};
use crate::transport::Backend;

pub fn run(rpc: &RpcClient<'_>, session: &Session) -> Result<Option<Value>, CliError> {
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

    let session_block = json!({
        "id": session_id,
        "client_id": client_id,
        "sources_dir": session.sources_dir.as_ref().map(|p| p.display().to_string()),
        "state_path": session.state_path.as_ref().map(|p| p.display().to_string()),
        "active_project": r_info.get("active_project").cloned().unwrap_or(Value::Null),
    });

    let rsession = json!({
        "r_version": r_info.get("r_version").cloned().unwrap_or(Value::Null),
        "rstudio_version": r_info.get("rstudio_version").cloned().unwrap_or(Value::Null),
    });

    let documents = json!({
        "open_count": open_count,
        "active_id": r_info.get("active_doc_id").cloned().unwrap_or(Value::Null),
        "active_path": r_info.get("active_doc_path").cloned().unwrap_or(Value::Null),
    });

    Ok(Some(json!({
        "cli": cli,
        "transport": transport,
        "user": session.user,
        "session": session_block,
        "rsession": rsession,
        "documents": documents,
    })))
}

/// Single R round-trip that collects everything we need from the rsession.
fn collect_r_info(rpc: &RpcClient<'_>) -> Result<serde_json::Map<String, Value>, CliError> {
    let r_code = r#"local({
  out <- list(
    r_version = R.version$version.string,
    rstudio_version = tryCatch(
      as.character(rstudioapi::versionInfo()$version),
      error = function(e) NULL
    ),
    active_project = tryCatch(
      rstudioapi::getActiveProject(),
      error = function(e) NULL
    ),
    active_doc_id = tryCatch(
      rstudioapi::documentId(allowConsole = FALSE),
      error = function(e) NULL
    ),
    active_doc_path = tryCatch(
      rstudioapi::documentPath(),
      error = function(e) NULL
    )
  )
  cat(jsonlite::toJSON(out, auto_unbox = TRUE, null = "null"))
})"#;
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

/// Count documents currently open in the Source pane by enumerating the
/// sources directory (cheap, no RPC). Returns 0 if the dir is unreachable.
fn count_open_docs(session: &Session) -> usize {
    let Some(dir) = session.sources_dir.as_ref() else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(dir) else {
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
