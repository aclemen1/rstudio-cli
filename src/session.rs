use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
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

    /// Best-effort extraction of the session id from `session_dir` /
    /// `sources_dir` (both end with `session-<id>`). Used by the
    /// session-scoped file lock.
    pub fn session_id(&self) -> Option<String> {
        let dir = self.session_dir.as_ref().or(self.sources_dir.as_ref())?;
        dir.file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("session-"))
            .map(str::to_string)
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

    /// Resolve the actual sources directory, taking project-relocation
    /// into account.
    ///
    /// RStudio stores per-session source-document metadata at
    /// `~/.local/share/rstudio/sources/session-<id>` while no project
    /// is active. As soon as a project is opened, that whole directory
    /// MOVES to `<project>/.Rproj.user/<hash>/sources/session-<id>` —
    /// the session id is unchanged, only the parent path moves.
    /// `self.sources_dir` is computed once at session detection from
    /// the global path, so it is stale whenever a project is active.
    ///
    /// This method tries the global path first (cheap stat), then
    /// falls back to reading the active project path from disk
    /// (`last-project-path`, plus `active-project-file` from the
    /// session-persistent-state file on Server) and globs the
    /// `.Rproj.user/<hash>/sources/session-<id>` candidate.
    ///
    /// Filesystem-only — never invokes the R interpreter.
    pub fn resolve_sources_dir(&self) -> Result<PathBuf, CliError> {
        let configured = self.require_sources_dir()?.to_path_buf();
        if configured.is_dir() {
            return Ok(configured);
        }
        let session_id = configured
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("session-"))
            .ok_or_else(|| {
                CliError::session(format!(
                    "Cannot derive session id from sources_dir path {}",
                    configured.display()
                ))
            })?
            .to_string();

        if let Some(project) = client_id::read_active_project_dir(self.state_path.as_deref())
            && let Some(path) = find_project_sources_dir(&project, &session_id)
        {
            return Ok(path);
        }

        Err(CliError::session(format!(
            "Cannot locate sources directory for session-{session_id}. \
             Tried {} (no active project) and the active project's \
             .Rproj.user/<hash>/sources/. Open RStudio in your browser \
             first, or pass --session-id.",
            configured.display()
        )))
    }
}

/// Glob `<project>/.Rproj.user/*/sources/session-<id>` and return the
/// first match. The hash subdirectory is one-per-profile and a single
/// OS user typically has exactly one.
fn find_project_sources_dir(project: &Path, session_id: &str) -> Option<PathBuf> {
    let rproj_user = project.join(".Rproj.user");
    let entries = std::fs::read_dir(&rproj_user).ok()?;
    for entry in entries.flatten() {
        let candidate = entry
            .path()
            .join("sources")
            .join(format!("session-{session_id}"));
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
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
        None => match env::var("RSTUDIO_SESSION_STREAM") {
            Ok(stream) => {
                let dir = env::var("RS_SESSION_TMP_DIR")
                    .unwrap_or_else(|_| DEFAULT_SESSION_TMP_DIR.to_string());
                PathBuf::from(dir).join(stream)
            }
            // Env var unset (e.g. running on the same machine as the rsession but
            // not from inside its terminal): scan the socket directory for one
            // owned by the current user. Single match wins, otherwise we surface
            // an actionable error.
            Err(_) => auto_discover_server_socket()?,
        },
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

/// Scan `$RS_SESSION_TMP_DIR` for rsession Unix sockets owned by the current
/// uid. Returns all matches sorted by path. Returns an empty Vec when the
/// directory doesn't exist or isn't readable (caller decides how to handle).
pub fn list_server_sockets() -> Vec<PathBuf> {
    let dir =
        env::var("RS_SESSION_TMP_DIR").unwrap_or_else(|_| DEFAULT_SESSION_TMP_DIR.to_string());
    let dir_path = PathBuf::from(&dir);

    if !dir_path.is_dir() {
        return Vec::new();
    }

    // SAFETY: getuid() is always-safe; it has no failure mode.
    let our_uid = unsafe { libc::getuid() };

    let Ok(entries) = std::fs::read_dir(&dir_path) else {
        return Vec::new();
    };

    let mut sockets: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let Ok(ft) = e.file_type() else { return false };
            ft.is_socket() && e.metadata().map(|m| m.uid() == our_uid).unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();
    sockets.sort();
    sockets
}

fn auto_discover_server_socket() -> Result<PathBuf, CliError> {
    let dir =
        env::var("RS_SESSION_TMP_DIR").unwrap_or_else(|_| DEFAULT_SESSION_TMP_DIR.to_string());
    let dir_path = PathBuf::from(&dir);

    if !dir_path.is_dir() {
        return Err(CliError::session(format!(
            "$RSTUDIO_SESSION_STREAM is not set and the rsession socket directory \
             ({}) does not exist. Pass --socket <path>, or run with --mode desktop.",
            dir_path.display()
        )));
    }

    let sockets = list_server_sockets();

    match sockets.len() {
        0 => Err(CliError::session(format!(
            "$RSTUDIO_SESSION_STREAM is not set and no rsession socket owned by \
             the current user was found in {}. Either rsession isn't running, or \
             you're on the wrong machine. Pass --socket <path>, or run with \
             --mode desktop.",
            dir_path.display()
        ))),
        1 => Ok(sockets.into_iter().next().unwrap()),
        _ => {
            let listing: Vec<String> = sockets
                .iter()
                .map(|p| format!("  --socket {}", p.display()))
                .collect();
            Err(CliError::session(format!(
                "$RSTUDIO_SESSION_STREAM is not set and multiple rsession sockets \
                 owned by the current user are present in {}:\n{}\n\
                 Pass one of them explicitly, or set $RSTUDIO_SESSION_STREAM.",
                dir_path.display(),
                listing.join("\n")
            )))
        }
    }
}
