use std::fs;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::RpcClient;
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};
use crate::session::Session;

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "console",
        name: "history",
        summary: "List the latest commands typed by the user in the R console.",
        description: "Live — reads the in-memory R history via the get_recent_history RPC.",
        params: &[ParamSpec {
            name: "--limit",
            kind: ParamKind::Integer,
            required: false,
            default: Some("100"),
            allowed: &[],
            description: "Max number of commands (most recent first). Must be > 0.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio console history --limit 5",
            explanation: "Returns the 5 most recently typed commands.",
        }],
        returns: "{commands: [string]}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("get_recent_history"),
    },
    ActionSpec {
        category: "console",
        name: "actions",
        summary: "Read the on-disk console buffer snapshot (last suspend; not live).",
        description: "Decodes suspended-session-data/console_actions {type, data}. \
                      type codes: 0=prompt, 1=input, 2=output, 3=error. \
                      Check last_modified_unix in the return value to gauge freshness.",
        params: &[
            ParamSpec {
                name: "--limit",
                kind: ParamKind::Integer,
                required: false,
                default: None,
                allowed: &[],
                description: "Max number of entries (most recent first).",
            },
            ParamSpec {
                name: "--types",
                kind: ParamKind::Enum,
                required: false,
                default: None,
                allowed: &["prompt", "input", "output", "error"],
                description: "Filter by type. Multi-valued, comma-separated.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio console actions --types output --limit 10",
            explanation: "Last 10 outputs from the last snapshot.",
        }],
        returns: "{snapshot_path, last_modified_unix, is_live: false, entries: [{type, code, text}]}",
        errors: &[ErrorSpec {
            kind: "session_unavailable",
            when: "No console_actions file (session was never suspended).",
        }],
        rstudioapi_fn: None,
        rpc_method: None,
    },
    ActionSpec {
        category: "console",
        name: "activate",
        summary: "Move keyboard focus to the R console pane.",
        description: "Wraps the named RStudio command `activateConsole` (via \
                      .rs.api.executeCommand). Symmetric counterpart to `term activate <id>` \
                      for the R console (which has no id — there's only one).",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio console activate",
            explanation: "User's cursor lands in the R console, ready for typing.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "console",
        name: "context",
        summary: "Context of the R console editor (currently typed input + cursor position).",
        description: "Wraps rstudioapi::getConsoleEditorContext(). Returns what the user is \
                      currently typing in the R console (id always = '#console'), the cursor \
                      position, and the selection if any. Live.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio console context",
            explanation: "Returns {id: '#console', path: '', selections: [...], contents?}.",
        }],
        returns: "{id, path, selections, contents?}",
        errors: &[],
        rstudioapi_fn: Some("getConsoleEditorContext"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum ConsoleCmd {
    /// List the latest commands typed by the user in the R console (live).
    History {
        /// Max number of commands to return (most recent first).
        #[arg(long, short = 'n', default_value_t = 100)]
        limit: u32,
    },
    /// Read the on-disk console_actions snapshot (last suspend; not live).
    Actions {
        /// Max number of entries to return (most recent first).
        #[arg(long, short = 'n')]
        limit: Option<usize>,
        /// Filter by action type. Multi-valued, comma-separated.
        /// Without this flag, all types are returned.
        #[arg(long, value_delimiter = ',')]
        types: Vec<ActionType>,
    },
    /// Context of the R console editor (currently typed input + cursor position). Live.
    Context,
    /// Move keyboard focus to the R console pane.
    Activate,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionType {
    Prompt,
    Input,
    Output,
    Error,
}

impl ActionType {
    fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Prompt),
            1 => Some(Self::Input),
            2 => Some(Self::Output),
            3 => Some(Self::Error),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Input => "input",
            Self::Output => "output",
            Self::Error => "error",
        }
    }
}

pub fn run(
    cmd: &ConsoleCmd,
    rpc: &RpcClient<'_>,
    session: &Session,
) -> Result<Option<Value>, CliError> {
    match cmd {
        ConsoleCmd::History { limit } => history(rpc, *limit),
        ConsoleCmd::Actions { limit, types } => actions(session, *limit, types),
        ConsoleCmd::Context => context(rpc),
        ConsoleCmd::Activate => activate(rpc),
    }
}

fn activate(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    r_eval::run_silent(rpc, "rstudiocli::console_activate()")?;
    Ok(None)
}

fn context(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let r_code = r#"local({
  ctx <- rstudioapi::getConsoleEditorContext()
  if (is.null(ctx)) {
    cat("null")
    return(invisible())
  }
  selections <- lapply(ctx$selection, function(s) {
    list(
      start_row = as.integer(s$range$start[[1]]),
      start_col = as.integer(s$range$start[[2]]),
      end_row = as.integer(s$range$end[[1]]),
      end_col = as.integer(s$range$end[[2]]),
      text = s$text
    )
  })
  out <- list(
    id = ctx$id,
    path = ctx$path,
    contents = paste(ctx$contents, collapse = "\n"),
    selections = selections
  )
  cat(jsonlite::toJSON(out, auto_unbox = TRUE, null = "null"))
})"#;
    let raw = r_eval::run(rpc, r_code)?;
    if raw.trim() == "null" {
        return Ok(Some(Value::Null));
    }
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("console context: invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

fn history(rpc: &RpcClient<'_>, limit: u32) -> Result<Option<Value>, CliError> {
    if limit == 0 {
        return Err(CliError::user("--limit must be > 0"));
    }
    let raw = rpc.rpc("get_recent_history", vec![json!(limit)])?;
    let commands: Vec<String> = raw
        .get("command")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(json!({ "commands": commands })))
}

fn actions(
    session: &Session,
    limit: Option<usize>,
    types: &[ActionType],
) -> Result<Option<Value>, CliError> {
    let dir = session.require_session_dir()?;
    let path = dir.join("suspended-session-data").join("console_actions");
    let metadata = fs::metadata(&path).map_err(|e| {
        CliError::session(format!(
            "console_actions snapshot not found at {} ({e}). \
             A snapshot is only written when the session has been suspended at least once.",
            path.display()
        ))
    })?;
    let last_modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    let content = fs::read_to_string(&path)
        .map_err(|e| CliError::internal(format!("read {}: {e}", path.display())))?;
    let parsed: Value = serde_json::from_str(&content)
        .map_err(|e| CliError::internal(format!("parse {}: {e}", path.display())))?;

    let type_codes = parsed
        .get("type")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CliError::internal(format!("{}: missing 'type' array", path.display())))?;
    let data_arr = parsed
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CliError::internal(format!("{}: missing 'data' array", path.display())))?;

    let allow_all = types.is_empty();
    let mut entries: Vec<Value> = type_codes
        .iter()
        .zip(data_arr.iter())
        .filter_map(|(t, d)| {
            let code = t.as_i64()?;
            let kind = ActionType::from_code(code);
            let kind_str = kind
                .map(ActionType::as_str)
                .unwrap_or("unknown")
                .to_string();
            let text = d.as_str().unwrap_or("").to_string();
            let keep = allow_all || kind.map(|k| types.contains(&k)).unwrap_or(false);
            if !keep {
                return None;
            }
            Some(json!({
                "type": kind_str,
                "code": code,
                "text": text,
            }))
        })
        .collect();

    if let Some(n) = limit {
        let drop = entries.len().saturating_sub(n);
        entries.drain(..drop);
    }

    Ok(Some(json!({
        "snapshot_path": path.to_string_lossy(),
        "last_modified_unix": last_modified,
        "is_live": false,
        "entries": entries,
    })))
}
