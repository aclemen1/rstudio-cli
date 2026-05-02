use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "editor",
        name: "open",
        summary: "Ouvre un fichier dans le pane Source (non-modal). Retourne l'id du document.",
        description: "Wraps rstudioapi::documentOpen(path, line, col, moveCursor). The file \
                      appears as a tab in the Source pane and the user retains control. \
                      Different from `editor edit`, which opens the modal R `edit()` dialog \
                      (Save/Cancel).",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "File path (resolved to absolute via canonicalize).",
            },
            ParamSpec {
                name: "--line",
                kind: ParamKind::Integer,
                required: false,
                default: None,
                allowed: &[],
                description: "Line (1-based) where the cursor should land after opening.",
            },
            ParamSpec {
                name: "--col",
                kind: ParamKind::Integer,
                required: false,
                default: None,
                allowed: &[],
                description: "Column (1-based); combine with --line.",
            },
            ParamSpec {
                name: "--no-cursor",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Don't move the cursor (moveCursor=FALSE). Useful to open in the background.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor open ~/code/aclemen1/rstudio-cli/Cargo.toml",
                explanation: "Opens Cargo.toml in the Source pane; cursor unchanged if already open.",
            },
            ExampleSpec {
                cmd: "rstudio editor open src/main.rs --line 42 --col 5",
                explanation: "Opens then moves the cursor to (42, 5).",
            },
        ],
        returns: "{path: string, line: int|null, col: int|null, id: string}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "File not found (canonicalize fail).",
            },
            ErrorSpec {
                kind: "r_error",
                when: "rstudioapi::documentOpen rejects the path.",
            },
        ],
        rstudioapi_fn: Some("documentOpen"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "edit",
        summary: "Ouvre la modale R edit() pour le fichier (Save/Cancel). Bloquant.",
        description: "Wraps the editfile postback — standard R `edit(file = ...)` behaviour. \
                      RStudio displays a modal editor window separate from the Source pane. \
                      The user must click Save or Cancel to close it. While the modal is up, \
                      the R session is blocked, so subsequent `r exec` calls will wait. \
                      For normal (non-modal) editing, prefer `editor open`.",
        params: &[ParamSpec {
            name: "path",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "File path (resolved to absolute via canonicalize).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio editor edit /tmp/scratch.R",
            explanation: "Opens a modal editor for /tmp/scratch.R. Blocks until Save/Cancel.",
        }],
        returns: "{path: string, exit_code: int}",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "File not found.",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("postback:editfile"),
    },
    ActionSpec {
        category: "editor",
        name: "read",
        summary: "Read a file's contents (the on-disk file, not the editor buffer).",
        description: "Wraps the get_file_contents RPC [path, encoding=UTF-8]. To read the \
                      live editor buffer (including unsaved changes), use \
                      `editor context --include-contents`.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "File path (canonicalized CLI-side).",
            },
            ParamSpec {
                name: "--encoding",
                kind: ParamKind::String,
                required: false,
                default: Some("UTF-8"),
                allowed: &[],
                description: "Encoding forwarded to get_file_contents.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio editor read ~/projects/foo/main.R",
            explanation: "Returns the on-disk contents of main.R.",
        }],
        returns: "{path: string, contents: string}",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "File not found.",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("get_file_contents"),
    },
    ActionSpec {
        category: "editor",
        name: "context",
        summary: "Context of the active document in the Source pane (path, selection, etc.).",
        description: "Wraps rstudioapi::getSourceEditorContext(). Without the flag, returns id, \
                      path, and the selections list (start/end positions + selected text). \
                      With --include-contents, adds the buffer lines (live, including unsaved edits).",
        params: &[ParamSpec {
            name: "--include-contents",
            kind: ParamKind::Bool,
            required: false,
            default: Some("false"),
            allowed: &[],
            description: "Include the live buffer contents (may be large).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio editor context",
            explanation: "Retourne {id, path, selections: [{start_row, start_col, end_row, end_col, text}]}.",
        }],
        returns: "{id, path, selections, contents?}",
        errors: &[],
        rstudioapi_fn: Some("getSourceEditorContext"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "active-context",
        summary: "Context of the document the user last interacted with (Source pane OR console).",
        description: "Wraps rstudioapi::getActiveDocumentContext(). Unlike `editor context` \
                      (which always targets the Source pane), this returns whichever document \
                      currently has focus — including the R console (id = '#console'). \
                      Useful when the agent needs to know where the user is acting right now.",
        params: &[ParamSpec {
            name: "--include-contents",
            kind: ParamKind::Bool,
            required: false,
            default: Some("false"),
            allowed: &[],
            description: "Include the live buffer contents (may be large).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio editor active-context",
            explanation: "Returns the active document — could be a file or the R console.",
        }],
        returns: "{id, path, selections, contents?}",
        errors: &[],
        rstudioapi_fn: Some("getActiveDocumentContext"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "insert",
        summary: "Insert text into the active document.",
        description: "Wraps rstudioapi::insertText(). Without --at, inserts at the cursor. \
                      --at start = (1,1), --at end = end of file, --at L:C = explicit position.",
        params: &[
            ParamSpec {
                name: "text",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Text to insert.",
            },
            ParamSpec {
                name: "--at",
                kind: ParamKind::String,
                required: false,
                default: Some("cursor"),
                allowed: &["cursor", "start", "end"],
                description: "Insertion position. Special values 'cursor', 'start', 'end', or 'L:C'.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor insert 'TODO\\n' --at start",
                explanation: "Prepends 'TODO\\n' to the file.",
            },
            ExampleSpec {
                cmd: "rstudio editor insert 'x' --at 5:1",
                explanation: "Inserts 'x' at line 5, column 1.",
            },
        ],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Invalid position or no active editor.",
        }],
        rstudioapi_fn: Some("insertText"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "close",
        summary: "Close a document by id (Source pane).",
        description: "Wraps .rs.api.documentClose(id, save). Goes through the C call \
                      `rs_requestDocumentClose`, which actually unmounts the tab. Distinct \
                      from the close_document RPC, which only enqueues a UI event that may \
                      not be applied. --save controls what to do with unsaved changes: \
                      true = save silently, false = discard, ask = prompt the user (modal).",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Document id (from `editor open` / `editor context`).",
            },
            ParamSpec {
                name: "--save",
                kind: ParamKind::Enum,
                required: false,
                default: Some("true"),
                allowed: &["true", "false", "ask"],
                description: "How to handle unsaved changes.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor close 947E7AED",
                explanation: "Save unsaved changes silently and close.",
            },
            ExampleSpec {
                cmd: "rstudio editor close 947E7AED --save false",
                explanation: "Discard unsaved changes and close.",
            },
        ],
        returns: "{id: string, saved: 'true'|'false'|'ask'}",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Unknown id or .rs.api.documentClose error.",
        }],
        rstudioapi_fn: Some("documentClose"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "save",
        summary: "Save a document by id (or the active one if --id is omitted).",
        description: "Wraps .rs.api.documentSave(id). Returns the saved document's id.",
        params: &[ParamSpec {
            name: "--id",
            kind: ParamKind::String,
            required: false,
            default: None,
            allowed: &[],
            description: "Document id; defaults to the active document (excluding the console).",
        }],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor save",
                explanation: "Save the active document.",
            },
            ExampleSpec {
                cmd: "rstudio editor save --id 947E7AED",
                explanation: "Save document 947E7AED.",
            },
        ],
        returns: "{id: string}",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Unknown id or write failure.",
        }],
        rstudioapi_fn: Some("documentSave"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "save-all",
        summary: "Save every dirty document in the Source pane.",
        description: "Wraps .rs.api.documentSaveAll().",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio editor save-all",
            explanation: "Save every dirty buffer.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("documentSaveAll"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "select",
        summary: "Set the selection (or move the cursor) in the active document.",
        description: "Wraps rstudioapi::setSelectionRanges(). Range format: 'L:C' (cursor only, \
                      no selection) or 'L1:C1-L2:C2' (selection range).",
        params: &[ParamSpec {
            name: "range",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Range to select. 'L:C' or 'L1:C1-L2:C2'. 1-based.",
        }],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor select 10:1",
                explanation: "Moves the cursor to line 10, column 1.",
            },
            ExampleSpec {
                cmd: "rstudio editor select 5:1-7:80",
                explanation: "Selects from (5,1) to (7,80).",
            },
        ],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Invalid range format.",
        }],
        rstudioapi_fn: Some("setSelectionRanges"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum EditorCmd {
    /// Ouvre un fichier dans le pane Source (non-modal). Retourne l'id du document.
    Open {
        path: PathBuf,
        #[arg(long)]
        line: Option<u32>,
        #[arg(long)]
        col: Option<u32>,
        /// Don't move the cursor (moveCursor=FALSE).
        #[arg(long)]
        no_cursor: bool,
    },
    /// Open the modal R `edit()` dialog for the file (Save/Cancel).
    /// Blocks the R session until the modal is dismissed.
    Edit { path: PathBuf },
    /// Context of whatever document the user has focus on (Source pane or console).
    ActiveContext {
        /// Include the live buffer contents (may be large).
        #[arg(long)]
        include_contents: bool,
    },
    /// Read the on-disk file contents (not the live editor buffer).
    Read {
        path: PathBuf,
        /// Encoding (default UTF-8).
        #[arg(long, default_value = "UTF-8")]
        encoding: String,
    },
    /// Context of the active document in the Source pane.
    Context {
        /// Include the live buffer contents (may be large).
        #[arg(long)]
        include_contents: bool,
    },
    /// Insert text into the active document.
    Insert {
        text: String,
        /// Insertion position: 'cursor' (default), 'start', 'end', or 'L:C'.
        #[arg(long, default_value = "cursor")]
        at: String,
    },
    /// Set the selection (or cursor) in the active document.
    Select {
        /// Range: 'L:C' or 'L1:C1-L2:C2'.
        range: String,
    },
    /// Close a document by id (true close, via .rs.api.documentClose).
    Close {
        id: String,
        /// How to handle unsaved changes: true = save silently, false = discard, ask = prompt.
        #[arg(long, default_value = "true")]
        save: String,
    },
    /// Save a document by id (or the active document if --id is omitted).
    Save {
        #[arg(long)]
        id: Option<String>,
    },
    /// Save every dirty document in the Source pane.
    SaveAll,
}

pub fn run(cmd: &EditorCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        EditorCmd::Open {
            path,
            line,
            col,
            no_cursor,
        } => open(rpc, path, *line, *col, *no_cursor),
        EditorCmd::Edit { path } => edit_modal(rpc, path),
        EditorCmd::ActiveContext { include_contents } => {
            context_via(rpc, "getActiveDocumentContext", *include_contents)
        }
        EditorCmd::Read { path, encoding } => read(rpc, path, encoding),
        EditorCmd::Context { include_contents } => {
            context_via(rpc, "getSourceEditorContext", *include_contents)
        }
        EditorCmd::Insert { text, at } => insert(rpc, text, at),
        EditorCmd::Select { range } => select(rpc, range),
        EditorCmd::Close { id, save } => close(rpc, id, save),
        EditorCmd::Save { id } => save(rpc, id.as_deref()),
        EditorCmd::SaveAll => save_all(rpc),
    }
}

