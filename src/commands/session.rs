use clap::Subcommand;
use serde_json::Value;

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "session",
        name: "info",
        summary: "Return overall information about the active RStudio session.",
        description: "Combines rstudioapi::versionInfo, getVersion, userIdentity, \
                      systemUsername, hasColorConsole, getActiveProject into a single \
                      JSON payload — the kind of context an agent wants to know on \
                      startup (RStudio version, mode, user, current project).",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio session info",
            explanation: "Returns version + mode + user_identity + system_username + has_color_console + active_project.",
        }],
        returns: "{version, long_version, release_name, mode, user_identity, system_username, has_color_console, active_project}",
        errors: &[],
        rstudioapi_fn: Some("versionInfo"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "session",
        name: "project",
        summary: "Return the path of the active RStudio project (null if none).",
        description: "Wraps rstudioapi::getActiveProject().",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio session project",
            explanation: "Returns {path: '/path/to/project'} or {path: null}.",
        }],
        returns: "{path: string|null}",
        errors: &[],
        rstudioapi_fn: Some("getActiveProject"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "session",
        name: "open-project",
        summary: "Open an RStudio project (DISRUPTIVE: switches the user's session context).",
        description: "Wraps rstudioapi::openProject(path, newSession). When --new-session \
                      is passed the project opens in a new RStudio session; otherwise it \
                      replaces the current one (the R session restarts). Use carefully.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: ".Rproj path or project root directory.",
            },
            ParamSpec {
                name: "--new-session",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Open in a new RStudio session instead of replacing the current one.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio session open-project ~/projects/foo",
            explanation: "Replace the current session by foo's project (current R state lost).",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Path not found.",
        }],
        rstudioapi_fn: Some("openProject"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "session",
        name: "restart",
        summary: "Restart the R session (DESTRUCTIVE: drops in-memory state).",
        description: "Wraps rstudioapi::restartSession(command). All in-memory R objects \
                      are lost; if --command is given, that R code runs after restart. \
                      Requires --confirm to actually fire — bare invocation just describes \
                      what would happen.",
        params: &[
            ParamSpec {
                name: "--command",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "R code to run after the restart (passed to restartSession).",
            },
            ParamSpec {
                name: "--confirm",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Required to actually invoke the restart. Without it the action \
                              prints a description and exits with user_error.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio session restart",
                explanation: "Returns user_error: refuses to restart without --confirm.",
            },
            ExampleSpec {
                cmd: "rstudio session restart --confirm --command 'library(tidyverse)'",
                explanation: "Restart and re-run library(tidyverse) afterwards.",
            },
        ],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Called without --confirm.",
        }],
        rstudioapi_fn: Some("restartSession"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// Return version, mode, user, color console support, and active project.
    Info,
    /// Return the active RStudio project path (or null).
    Project,
    /// Open an RStudio project (replaces current session unless --new-session).
    OpenProject {
        path: String,
        /// Open in a new session instead of replacing the current one.
        #[arg(long)]
        new_session: bool,
    },
    /// Restart the R session. Requires --confirm to actually fire.
    Restart {
        /// R code to run after the restart.
        #[arg(long)]
        command: Option<String>,
        /// Required to actually invoke the restart.
        #[arg(long)]
        confirm: bool,
    },
}

pub fn run(cmd: &SessionCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        SessionCmd::Info => info(rpc),
        SessionCmd::Project => project(rpc),
        SessionCmd::OpenProject { path, new_session } => open_project(rpc, path, *new_session),
        SessionCmd::Restart { command, confirm } => restart(rpc, command.as_deref(), *confirm),
    }
}

fn info(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    // versionInfo() is a list with fields {version, long_version, release_name, mode, citation};
    // we project the useful fields and add user / project info from the other helpers.
    let r_code = r#"local({
  vi <- rstudioapi::versionInfo()
  proj <- rstudioapi::getActiveProject()
  out <- list(
    version = as.character(vi$version),
    long_version = vi$long_version,
    release_name = vi$release_name,
    mode = vi$mode,
    user_identity = rstudioapi::userIdentity(),
    system_username = rstudioapi::systemUsername(),
    has_color_console = rstudioapi::hasColorConsole(),
    active_project = if (is.null(proj)) NA else proj
  )
  cat(jsonlite::toJSON(out, auto_unbox = TRUE, na = "null", null = "null"))
})"#;
    let raw = r_eval::run(rpc, r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("session info: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn project(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let r_code = r#"local({
  p <- rstudioapi::getActiveProject()
  if (is.null(p)) cat("{\"path\":null}")
  else cat(jsonlite::toJSON(list(path = p), auto_unbox = TRUE))
})"#;
    let raw = r_eval::run(rpc, r_code)?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("session project: invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

fn open_project(
    rpc: &RpcClient<'_>,
    path: &str,
    new_session: bool,
) -> Result<Option<Value>, CliError> {
    let new_arg = if new_session { "TRUE" } else { "FALSE" };
    let r_code = format!(
        "rstudioapi::openProject(path = {}, newSession = {new_arg})",
        r_quote(path)
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn restart(
    rpc: &RpcClient<'_>,
    command: Option<&str>,
    confirm: bool,
) -> Result<Option<Value>, CliError> {
    if !confirm {
        return Err(CliError::user(
            "session restart is destructive (drops all in-memory R state); \
             pass --confirm to actually invoke it.",
        ));
    }
    let cmd_arg = command.map(r_quote).unwrap_or_else(|| "\"\"".into());
    let r_code = format!("rstudioapi::restartSession(command = {cmd_arg})");
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}
