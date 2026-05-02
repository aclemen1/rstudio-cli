use std::env;
use std::path::{Path, PathBuf};

use crate::client_id;
use crate::error::CliError;

const DEFAULT_SESSION_TMP_DIR: &str = "/var/run/rstudio-server/rstudio-rsession";

#[derive(Debug, Clone)]
pub struct Session {
    pub socket_path: PathBuf,
    pub user: String,
    /// Path to the session-persistent-state file holding `active-client-id`.
    /// `None` until first needed by an RPC call (postbacks don't require it).
    pub state_path: Option<PathBuf>,
    /// Path to the session directory under
    /// `~/.local/share/rstudio/sessions/active/session-<id>`. Same `None`
    /// caveat as `state_path` — only resolved when we know the session id.
    pub session_dir: Option<PathBuf>,
    /// Path to the open-documents directory under
    /// `~/.local/share/rstudio/sources/session-<id>`. Each open document is
    /// a JSON file (`<docId>` + `<docId>-contents` for the live buffer).
    pub sources_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub struct SessionOverrides {
    pub socket: Option<PathBuf>,
    pub user: Option<String>,
    pub session_id: Option<String>,
    pub state_path: Option<PathBuf>,
}

impl Session {
    pub fn detect(overrides: SessionOverrides) -> Result<Self, CliError> {
        let user = match overrides.user {
            Some(u) => u,
            None => env::var("USER")
                .or_else(|_| env::var("LOGNAME"))
                .map_err(|_| {
                    CliError::session("cannot determine user (set $USER or pass --user)")
                })?,
        };

        let socket_path = match overrides.socket {
            Some(p) => p,
            None => {
                let stream = env::var("RSTUDIO_SESSION_STREAM").map_err(|_| {
                    CliError::session(
                        "RSTUDIO_SESSION_STREAM is not set — not running inside an RStudio \
                         session? Pass --socket <path> to override.",
                    )
                })?;
                let dir = env::var("RS_SESSION_TMP_DIR")
                    .unwrap_or_else(|_| DEFAULT_SESSION_TMP_DIR.to_string());
                PathBuf::from(dir).join(stream)
            }
        };

        if !socket_path.exists() {
            return Err(CliError::session(format!(
                "RStudio session socket not found at {}",
                socket_path.display()
            )));
        }

        let session_id = overrides
            .session_id
            .clone()
            .or_else(client_id::detect_session_id);

        let state_path = match overrides.state_path {
            Some(explicit) => Some(explicit),
            None => session_id.as_deref().and_then(client_id::state_path_for),
        };

        let session_dir = session_id.as_deref().and_then(client_id::session_dir_for);

        let sources_dir = session_id.as_deref().and_then(client_id::sources_dir_for);

        Ok(Self {
            socket_path,
            user,
            state_path,
            session_dir,
            sources_dir,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket_path
    }

    pub fn require_state_path(&self) -> Result<&Path, CliError> {
        self.state_path.as_deref().ok_or_else(|| {
            CliError::session(
                "Cannot locate the RStudio session state file. \
                 Set $RSTUDIO_SESSION_ID, pass --session-id, or --state-path. \
                 Open RStudio in your browser first.",
            )
        })
    }

    pub fn require_session_dir(&self) -> Result<&Path, CliError> {
        self.session_dir.as_deref().ok_or_else(|| {
            CliError::session(
                "Cannot locate the RStudio session directory. \
                 Set $RSTUDIO_SESSION_ID or pass --session-id.",
            )
        })
    }

    pub fn require_sources_dir(&self) -> Result<&Path, CliError> {
        self.sources_dir.as_deref().ok_or_else(|| {
            CliError::session(
                "Cannot locate the RStudio sources directory \
                 (~/.local/share/rstudio/sources/session-<ID>). \
                 Set $RSTUDIO_SESSION_ID or pass --session-id.",
            )
        })
    }
}
