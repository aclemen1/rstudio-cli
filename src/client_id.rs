use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::CliError;
use crate::session::{Mode, Session};

const SESSIONS_SUBDIR: &str = ".local/share/rstudio/sessions/active";
const STATE_FILENAME: &str = "session-persistent-state";

/// The client id RStudio Desktop uses for every JSON-RPC call. Confirmed
/// from `src/cpp/session/SessionPersistentState.cpp` upstream — Desktop
/// hardcodes this value rather than persisting one to disk.
pub const DESKTOP_CLIENT_ID: &str = "33e600bb-c1b1-46bf-b562-ab5cba070b0e";

pub fn detect_session_id() -> Option<String> {
    if let Ok(id) = env::var("RSTUDIO_SESSION_ID")
        && !id.is_empty()
    {
        return Some(id);
    }
    let dir = sessions_dir()?;
    let mut best: Option<(String, SystemTime)> = None;
    for entry in fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let Some(id) = name_str.strip_prefix("session-") else {
            continue;
        };
        let state = entry.path().join(STATE_FILENAME);
        let Ok(meta) = fs::metadata(&state) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, t)| modified > *t) {
            best = Some((id.to_string(), modified));
        }
    }
    best.map(|(id, _)| id)
}

pub fn state_path_for(session_id: &str) -> Option<PathBuf> {
    let dir = sessions_dir()?;
    Some(
        dir.join(format!("session-{session_id}"))
            .join(STATE_FILENAME),
    )
}

pub fn session_dir_for(session_id: &str) -> Option<PathBuf> {
    let dir = sessions_dir()?;
    Some(dir.join(format!("session-{session_id}")))
}

pub fn sources_dir_for(session_id: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".local/share/rstudio/sources")
            .join(format!("session-{session_id}")),
    )
}

/// Path to RStudio's global `last-project-path` file. Holds the path of
/// the .Rproj file currently associated with the active session
/// (relocates from the global sources dir to the project's
/// `.Rproj.user/<hash>/sources/session-<id>` whenever a project is
/// open). Filesystem-only — never invokes R.
pub fn last_project_path_file() -> Option<PathBuf> {
    let home = env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".local/share/rstudio/projects_settings/last-project-path"))
}

/// Read the active project directory.
///
/// Two sources are tried, in this order:
/// `session-persistent-state.active-project-file` (Server only) and the
/// global `last-project-path`. Both files hold a path to a `.Rproj`
/// file; its parent is the project root. Returns `None` when no project
/// is active or the hint files cannot be read.
pub fn read_active_project_dir(state_path: Option<&Path>) -> Option<PathBuf> {
    // Server-only: session-persistent-state may carry `active-project-file`.
    if let Some(p) = state_path
        && let Ok(content) = fs::read_to_string(p)
    {
        for line in content.lines() {
            if let Some(rest) = line.trim().strip_prefix("active-project-file=") {
                let value = rest.trim().trim_matches('"');
                if !value.is_empty() && value != "none" {
                    return project_dir_from_rproj(Path::new(value));
                }
            }
        }
    }
    // Both modes: the global last-project-path file.
    let last = last_project_path_file()?;
    let raw = fs::read_to_string(&last).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "none" {
        return None;
    }
    project_dir_from_rproj(Path::new(trimmed))
}

fn project_dir_from_rproj(p: &Path) -> Option<PathBuf> {
    if p.extension().is_some_and(|e| e == "Rproj") {
        p.parent().map(Path::to_path_buf)
    } else if p.is_dir() {
        Some(p.to_path_buf())
    } else {
        None
    }
}

/// Resolve the client id to use for JSON-RPC calls in this session. On
/// Desktop, the value is the hardcoded `DESKTOP_CLIENT_ID` constant. On
/// Server, we read it from `session-persistent-state` so the CLI shares
/// identity with the user's open browser tab.
pub fn resolve_client_id(session: &Session) -> Result<String, CliError> {
    // Test/integration override: when the CLI runs against a bridged
    // session (e.g. a Dockerised RStudio Server) it can't share identity
    // with a real browser tab — the `session-persistent-state` may be
    // empty or stale because no GWT client owns the session. The harness
    // performs its own `client_init` and exports the assigned clientId
    // via this env var so the CLI uses it for subsequent RPCs.
    if let Ok(id) = env::var("RSTUDIO_CLI_CLIENT_ID")
        && !id.is_empty()
    {
        return Ok(id);
    }
    match session.mode {
        Mode::Desktop => Ok(DESKTOP_CLIENT_ID.to_string()),
        Mode::Server => {
            let path = session.require_state_path()?;
            read_active_client_id(path)
        }
    }
}

pub fn read_active_client_id(state_path: &Path) -> Result<String, CliError> {
    let content = fs::read_to_string(state_path).map_err(|e| {
        CliError::session(format!(
            "No active browser client found at {} ({e}). \
             Open RStudio in your browser first, then retry.",
            state_path.display()
        ))
    })?;
    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("active-client-id=") else {
            continue;
        };
        let value = rest.trim().trim_matches('"');
        if !value.is_empty() {
            return Ok(value.to_string());
        }
    }
    Err(CliError::session(format!(
        "active-client-id missing in {}. \
         Open RStudio in your browser first, then retry.",
        state_path.display()
    )))
}

fn sessions_dir() -> Option<PathBuf> {
    let home = env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(SESSIONS_SUBDIR))
}