fn open(
    rpc: &RpcClient<'_>,
    path: &PathBuf,
    line: Option<u32>,
    col: Option<u32>,
    no_cursor: bool,
) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();

    let line_arg = line.map(|l| format!("{l}L")).unwrap_or_else(|| "-1L".into());
    let col_arg = col.map(|c| format!("{c}L")).unwrap_or_else(|| "-1L".into());
    let move_cursor = if no_cursor { "FALSE" } else { "TRUE" };

    let r_code = format!(
        "cat(rstudioapi::documentOpen({path}, line = {line_arg}, col = {col_arg}, moveCursor = {move_cursor}))",
        path = r_quote(&abs_str),
    );
    let id = r_eval::run(rpc, &r_code)?;

    Ok(Some(json!({
        "path": abs_str,
        "line": line,
        "col": col,
        "id": id.trim(),
    })))
}

fn edit_modal(rpc: &RpcClient<'_>, path: &PathBuf) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();
    let pb = rpc.postback("editfile", &abs_str)?;
    Ok(Some(json!({
        "path": abs_str,
        "exit_code": pb.exit_code,
    })))
}

fn read(rpc: &RpcClient<'_>, path: &PathBuf, encoding: &str) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();
    let raw = rpc.rpc(
        "get_file_contents",
        vec![
            Value::String(abs_str.clone()),
            Value::String(encoding.to_string()),
        ],
    )?;
    let contents = raw.as_str().unwrap_or("").to_string();
    Ok(Some(json!({
        "path": abs_str,
        "contents": contents,
    })))
}

