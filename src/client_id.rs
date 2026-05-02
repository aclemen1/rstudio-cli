use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::CliError;

const SESSIONS_SUBDIR: &str = ".local/share/rstudio/sessions/active";
const STATE_FILENAME: &str = "session-persistent-state";

pub fn detect_session_id() -> Option<String> {
    if let Ok(id) = env::var("RSTUDIO_SESSION_ID") {
        if !id.is_empty() {
            return Some(id);
        }
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
    Some(dir.join(format!("session-{session_id}")).join(STATE_FILENAME))
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
