use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "term",
        name: "list",
        summary: "List open terminals with their full context.",
        description: "Wraps rstudioapi::terminalList() + terminalContext() for each id.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio term list",
            explanation: "Returns an array of terminals, each with id, caption, working_dir, shell, pid, busy, ...",
        }],
        returns: "{terminals: [{id, caption, title, working_dir, shell, running, busy, exit_code, pid, cols, rows, lines, connection}]}",
        errors: &[],
        rstudioapi_fn: Some("terminalList"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "buffer",
        summary: "Read the buffer (lines) of a terminal. Live.",
        description: "rstudioapi::terminalBuffer(id, stripAnsi). Strips ANSI SGR codes \
                      (colors) by default, keeps OSC codes (window titles, etc.).",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Terminal id (8 hex chars, from `term list`).",
            },
            ParamSpec {
                name: "--limit",
                kind: ParamKind::Integer,
                required: false,
                default: None,
                allowed: &[],
                description: "Max number of lines (most recent first).",
            },
            ParamSpec {
                name: "--ansi",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Keep ANSI SGR codes (stripped by default).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio term buffer 93555F0A --limit 20",
            explanation: "Last 20 lines of terminal 93555F0A.",
        }],
        returns: "{id: string, lines: [string]}",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Unknown terminal id.",
        }],
        rstudioapi_fn: Some("terminalBuffer"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "context",
        summary: "Return the full context of a terminal (metadata).",
        description: "rstudioapi::terminalContext(id) — every field (handle, caption, pid, ...).",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Terminal id.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio term context 93555F0A",
            explanation: "Returns handle, caption, working_dir, shell, pid, busy, exit_code, ...",
        }],
        returns: "{handle, caption, title, working_dir, shell, running, busy, exit_code, connection, sequence, lines, cols, rows, pid, full_screen, restarted}",
        errors: &[],
        rstudioapi_fn: Some("terminalContext"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "create",
        summary: "Create a new terminal and return its id.",
        description: "rstudioapi::terminalCreate(caption, show, shellType). \
                      show=FALSE by default so the pane focus isn't disturbed.",
        params: &[
            ParamSpec {
                name: "--name",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Caption shown in the Terminal pane.",
            },
            ParamSpec {
                name: "--shell-type",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Shell type (e.g. bash, zsh, default). NULL = default.",
            },
            ParamSpec {
                name: "--show",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Focus the Terminal pane after creation.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio term create --name \"my-task\"",
            explanation: "Creates a terminal named my-task; returns {id: \"...\"}.",
        }],
        returns: "{id: string}",
        errors: &[],
        rstudioapi_fn: Some("terminalCreate"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "send",
        summary: "Send text to the terminal WITHOUT a trailing Enter (poked at the current prompt).",
        description: "Gotcha: a subsequent `term exec` appends to the current line instead of \
                      starting a new command. Prefer multiple `term exec` calls to run distinct commands.",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Terminal id.",
            },
            ParamSpec {
                name: "text",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Text to insert.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio term send 93555F0A \"git status\"",
            explanation: "Types `git status` into the terminal without executing.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalSend"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "exec",
        summary: "Send text to the terminal WITH a trailing Enter (execute). Fire-and-forget.",
        description: "Doesn't wait for the command to finish. Read `term buffer <id>` afterwards \
                      to retrieve the result.",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Terminal id.",
            },
            ParamSpec {
                name: "text",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Code to run (a trailing newline is appended if absent).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio term exec 93555F0A 'ls -la /tmp'",
            explanation: "Types and executes `ls -la /tmp` in the terminal.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalSend"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "kill",
        summary: "Kill a terminal (removes it from the pane).",
        description: "rstudioapi::terminalKill(id).",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Terminal id.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio term kill 0ACC78A5",
            explanation: "Terminates and removes terminal 0ACC78A5.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalKill"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "clear",
        summary: "Clear a terminal's buffer.",
        description: "rstudioapi::terminalClear(id).",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Terminal id.",
        }],
        examples: &[],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalClear"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "activate",
        summary: "Focus the Terminal pane and activate this terminal.",
        description: "rstudioapi::terminalActivate(id). User-visible.",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Terminal id.",
        }],
        examples: &[],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalActivate"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum TermCmd {
    /// List open terminals in the Terminal pane with their context.
    List,
    /// Read the buffer (lines) of a terminal. Live.
    Buffer {
        /// Terminal id (8 hex chars, as returned by `term list`).
        id: String,
        /// Max number of lines (most recent first).
        #[arg(long, short = 'n')]
        limit: Option<usize>,
        /// Keep ANSI codes (stripped by default).
        #[arg(long)]
        ansi: bool,
    },
    /// Create a new terminal. Returns its id.
    Create {
        /// Caption shown in the Terminal pane.
        #[arg(long)]
        name: Option<String>,
        /// Shell type (e.g. "bash", "zsh", "default").
        #[arg(long)]
        shell_type: Option<String>,
        /// Focus the Terminal pane (default: FALSE).
        #[arg(long)]
        show: bool,
    },
    /// Send text to the terminal without a trailing newline (no Enter).
    /// The text is poked at the current prompt. A subsequent `term exec` will
    /// append to that line instead of starting a new command — prefer several
    /// `term exec` calls to run distinct commands.
    Send {
        id: String,
        text: String,
    },
    /// Send text to the terminal with a trailing newline (equivalent to pressing Enter).
    /// Fire-and-forget: does not block or wait for the command to finish. Read
    /// `term buffer <id>` afterwards to retrieve the output.
    Exec {
        id: String,
        text: String,
    },
    /// Kill a terminal (removes it from the pane).
    Kill {
        id: String,
    },
    /// Clear a terminal's buffer.
    Clear {
        id: String,
    },
    /// Return the full context of a terminal (caption, working_dir, shell, pid, ...).
    Context {
        id: String,
    },
    /// Focus the Terminal pane and activate this terminal.
    Activate {
        id: String,
    },
}

