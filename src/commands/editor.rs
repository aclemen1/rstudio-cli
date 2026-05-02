use std::fs;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};
use crate::session::Session;

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
        name: "new",
        summary: "Create a new untitled document with given text and type.",
        description: "Wraps rstudioapi::documentNew(text, type, position, execute). Returns \
                      the new document's id. Useful to spawn a scratch buffer programmatically.",
        params: &[
            ParamSpec {
                name: "text",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Initial contents of the new document.",
            },
            ParamSpec {
                name: "--type",
                kind: ParamKind::Enum,
                required: false,
                default: Some("r"),
                allowed: &["r", "rmarkdown", "sql"],
                description: "Document type.",
            },
            ParamSpec {
                name: "--execute",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "If true, the text is executed in the console after the document is created.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio editor new 'plot(rnorm(100))' --type r --execute",
            explanation: "Create a new R document with that text, then execute it.",
        }],
        returns: "{id: string, type: string}",
        errors: &[],
        rstudioapi_fn: Some("documentNew"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "active-id",
        summary: "Return the id of the active document.",
        description: "Wraps rstudioapi::documentId(allowConsole). When --no-console is \
                      passed, returns null if the active document is the R console.",
        params: &[ParamSpec {
            name: "--no-console",
            kind: ParamKind::Bool,
            required: false,
            default: Some("false"),
            allowed: &[],
            description: "Exclude the R console from the result (return null if it's active).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio editor active-id",
            explanation: "Returns the id of whatever has focus (Source pane tab or '#console').",
        }],
        returns: "{id: string|null}",
        errors: &[],
        rstudioapi_fn: Some("documentId"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "path",
        summary: "Return the path of a document by id (or the active one if --id absent).",
        description: "Wraps rstudioapi::documentPath(id). Returns null for unsaved documents.",
        params: &[ParamSpec {
            name: "--id",
            kind: ParamKind::String,
            required: false,
            default: None,
            allowed: &[],
            description: "Document id (defaults to the active document).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio editor path --id 719092EF",
            explanation: "Returns {path: '~/projects/.../Cargo.toml'}.",
        }],
        returns: "{path: string|null}",
        errors: &[],
        rstudioapi_fn: Some("documentPath"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "set-contents",
        summary: "Replace the entire contents of a document (DESTRUCTIVE).",
        description: "Wraps rstudioapi::setDocumentContents(text, id). The full buffer \
                      is replaced; previous unsaved content is lost. If --id is omitted \
                      the active document is targeted.",
        params: &[
            ParamSpec {
                name: "text",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "New full content of the document.",
            },
            ParamSpec {
                name: "--id",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Target document id (defaults to active).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio editor set-contents 'x <- 1\\n' --id 947E7AED",
            explanation: "Replace the full content of doc 947E7AED.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Unknown id.",
        }],
        rstudioapi_fn: Some("setDocumentContents"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "modify-range",
        summary: "Replace text within a range (insert/delete/replace).",
        description: "Wraps rstudioapi::modifyRange(location, text, id). The given range \
                      is replaced by `text`. If `text` is empty the range is deleted; if \
                      the range is zero-width (start == end), `text` is inserted at that \
                      position. Range format: 'L1:C1-L2:C2' (1-based, inclusive of start, \
                      exclusive of end as per RStudio convention).",
        params: &[
            ParamSpec {
                name: "range",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Range to replace, format 'L1:C1-L2:C2'.",
            },
            ParamSpec {
                name: "text",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Replacement text.",
            },
            ParamSpec {
                name: "--id",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Target document id (defaults to active).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio editor modify-range 1:1-1:6 'NEW' --id 947E7AED",
            explanation: "Replace columns 1-5 of line 1 with 'NEW'.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Invalid range format.",
        }],
        rstudioapi_fn: Some("modifyRange"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "set-cursor",
        summary: "Move the cursor to a position in a document.",
        description: "Wraps rstudioapi::setCursorPosition(position, id). The cursor moves \
                      with no selection. Same as `editor select L:C` but more explicit.",
        params: &[
            ParamSpec {
                name: "position",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Position 'L:C', 1-based.",
            },
            ParamSpec {
                name: "--id",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Target document id (defaults to active).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio editor set-cursor 10:1",
            explanation: "Move the cursor to line 10, column 1, in the active document.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Invalid position format.",
        }],
        rstudioapi_fn: Some("setCursorPosition"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "list",
        summary: "List every document currently open in the Source pane.",
        description: "RStudio's rsession exposes no RPC method to enumerate open documents \
                      (verified against the rstudio source tree: SessionSource.cpp registers \
                      24 methods, none returns a list — the full list is shipped only via \
                      `client_init`, which we must NOT call). \
                      \
                      We therefore enumerate document ids from the filesystem (filenames \
                      matching ^[0-9A-F]{8}$ under ~/.local/share/rstudio/sources/session-<ID>/), \
                      then fetch each document's metadata through the official `get_source_document` \
                      RPC. The on-disk surface is intentionally minimal: we only read filenames, \
                      not their contents. `contents` is stripped from the RPC response since this \
                      is a listing — use `editor context --include-contents` or `editor read` for \
                      the buffer/file body.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio editor list",
            explanation: "Returns every open document, ordered by tab order, with metadata only.",
        }],
        returns: "{documents: [{id, path, project_path, type, dirty, relative_order, source_on_save, last_known_write_time, encoding, read_only, ...}]}",
        errors: &[ErrorSpec {
            kind: "session_unavailable",
            when: "Cannot locate the sources directory (no session id).",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("get_source_document"),
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
    /// List every document currently open in the Source pane.
    List,
    /// Create a new untitled document.
    New {
        text: String,
        /// Document type.
        #[arg(long, default_value = "r")]
        r#type: String,
        /// Execute the text in the console after creation.
        #[arg(long)]
        execute: bool,
    },
    /// Return the id of the active document.
    ActiveId {
        /// Exclude the R console (return null if it's active).
        #[arg(long)]
        no_console: bool,
    },
    /// Return the path of a document by id (or the active one).
    Path {
        #[arg(long)]
        id: Option<String>,
    },
    /// Replace the entire contents of a document.
    SetContents {
        text: String,
        #[arg(long)]
        id: Option<String>,
    },
    /// Replace text within a range.
    ModifyRange {
        /// Range 'L1:C1-L2:C2'.
        range: String,
        text: String,
        #[arg(long)]
        id: Option<String>,
    },
    /// Move the cursor to a position.
    SetCursor {
        /// Position 'L:C'.
        position: String,
        #[arg(long)]
        id: Option<String>,
    },
}

pub fn run(
    cmd: &EditorCmd,
    rpc: &RpcClient<'_>,
    session: &Session,
) -> Result<Option<Value>, CliError> {
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
        EditorCmd::List => list_open(rpc, session),
        EditorCmd::New {
            text,
            r#type,
            execute,
        } => new_doc(rpc, text, r#type, *execute),
        EditorCmd::ActiveId { no_console } => active_id(rpc, *no_console),
        EditorCmd::Path { id } => path_of(rpc, id.as_deref()),
        EditorCmd::SetContents { text, id } => set_contents(rpc, text, id.as_deref()),
        EditorCmd::ModifyRange { range, text, id } => modify_range(rpc, range, text, id.as_deref()),
        EditorCmd::SetCursor { position, id } => set_cursor(rpc, position, id.as_deref()),
    }
}

fn open(
    rpc: &RpcClient<'_>,
    path: &Path,
    line: Option<u32>,
    col: Option<u32>,
    no_cursor: bool,
) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();

    let line_arg = line
        .map(|l| format!("{l}L"))
        .unwrap_or_else(|| "-1L".into());
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

fn edit_modal(rpc: &RpcClient<'_>, path: &Path) -> Result<Option<Value>, CliError> {
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

fn read(rpc: &RpcClient<'_>, path: &Path, encoding: &str) -> Result<Option<Value>, CliError> {
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
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!(
            "editor context ({api_fn}): invalid JSON: {e}; raw: {raw}"
        ))
    })?;
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

fn new_doc(
    rpc: &RpcClient<'_>,
    text: &str,
    doc_type: &str,
    execute: bool,
) -> Result<Option<Value>, CliError> {
    if !["r", "rmarkdown", "sql"].contains(&doc_type) {
        return Err(CliError::user(format!(
            "invalid --type '{doc_type}'. Expected: r, rmarkdown, sql."
        )));
    }
    let exec_arg = if execute { "TRUE" } else { "FALSE" };
    let r_code = format!(
        r#"local({{
  .__id <- rstudioapi::documentNew(text = {text_q}, type = {type_q}, execute = {exec_arg})
  cat(jsonlite::toJSON(list(id = .__id, type = {type_q}), auto_unbox = TRUE, null = "null"))
}})"#,
        text_q = r_quote(text),
        type_q = r_quote(doc_type),
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("editor new: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn active_id(rpc: &RpcClient<'_>, no_console: bool) -> Result<Option<Value>, CliError> {
    let allow_console = if no_console { "FALSE" } else { "TRUE" };
    let r_code = format!(
        r#"local({{
  .__id <- rstudioapi::documentId(allowConsole = {allow_console})
  if (is.null(.__id)) cat("{{\"id\":null}}")
  else cat(jsonlite::toJSON(list(id = .__id), auto_unbox = TRUE))
}})"#
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("editor active-id: invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

fn path_of(rpc: &RpcClient<'_>, id: Option<&str>) -> Result<Option<Value>, CliError> {
    let id_arg = match id {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    let r_code = format!(
        r#"local({{
  .__p <- rstudioapi::documentPath({id_arg})
  if (is.null(.__p)) cat("{{\"path\":null}}")
  else cat(jsonlite::toJSON(list(path = .__p), auto_unbox = TRUE))
}})"#
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("editor path: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn set_contents(
    rpc: &RpcClient<'_>,
    text: &str,
    id: Option<&str>,
) -> Result<Option<Value>, CliError> {
    let id_arg = match id {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    let r_code = format!(
        "rstudioapi::setDocumentContents(text = {}, id = {id_arg})",
        r_quote(text)
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn modify_range(
    rpc: &RpcClient<'_>,
    range: &str,
    text: &str,
    id: Option<&str>,
) -> Result<Option<Value>, CliError> {
    let ((l1, c1), (l2, c2)) = parse_range(range).ok_or_else(|| {
        CliError::user(format!("invalid range '{range}'. Expected 'L1:C1-L2:C2'."))
    })?;
    let id_arg = match id {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    let r_code = format!(
        "rstudioapi::modifyRange(\
           location = rstudioapi::document_range(\
             rstudioapi::document_position({l1}L, {c1}L), \
             rstudioapi::document_position({l2}L, {c2}L)), \
           text = {}, id = {id_arg})",
        r_quote(text)
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn set_cursor(
    rpc: &RpcClient<'_>,
    position: &str,
    id: Option<&str>,
) -> Result<Option<Value>, CliError> {
    let (line, col) = parse_line_col(position)
        .ok_or_else(|| CliError::user(format!("invalid position '{position}'. Expected 'L:C'.")))?;
    let id_arg = match id {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    let r_code = format!(
        "rstudioapi::setCursorPosition(\
           position = rstudioapi::document_position({line}L, {col}L), \
           id = {id_arg})"
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn list_open(rpc: &RpcClient<'_>, session: &Session) -> Result<Option<Value>, CliError> {
    let dir = session.require_sources_dir()?;
    let entries = fs::read_dir(dir).map_err(|e| {
        CliError::session(format!(
            "cannot read RStudio sources directory {}: {e}",
            dir.display()
        ))
    })?;

    // Step 1: enumerate document IDs by filename pattern only. We deliberately
    // do NOT read these files' contents — that on-disk format is internal to
    // RStudio and out of contract.
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_document_id(&name) {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    ids.sort();

    // Step 2: fetch each document's metadata through the official RPC. If a
    // doc was closed between the read_dir and the RPC call (race condition),
    // skip it silently rather than fail the whole listing.
    let mut docs: Vec<Value> = Vec::with_capacity(ids.len());
    for id in &ids {
        let result = rpc.rpc("get_source_document", vec![Value::String(id.clone())]);
        let mut entry = match result {
            Ok(Value::Object(map)) => map,
            Ok(_) | Err(_) => continue, // race or unexpected shape — skip
        };
        // Strip the body from the listing — `editor list` is metadata-only.
        // Use `editor context --include-contents` or `editor read` to retrieve it.
        entry.remove("contents");
        docs.push(Value::Object(entry));
    }

    // Step 3: order by RStudio's reported tab order so the listing matches
    // what the user sees in the Source pane.
    docs.sort_by_key(|d| {
        d.get("relative_order")
            .and_then(|v| v.as_i64())
            .unwrap_or(i64::MAX)
    });

    Ok(Some(json!({ "documents": docs })))
}

/// Match the source-database filename pattern: 8 uppercase hex characters.
/// Skips `<id>-contents` files (live buffer) and `lock_file`.
fn is_document_id(name: &str) -> bool {
    name.len() == 8
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
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
