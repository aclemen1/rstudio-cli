use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use crate::client_id;
use crate::desktop_discovery;
use crate::error::CliError;
use crate::transport::Backend;

const DEFAULT_SESSION_TMP_DIR: &str = "/var/run/rstudio-server/rstudio-rsession";

/// Which RStudio variant we're talking to. Drives transport selection
/// (Unix socket vs TCP loopback) and authentication style
/// (SO_PEERCRED + identity header vs shared secret header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Server,
    Desktop,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub mode: Mode,
    pub transport: Backend,
    pub user: String,
    /// Server only: path to the session-persistent-state file holding
    /// `active-client-id`. None on Desktop, where the client id is hardcoded
    /// and no such file exists.
    pub state_path: Option<PathBuf>,
    /// Path to the per-session source directory under
    /// `~/.local/share/rstudio/sessions/active/session-<id>` (Server) or
    /// `~/.local/share/rstudio/sources/session-<launcher-token>` (Desktop).
    pub session_dir: Option<PathBuf>,
    pub sources_dir: Option<PathBuf>,
    /// Desktop only: shared secret extracted from the rsession process
    /// environment (`RS_SHARED_SECRET`). Sent as the `X-Shared-Secret`
    /// request header on every RPC.
    pub shared_secret: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct SessionOverrides {
    pub mode: Option<Mode>,
    pub socket: Option<PathBuf>,
    pub user: Option<String>,
    pub session_id: Option<String>,
    pub state_path: Option<PathBuf>,
    pub port: Option<u16>,
    pub secret: Option<String>,
}

impl Session {
    pub fn detect(overrides: SessionOverrides) -> Result<Self, CliError> {
        let mode = match overrides.mode {
            Some(m) => m,
            None => auto_detect_mode(&overrides)?,
        };
        match mode {
            Mode::Server => detect_server(overrides),
            Mode::Desktop => detect_desktop(overrides),
        }
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

fn auto_detect_mode(overrides: &SessionOverrides) -> Result<Mode, CliError> {
    // Strongest hint: an explicit Server socket override wins.
    if overrides.socket.is_some() {
        return Ok(Mode::Server);
    }
    // Strongest Desktop hints: explicit port or secret override.
    if overrides.port.is_some() || overrides.secret.is_some() {
        return Ok(Mode::Desktop);
    }
    // Server hint: the env var the rserver/rsession itself sets.
    if env::var("RSTUDIO_SESSION_STREAM").is_ok() {
        return Ok(Mode::Server);
    }
    // Fallback: look for the canonical Server socket directory on disk. If
    // any socket file is present there, we're on Server.
    let server_dir = PathBuf::from(DEFAULT_SESSION_TMP_DIR);
    if server_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&server_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().ends_with(".pid") {
                return Ok(Mode::Server);
            }
        }
    }
    // Otherwise, look for a Desktop rsession process; if one is found, use
    // it. If discovery fails, surface the error explaining how to override.
    desktop_discovery::discover().map(|_| Mode::Desktop)
}

fn detect_server(overrides: SessionOverrides) -> Result<Session, CliError> {
    let user = resolve_user(overrides.user)?;

    let socket_path = match overrides.socket {
        Some(p) => p,
        None => {
            let stream = env::var("RSTUDIO_SESSION_STREAM").map_err(|_| {
                CliError::session(
                    "RSTUDIO_SESSION_STREAM is not set — not running inside an RStudio \
                     Server session? Pass --socket <path>, or run with --mode desktop.",
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

    Ok(Session {
        mode: Mode::Server,
        transport: Backend::Unix(socket_path),
        user,
        state_path,
        session_dir,
        sources_dir,
        shared_secret: None,
    })
}

fn detect_desktop(overrides: SessionOverrides) -> Result<Session, CliError> {
    let user = resolve_user(overrides.user)?;

    // If both port and secret are provided explicitly, we can skip process
    // discovery entirely. session_id (= launcher-token) is then required if
    // the caller wants `editor list` etc. that need the sources dir.
    let (port, secret, launcher_token) = match (
        overrides.port,
        overrides.secret.clone(),
        overrides.session_id.clone(),
    ) {
        (Some(port), Some(secret), token) => (port, secret, token),
        (Some(_), None, _) | (None, Some(_), _) => {
            return Err(CliError::session(
                "Desktop mode: --port and --secret must be passed together when \
                 overriding discovery, or omit both to auto-discover from the \
                 running rsession process.",
            ));
        }
        (None, None, override_token) => {
            let info = desktop_discovery::discover()?;
            (
                info.port,
                info.shared_secret,
                override_token.or(Some(info.launcher_token)),
            )
        }
    };

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);

    // Desktop uses the launcher-token as session id for on-disk paths.
    let sources_dir = launcher_token
        .as_deref()
        .and_then(client_id::sources_dir_for);
    // Desktop has no session-persistent-state file; state_path stays None and
    // client_id::read_active_client_id short-circuits in Desktop mode.
    Ok(Session {
        mode: Mode::Desktop,
        transport: Backend::Tcp(addr),
        user,
        state_path: None,
        session_dir: None,
        sources_dir,
        shared_secret: Some(secret),
    })
}

fn resolve_user(user_override: Option<String>) -> Result<String, CliError> {
    match user_override {
        Some(u) => Ok(u),
        None => env::var("USER")
            .or_else(|_| env::var("LOGNAME"))
            .map_err(|_| CliError::session("cannot determine user (set $USER or pass --user)")),
    }
}