pub fn run(cmd: &TermCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        TermCmd::List => list(rpc),
        TermCmd::Buffer { id, limit, ansi } => buffer(rpc, id, *limit, *ansi),
        TermCmd::Create { name, shell_type, show } => create(rpc, name.as_deref(), shell_type.as_deref(), *show),
        TermCmd::Send { id, text } => send(rpc, id, text),
        TermCmd::Exec { id, text } => exec(rpc, id, text),
        TermCmd::Kill { id } => kill(rpc, id),
        TermCmd::Clear { id } => clear(rpc, id),
        TermCmd::Context { id } => context(rpc, id),
        TermCmd::Activate { id } => activate(rpc, id),
    }
}

fn list(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let r = r#"local({
  ids <- rstudioapi::terminalList()
  if (length(ids) == 0) {
    cat("[]")
  } else {
    items <- lapply(ids, function(id) {
      ctx <- rstudioapi::terminalContext(id)
      list(
        id = ctx$handle,
        caption = ctx$caption,
        title = ctx$title,
        working_dir = ctx$working_dir,
        shell = ctx$shell,
        running = ctx$running,
        busy = ctx$busy,
        exit_code = ctx$exit_code,
        pid = ctx$pid,
        cols = ctx$cols,
        rows = ctx$rows,
        lines = ctx$lines,
        connection = ctx$connection
      )
    })
    cat(jsonlite::toJSON(items, auto_unbox = TRUE, null = "null"))
  }
})"#;
    let raw = r_eval::run(rpc, r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("term list: invalid JSON from R: {e}; raw: {raw}")))?;
    Ok(Some(json!({ "terminals": parsed })))
}

fn context(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    let r = format!(
        r#"local({{
  ctx <- rstudioapi::terminalContext({id_q})
  cat(jsonlite::toJSON(ctx, auto_unbox = TRUE, null = "null"))
}})"#,
        id_q = r_quote(id)
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("term context: invalid JSON from R: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn buffer(
    rpc: &RpcClient<'_>,
    id: &str,
    limit: Option<usize>,
    ansi: bool,
) -> Result<Option<Value>, CliError> {
    let strip = if ansi { "FALSE" } else { "TRUE" };
    // We want a JSON array of lines so the CLI can return structured output
    // and the `--limit N` knob works on the server side too (saves transport).
    let n_clause = match limit {
        Some(n) => format!("buf <- tail(buf, {n}); "),
        None => String::new(),
    };
    let r = format!(
        r#"local({{
  buf <- rstudioapi::terminalBuffer({id_q}, stripAnsi = {strip})
  {n_clause}cat(jsonlite::toJSON(buf, auto_unbox = FALSE))
}})"#,
        id_q = r_quote(id),
    );
    let raw = r_eval::run(rpc, &r)?;
    let lines: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("term buffer: invalid JSON from R: {e}; raw: {raw}")))?;
    Ok(Some(json!({ "id": id, "lines": lines })))
}

fn create(
    rpc: &RpcClient<'_>,
    name: Option<&str>,
    shell_type: Option<&str>,
    show: bool,
) -> Result<Option<Value>, CliError> {
    let caption_arg = name.map(r_quote).unwrap_or_else(|| "NULL".into());
    let shell_arg = shell_type.map(r_quote).unwrap_or_else(|| "NULL".into());
    let show_r = if show { "TRUE" } else { "FALSE" };
    let r = format!(
        r#"cat(rstudioapi::terminalCreate(caption = {caption_arg}, show = {show_r}, shellType = {shell_arg}))"#
    );
    let id = r_eval::run(rpc, &r)?;
    Ok(Some(json!({ "id": id.trim() })))
}

fn send(rpc: &RpcClient<'_>, id: &str, text: &str) -> Result<Option<Value>, CliError> {
    let r = format!(
        "rstudioapi::terminalSend({}, {})",
        r_quote(id),
        r_quote(text)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn exec(rpc: &RpcClient<'_>, id: &str, text: &str) -> Result<Option<Value>, CliError> {
    let with_newline = if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    };
    let r = format!(
        "rstudioapi::terminalSend({}, {})",
        r_quote(id),
        r_quote(&with_newline)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn kill(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    let r = format!("rstudioapi::terminalKill({})", r_quote(id));
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn clear(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    let r = format!("rstudioapi::terminalClear({})", r_quote(id));
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn activate(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    let r = format!("rstudioapi::terminalActivate({})", r_quote(id));
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}
