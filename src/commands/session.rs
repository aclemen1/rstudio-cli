use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};
use crate::session;

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
    ActionSpec {
        category: "session",
        name: "list",
        summary: "List all active RStudio Server sessions for the current user.",
        description: "Scans $RS_SESSION_TMP_DIR for rsession Unix sockets owned by \
                      the current user. Each entry includes the --socket path to \
                      pass to any other command to target that specific session. \
                      Does not require a live session — safe to call even when \
                      $RSTUDIO_SESSION_STREAM is unset or ambiguous. \
                      Server mode only; Desktop multi-process listing is not yet supported.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio session list",
            explanation: "Returns {sessions: [{socket}]}. Use `--socket <path>` with \
                          any other command to target a specific session.",
        }],
        returns: "{sessions: [{socket: string}]}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: None,
    },
];

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// Return version, mode, user, color console support, and active project.
    Info,
    /// Restart the R session. Requires --confirm to actually fire.
    Restart {
        /// R code to run after the restart.
        #[arg(long)]
        command: Option<String>,
        /// Required to actually invoke the restart.
        #[arg(long)]
        confirm: bool,
    },
    /// List all active RStudio Server sessions for the current user (no session required).
    List,
}

pub fn run(cmd: &SessionCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        SessionCmd::Info => info(rpc),
        SessionCmd::Restart { command, confirm } => restart(rpc, command.as_deref(), *confirm),
        // List is handled upstream (no session needed); unreachable here.
        SessionCmd::List => unreachable!("session list is dispatched before Session::detect"),
    }
}

/// List active rsession sockets — called directly from the dispatcher, no RPC needed.
pub fn list_sessions() -> Result<Option<Value>, CliError> {
    let sockets = session::list_server_sockets();
    let entries: Vec<Value> = sockets
        .iter()
        .map(|p| json!({ "socket": p.display().to_string() }))
        .collect();
    Ok(Some(json!({ "sessions": entries })))
}

fn info(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    // Delegated to the rstudiocli.mcp R package: see `r-package/R/session.R`.
    let r_code = r#"cat(jsonlite::toJSON(
        rstudiocli.mcp::session_info(),
        auto_unbox = TRUE, na = "null", null = "null"
    ))"#;
    let raw = r_eval::run(rpc, r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("session info: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
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
    let r_code = format!("rstudiocli.mcp::session_restart(command = {cmd_arg})");
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}