/// Shared implementation for `editor context` (getSourceEditorContext) and
/// `editor active-context` (getActiveDocumentContext). Both return the same
/// shape; only the rstudioapi getter differs.
fn context_via(
    rpc: &RpcClient<'_>,
    api_fn: &str,
    include_contents: bool,
) -> Result<Option<Value>, CliError> {
    let contents_field = if include_contents {
        "contents = paste(ctx$contents, collapse = \"\\n\"),"
    } else {
        ""
    };
    let r_code = format!(
        r#"local({{
  ctx <- rstudioapi::{api_fn}()
  if (is.null(ctx)) {{
    cat("null")
    return(invisible())
  }}
  selections <- lapply(ctx$selection, function(s) {{
    list(
      start_row = as.integer(s$range$start[[1]]),
      start_col = as.integer(s$range$start[[2]]),
      end_row = as.integer(s$range$end[[1]]),
      end_col = as.integer(s$range$end[[2]]),
      text = s$text
    )
  }})
  out <- list(
    id = ctx$id,
    path = ctx$path,
    {contents_field}
    selections = selections
  )
  cat(jsonlite::toJSON(out, auto_unbox = TRUE, null = "null"))
}})"#
    );
    let raw = r_eval::run(rpc, &r_code)?;
    if raw.trim() == "null" {
        return Ok(Some(Value::Null));
    }
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("editor context ({api_fn}): invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn insert(rpc: &RpcClient<'_>, text: &str, at: &str) -> Result<Option<Value>, CliError> {
    let location = match at {
        "cursor" => "NULL".to_string(),
        "start" => "rstudioapi::document_position(1L, 1L)".to_string(),
        "end" => {
            // End = (lines, last_col + 1). Compute on the R side.
            "{ ctx <- rstudioapi::getSourceEditorContext(); \
               n <- length(ctx$contents); \
               last <- if (n > 0) nchar(ctx$contents[n]) + 1L else 1L; \
               rstudioapi::document_position(if (n > 0) n else 1L, last) }"
                .to_string()
        }
        custom => {
            let (line, col) = parse_line_col(custom)
                .ok_or_else(|| CliError::user(format!("invalid --at value: {custom}")))?;
            format!("rstudioapi::document_position({line}L, {col}L)")
        }
    };
    let r_code = format!(
        "rstudioapi::insertText(location = {location}, text = {})",
        r_quote(text)
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn select(rpc: &RpcClient<'_>, range: &str) -> Result<Option<Value>, CliError> {
    let r_range = match parse_range(range) {
        Some(((l1, c1), (l2, c2))) => format!(
            "rstudioapi::document_range(rstudioapi::document_position({l1}L, {c1}L), \
                                         rstudioapi::document_position({l2}L, {c2}L))"
        ),
        None => {
            return Err(CliError::user(format!(
                "invalid range '{range}'. Expected 'L:C' or 'L1:C1-L2:C2'."
            )));
        }
    };
    let r_code = format!("rstudioapi::setSelectionRanges(list({r_range}))");
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn close(rpc: &RpcClient<'_>, id: &str, save: &str) -> Result<Option<Value>, CliError> {
    let save_arg = match save {
        "true" => "TRUE",
        "false" => "FALSE",
        "ask" => "\"ask\"",
        other => {
            return Err(CliError::user(format!(
                "invalid --save '{other}'. Expected: true, false, ask."
            )));
        }
    };
    let r_code = format!(
        ".rs.api.documentClose(id = {}, save = {save_arg})",
        r_quote(id)
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({
        "id": id,
        "saved": save,
    })))
}

fn save(rpc: &RpcClient<'_>, id: Option<&str>) -> Result<Option<Value>, CliError> {
    let id_arg = match id {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    let r_code = format!(
        r#"local({{
  .__id <- .rs.api.documentSave({id_arg})
  if (is.null(.__id)) cat("null") else cat(jsonlite::toJSON(list(id = .__id), auto_unbox = TRUE))
}})"#
    );
    let raw = r_eval::run(rpc, &r_code)?;
    if raw.trim() == "null" {
        return Ok(Some(Value::Null));
    }
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("editor save: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn save_all(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    r_eval::run_silent(rpc, ".rs.api.documentSaveAll()")?;
    Ok(None)
}

fn parse_line_col(s: &str) -> Option<(u32, u32)> {
    let (l, c) = s.split_once(':')?;
    Some((l.parse().ok()?, c.parse().ok()?))
}

fn parse_range(s: &str) -> Option<((u32, u32), (u32, u32))> {
    if let Some((a, b)) = s.split_once('-') {
        Some((parse_line_col(a)?, parse_line_col(b)?))
    } else {
        let pos = parse_line_col(s)?;
        Some((pos, pos))
    }
}
