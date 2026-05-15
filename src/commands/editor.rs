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
        summary: "Open a file in the Source pane (non-modal). Returns the document id.",
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
        summary: "Open the modal R edit() dialog for the file (Save/Cancel). Blocking.",
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
        name: "read-buffer",
        summary: "Read the live editor buffer of an open document by id (includes unsaved edits).",
        description: "Wraps the get_source_document RPC and returns the buffer's current \
                      contents along with id, path and the dirty flag. Unlike `editor read` \
                      (which reads the on-disk file) and `editor context --include-contents` \
                      (which works only on the active document), this lets you read any open \
                      doc's buffer, active or not. Note: the dirty flag is read from the \
                      source_database snapshot which can lag the frontend by a fraction of a \
                      second after rapid edits.",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Document id (8 hex chars). Mutually exclusive with --path.",
            },
            ParamSpec {
                name: "--path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "File path; the id is resolved by listing open documents and \
                              matching against this path. Mutually exclusive with positional id.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor read-buffer D4F4972F",
                explanation: "Returns {id, path, contents, dirty} for that open document.",
            },
            ExampleSpec {
                cmd: "rstudio editor read-buffer --path /tmp/foo.R",
                explanation: "Resolve id from path; same return shape.",
            },
            ExampleSpec {
                cmd: "rstudio --format text editor read-buffer D4F4972F",
                explanation: "Pretty-prints the JSON; pipe through `jq -r .contents` for raw text.",
            },
        ],
        returns: "{id: string, path: string, contents: string, dirty: bool}",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Neither <id> nor --path was given, or --path doesn't match any open doc, \
                       or --path matches multiple open docs (ambiguous).",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("get_source_document"),
    },
    ActionSpec {
        category: "editor",
        name: "context",
        summary: "Context (id, path, selections) of a document in the Source pane or the console.",
        description: "Wraps rstudioapi::getSourceEditorContext() by default. Modifiers: \
                      --id <ID> targets a specific source document (active or not); \
                      --include-console targets whichever document currently has focus, \
                      including the R console (id = '#console'), via \
                      rstudioapi::getActiveDocumentContext(). --id and --include-console are \
                      mutually exclusive (the console doesn't accept an id). \
                      --include-contents adds the buffer lines to the response.",
        params: &[
            ParamSpec {
                name: "--id",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Document id (8 hex chars). When omitted, returns the active \
                              source document. Mutually exclusive with --include-console.",
            },
            ParamSpec {
                name: "--include-console",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Use getActiveDocumentContext() instead — returns whichever doc \
                              currently has focus, including the R console. Mutually exclusive \
                              with --id.",
            },
            ParamSpec {
                name: "--include-contents",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Include the live buffer contents (may be large).",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor context",
                explanation: "Returns the active source doc {id, path, selections}.",
            },
            ExampleSpec {
                cmd: "rstudio editor context --id D4F4972F",
                explanation: "Returns context for that specific source doc, even if not active.",
            },
            ExampleSpec {
                cmd: "rstudio editor context --include-console",
                explanation: "Returns whichever doc has focus — could be a file or the R console.",
            },
        ],
        returns: "{id, path, selections, contents?}",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Both --id and --include-console were passed.",
        }],
        rstudioapi_fn: Some("getSourceEditorContext / getActiveDocumentContext"),
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
        summary: "Close a document by id or path (Source pane).",
        description: "Wraps .rs.api.documentClose(id, save). Goes through the C call \
                      `rs_requestDocumentClose`, which actually unmounts the tab. Distinct \
                      from the close_document RPC, which only enqueues a UI event that may \
                      not be applied. --save controls what to do with unsaved changes: \
                      true = save silently, false = discard, ask = prompt the user (modal). \
                      Either <id> or --path is required (closing the active doc by accident \
                      could lose work).",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Document id. Mutually exclusive with --path.",
            },
            ParamSpec {
                name: "--path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "File path; the id is resolved by listing open documents and \
                              matching against this path. Mutually exclusive with positional id.",
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
                cmd: "rstudio editor close --path /tmp/foo.R --save false",
                explanation: "Resolve id from path, discard unsaved changes, close.",
            },
        ],
        returns: "{id: string, saved: 'true'|'false'|'ask'}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "Neither <id> nor --path was given, or --path matches 0 / multiple docs.",
            },
            ErrorSpec {
                kind: "r_error",
                when: "Unknown id or .rs.api.documentClose error.",
            },
        ],
        rstudioapi_fn: Some("documentClose"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "editor",
        name: "reload",
        summary: "Reload a document's buffer from disk (revert to saved). The id stays the same.",
        description: "Wraps the rsession revert_document RPC. The buffer is replaced with the \
                      current on-disk contents, dirty flag is cleared, and the document id is \
                      preserved (so cached references stay valid). Never opens a file that isn't \
                      already in the Source pane — passing --path for an unopened file is a \
                      silent no-op (action='skipped-not-open'). With --if-clean, dirty buffers \
                      are also no-op'd (action='skipped-dirty'); without it, dirty buffers are \
                      reverted and unsaved edits are lost.",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Document id (8 hex chars). Mutually exclusive with --path.",
            },
            ParamSpec {
                name: "--path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "File path; the id is resolved by listing open documents and \
                              matching against this path. Mutually exclusive with positional id.",
            },
            ParamSpec {
                name: "--if-clean",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "No-op if the buffer has unsaved changes (dirty=true).",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor reload D4F4972F",
                explanation: "Reload buffer D4F4972F from disk, overwriting any unsaved edits.",
            },
            ExampleSpec {
                cmd: "rstudio editor reload --path /path/to/foo.R --if-clean",
                explanation: "Resolve id from path; reload only if buffer has no unsaved edits. \
                              Safe to call after every external file write.",
            },
        ],
        returns: "{id: string|null, path: string|null, action: 'reverted'|'skipped-dirty'|'skipped-not-open'}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "Neither <id> nor --path was given, or --path matches multiple open docs.",
            },
            ErrorSpec {
                kind: "rpc_error",
                when: "revert_document RPC failed (e.g. on-disk file unreadable).",
            },
        ],
        rstudioapi_fn: None,
        rpc_method: Some("revert_document"),
    },
    ActionSpec {
        category: "editor",
        name: "save",
        summary: "Save a document by id, path, or active by default.",
        description: "Wraps .rs.api.documentSave(id). Without --id or --path, targets the \
                      active source document (excluding the console).",
        params: &[
            ParamSpec {
                name: "--id",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Document id. Mutually exclusive with --path.",
            },
            ParamSpec {
                name: "--path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "File path; resolved to id via the open-doc listing. Mutually \
                              exclusive with --id.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor save",
                explanation: "Save the active document.",
            },
            ExampleSpec {
                cmd: "rstudio editor save --path /tmp/foo.R",
                explanation: "Save the open doc whose path matches.",
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
                      is replaced; previous unsaved content is lost. Without --id or \
                      --path, targets the active document.",
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
                description: "Target document id. Mutually exclusive with --path.",
            },
            ParamSpec {
                name: "--path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Resolve target id from this path. Mutually exclusive with --id.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio editor set-contents 'x <- 1\\n' --path /tmp/foo.R",
            explanation: "Replace the full content of /tmp/foo.R's buffer.",
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
                description: "Target document id. Mutually exclusive with --path.",
            },
            ParamSpec {
                name: "--path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Resolve target id from this path. Mutually exclusive with --id.",
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
                      with no selection. Same as `editor select L:C` but more explicit. \
                      Without --id or --path, targets the active document.",
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
                description: "Target document id. Mutually exclusive with --path.",
            },
            ParamSpec {
                name: "--path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Resolve target id from this path. Mutually exclusive with --id.",
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
        summary: "Set the selection (or move the cursor) in a document.",
        description: "Wraps rstudioapi::setSelectionRanges(). Range format: 'L:C' (cursor only, \
                      no selection) or 'L1:C1-L2:C2' (selection range). Without --id, targets \
                      the active document.",
        params: &[
            ParamSpec {
                name: "range",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Range to select. 'L:C' or 'L1:C1-L2:C2'. 1-based.",
            },
            ParamSpec {
                name: "--id",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Document id; defaults to the active document.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor select 10:1",
                explanation: "Moves the cursor to line 10, column 1 in the active document.",
            },
            ExampleSpec {
                cmd: "rstudio editor select 5:1-7:80 --id D4F4972F",
                explanation: "Selects (5,1)-(7,80) in document D4F4972F (even if not active).",
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
    ActionSpec {
        category: "editor",
        name: "set-marks",
        summary: "Read grep-format lines from stdin and display them in the Markers pane.",
        description: "Reads lines in grep -n format (file:line:text or file:line:col:text, \
                      as produced by grep, ripgrep, ag, …) from stdin and sends the results \
                      to rstudioapi::sourceMarkers(). Lines that do not match the pattern are \
                      silently skipped. Pairs naturally with any search tool; the CLI adds \
                      only the RStudio UI integration.",
        params: &[
            ParamSpec {
                name: "--name",
                kind: ParamKind::String,
                required: false,
                default: Some("rstudio-cli"),
                allowed: &[],
                description: "Label shown in the Markers pane header.",
            },
            ParamSpec {
                name: "--type",
                kind: ParamKind::String,
                required: false,
                default: Some("info"),
                allowed: &["info", "warning", "error"],
                description: "Severity applied to every marker.",
            },
            ParamSpec {
                name: "--base-path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Base path for resolving relative file paths \
                              (passed to sourceMarkers basePath).",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "grep -rn 'TODO' . --include='*.R' | rstudio editor set-marks",
                explanation: "Show all TODOs in R files as info markers.",
            },
            ExampleSpec {
                cmd: "rg --vimgrep 'FIXME' src/ | rstudio editor set-marks --name 'FIXMEs' --type warning",
                explanation: "Show FIXME hits from ripgrep as warning markers.",
            },
        ],
        returns: "{total: int, name: string}",
        errors: &[],
        rstudioapi_fn: Some("sourceMarkers"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum EditorCmd {
    /// Open a file in the Source pane (non-modal). Returns the document id.
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
    /// Read the on-disk file contents (not the live editor buffer).
    Read {
        path: PathBuf,
        /// Encoding (default UTF-8).
        #[arg(long, default_value = "UTF-8")]
        encoding: String,
    },
    /// Read the live editor buffer of an open document by id or path (includes unsaved edits).
    ReadBuffer {
        /// Document id (8 hex chars). Mutually exclusive with --path.
        id: Option<String>,
        /// Resolve the id from this file path. Mutually exclusive with positional id.
        #[arg(long, conflicts_with = "id")]
        path: Option<PathBuf>,
    },
    /// Context of a document (Source pane or, with --include-console, anywhere).
    Context {
        /// Specific source doc id (defaults to the active one). Excludes --include-console.
        #[arg(long)]
        id: Option<String>,
        /// Use getActiveDocumentContext() — returns the focused doc, including console.
        /// Mutually exclusive with --id.
        #[arg(long = "include-console", conflicts_with = "id")]
        include_console: bool,
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
    /// Set the selection (or cursor) in a document (active by default).
    Select {
        /// Range: 'L:C' or 'L1:C1-L2:C2'.
        range: String,
        /// Document id; defaults to the active document.
        #[arg(long)]
        id: Option<String>,
    },
    /// Close a document by id or path (true close, via .rs.api.documentClose).
    Close {
        /// Document id (8 hex chars). Mutually exclusive with --path.
        id: Option<String>,
        /// Resolve the id from this file path. Mutually exclusive with the positional id.
        #[arg(long, conflicts_with = "id")]
        path: Option<PathBuf>,
        /// How to handle unsaved changes: true = save silently, false = discard, ask = prompt.
        #[arg(long, default_value = "true")]
        save: String,
    },
    /// Reload a document's buffer from disk (revert to saved). Id stays the same.
    Reload {
        /// Document id (8 hex chars). Mutually exclusive with --path.
        id: Option<String>,
        /// Resolve the id from this file path. Mutually exclusive with the positional id.
        #[arg(long, conflicts_with = "id")]
        path: Option<PathBuf>,
        /// No-op if the buffer has unsaved changes (dirty=true).
        #[arg(long = "if-clean")]
        if_clean: bool,
    },
    /// Save a document by id, path, or active by default.
    Save {
        #[arg(long)]
        id: Option<String>,
        /// Resolve target id from this path. Mutually exclusive with --id.
        #[arg(long, conflicts_with = "id")]
        path: Option<PathBuf>,
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
    /// Replace the entire contents of a document (active by default).
    SetContents {
        text: String,
        #[arg(long)]
        id: Option<String>,
        /// Resolve target id from this path. Mutually exclusive with --id.
        #[arg(long, conflicts_with = "id")]
        path: Option<PathBuf>,
    },
    /// Replace text within a range (active doc by default).
    ModifyRange {
        /// Range 'L1:C1-L2:C2'.
        range: String,
        text: String,
        #[arg(long)]
        id: Option<String>,
        /// Resolve target id from this path. Mutually exclusive with --id.
        #[arg(long, conflicts_with = "id")]
        path: Option<PathBuf>,
    },
    /// Move the cursor to a position (active doc by default).
    SetCursor {
        /// Position 'L:C'.
        position: String,
        #[arg(long)]
        id: Option<String>,
        /// Resolve target id from this path. Mutually exclusive with --id.
        #[arg(long, conflicts_with = "id")]
        path: Option<PathBuf>,
    },
    /// Read grep-format lines from stdin and show them in the Markers pane.
    SetMarks {
        /// Label shown in the Markers pane header.
        #[arg(long, default_value = "rstudio-cli")]
        name: String,
        /// Severity applied to every marker.
        #[arg(long, default_value = "info", value_parser = ["info", "warning", "error"])]
        r#type: String,
        /// Base path for resolving relative file paths.
        #[arg(long)]
        base_path: Option<String>,
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
        EditorCmd::Read { path, encoding } => read(rpc, path, encoding),
        EditorCmd::ReadBuffer { id, path } => {
            read_buffer(rpc, session, id.as_deref(), path.as_deref())
        }
        EditorCmd::Context {
            id,
            include_console,
            include_contents,
        } => context(rpc, id.as_deref(), *include_console, *include_contents),
        EditorCmd::Insert { text, at } => insert(rpc, text, at),
        EditorCmd::Select { range, id } => select(rpc, range, id.as_deref()),
        EditorCmd::Close { id, path, save } => {
            close(rpc, session, id.as_deref(), path.as_deref(), save)
        }
        EditorCmd::Reload { id, path, if_clean } => {
            reload(rpc, session, id.as_deref(), path.as_deref(), *if_clean)
        }
        EditorCmd::Save { id, path } => save(rpc, session, id.as_deref(), path.as_deref()),
        EditorCmd::SaveAll => save_all(rpc),
        EditorCmd::List => list_open(rpc, session),
        EditorCmd::New {
            text,
            r#type,
            execute,
        } => new_doc(rpc, text, r#type, *execute),
        EditorCmd::ActiveId { no_console } => active_id(rpc, *no_console),
        EditorCmd::Path { id } => path_of(rpc, id.as_deref()),
        EditorCmd::SetContents { text, id, path } => {
            set_contents(rpc, session, text, id.as_deref(), path.as_deref())
        }
        EditorCmd::ModifyRange {
            range,
            text,
            id,
            path,
        } => modify_range(rpc, session, range, text, id.as_deref(), path.as_deref()),
        EditorCmd::SetCursor { position, id, path } => {
            set_cursor(rpc, session, position, id.as_deref(), path.as_deref())
        }
        EditorCmd::SetMarks {
            name,
            r#type,
            base_path,
        } => set_marks(rpc, name, r#type, base_path.as_deref()),
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

    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    let r_code = format!(
        r#"cat(rstudiocli::editor_open({path}, line = {line_arg}, col = {col_arg}, move_cursor = {move_cursor})$id)"#,
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

fn read_buffer(
    rpc: &RpcClient<'_>,
    session: &Session,
    id: Option<&str>,
    path: Option<&Path>,
) -> Result<Option<Value>, CliError> {
    // Resolve to a concrete id, or fail with a clean message.
    let resolved_id = match (id, path) {
        (Some(id), _) => id.to_string(),
        (None, Some(p)) => match find_open_doc_by_path(rpc, session, p)? {
            Some(meta) => meta.id,
            None => {
                return Err(CliError::user(format!(
                    "no open document matches path {} (run `editor list` to see open docs)",
                    p.display()
                )));
            }
        },
        (None, None) => {
            return Err(CliError::user(
                "editor read-buffer requires either <id> or --path <path>.",
            ));
        }
    };

    // Use the rstudiocli::editor_get_contents wrapper rather than the raw
    // get_source_document RPC. The wrapper goes through
    // rstudioapi::getSourceEditorContext, which sees the live buffer
    // immediately after a setDocumentContents / modifyRange. The raw RPC,
    // by contrast, can return the pre-modification state for ~1 s
    // (observed in the Docker bridge against rocker/rstudio:4.5.2).
    let r_code = format!(
        "cat(jsonlite::toJSON(rstudiocli::editor_get_contents({}), auto_unbox = TRUE))",
        r_quote(&resolved_id),
    );
    let raw = match r_eval::run(rpc, &r_code) {
        Ok(s) => s,
        Err(e) => {
            // editor_get_contents stops with a recognisable message when
            // the id is unknown. Surface it as a user error.
            if e.message.contains("no open document with id") {
                return Err(CliError::user(format!(
                    "no open document with id {resolved_id} \
                     (run `editor list` to see open docs)"
                )));
            }
            return Err(e);
        }
    };
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("editor read-buffer: invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

/// Implementation of `editor context [--id] [--include-console] [--include-contents]`.
/// Dispatches to the appropriate rstudioapi getter:
/// - `--include-console`: getActiveDocumentContext() (focused doc, can be the console)
/// - `--id <ID>`         : getSourceEditorContext(id) (specific source doc)
/// - default             : getSourceEditorContext() (active source doc)
fn context(
    rpc: &RpcClient<'_>,
    id: Option<&str>,
    include_console: bool,
    include_contents: bool,
) -> Result<Option<Value>, CliError> {
    // clap's conflicts_with already enforces this; defensive double-check for direct callers.
    if id.is_some() && include_console {
        return Err(CliError::user(
            "editor context: --id and --include-console are mutually exclusive.",
        ));
    }

    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    let api_call = if include_console {
        "rstudiocli::editor_context(console = TRUE)".to_string()
    } else if let Some(id) = id {
        format!("rstudiocli::editor_context(id = {})", r_quote(id))
    } else {
        "rstudiocli::editor_context()".to_string()
    };

    let contents_field = if include_contents {
        "contents = paste(ctx$contents, collapse = \"\\n\"),"
    } else {
        ""
    };
    let r_code = format!(
        r#"local({{
  ctx <- {api_call}
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
        CliError::internal(format!("editor context: invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

fn insert(rpc: &RpcClient<'_>, text: &str, at: &str) -> Result<Option<Value>, CliError> {
    // `at` defines a `location` to pass to insertText. We keep the
    // call site inline because the location expressions ("end", "start",
    // "L:C") need R-side evaluation of getSourceEditorContext for the
    // "end" branch — the rstudiocli wrapper would lose this
    // expressiveness. Constructor helpers (`document_position`) stay on
    // rstudioapi:: since they're tiny zero-side-effect builders, not
    // endpoints that warrant a re-wrap.
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

fn select(rpc: &RpcClient<'_>, range: &str, id: Option<&str>) -> Result<Option<Value>, CliError> {
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
    let id_arg = match id {
        Some(id) => format!(", id = {}", r_quote(id)),
        None => String::new(),
    };
    // `document_range` is a constructor; `editor_select_range` is our
    // wrapper for the actual endpoint (setSelectionRanges).
    let r_code = format!("rstudiocli::editor_select_range(list({r_range}){id_arg})");
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn reload(
    rpc: &RpcClient<'_>,
    session: &Session,
    id: Option<&str>,
    path: Option<&Path>,
    if_clean: bool,
) -> Result<Option<Value>, CliError> {
    // Resolve target doc to (id, on-disk path, dirty flag), or None if no
    // matching open doc exists in the Source pane.
    let resolved = match (id, path) {
        (Some(id), _) => fetch_doc_meta(rpc, id)?,
        (None, Some(p)) => find_open_doc_by_path(rpc, session, p)?,
        (None, None) => {
            return Err(CliError::user(
                "editor reload requires either <id> or --path <path>.",
            ));
        }
    };

    let Some(meta) = resolved else {
        // Path/id didn't match any open document. Safe no-op.
        return Ok(Some(json!({
            "id": Value::Null,
            "path": path.map(|p| p.to_string_lossy().into_owned()),
            "action": "skipped-not-open",
        })));
    };

    if if_clean && meta.dirty {
        return Ok(Some(json!({
            "id": meta.id,
            "path": meta.path,
            "action": "skipped-dirty",
        })));
    }

    // Empty fileType keeps the existing one — see SessionSource.cpp:reopen().
    rpc.rpc(
        "revert_document",
        vec![Value::String(meta.id.clone()), Value::String(String::new())],
    )?;

    Ok(Some(json!({
        "id": meta.id,
        "path": meta.path,
        "action": "reverted",
    })))
}

#[derive(Debug)]
struct DocMeta {
    id: String,
    path: String,
    dirty: bool,
}

/// Heuristic for "the rsession couldn't find this doc id". rsession surfaces
/// it under several different error messages — we treat them all as "not open".
fn rpc_error_is_unknown_doc(err: &CliError) -> bool {
    let msg = err.message.as_str();
    msg.contains("not found")
        || msg.contains("Unknown")
        || msg.contains("No such file or directory")
}

/// Look up an open document's metadata by id. Returns None if the id is unknown.
fn fetch_doc_meta(rpc: &RpcClient<'_>, id: &str) -> Result<Option<DocMeta>, CliError> {
    match rpc.rpc("get_source_document", vec![Value::String(id.to_string())]) {
        Ok(Value::Object(map)) => Ok(Some(DocMeta {
            id: id.to_string(),
            path: map
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            dirty: map.get("dirty").and_then(|v| v.as_bool()).unwrap_or(false),
        })),
        Ok(_) => Ok(None),
        Err(e) if rpc_error_is_unknown_doc(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Find the single open document whose `path` matches `target` (canonicalized
/// where possible). Returns Ok(None) if no match, Err if more than one.
/// Common resolver for actions that accept either `<id>` (positional) or
/// `--path <PATH>` (flag). Returns Some(id) on a positive resolution,
/// None when both inputs are absent (caller decides whether that means
/// "default to active" or "error").
///
/// Explicit `<id>` is validated against the open-doc set (one RPC call),
/// so no action silently no-ops on a typoed id. The path resolver
/// (`find_open_doc_by_path`) already does its own lookup, so it's
/// inherently validated.
fn resolve_target_id(
    rpc: &RpcClient<'_>,
    session: &Session,
    id: Option<&str>,
    path: Option<&Path>,
) -> Result<Option<String>, CliError> {
    match (id, path) {
        (Some(id), _) => {
            // Validate the id is actually open in the Source pane. rstudioapi
            // wrappers (.rs.api.documentClose, documentSave, setDocumentContents)
            // silently no-op on unknown ids, which would mask agent typos.
            if fetch_doc_meta(rpc, id)?.is_none() {
                return Err(CliError::user(format!(
                    "no open document with id {id} (run `editor list` to see open docs)"
                )));
            }
            Ok(Some(id.to_string()))
        }
        (None, Some(p)) => match find_open_doc_by_path(rpc, session, p)? {
            Some(meta) => Ok(Some(meta.id)),
            None => Err(CliError::user(format!(
                "no open document matches path {} (run `editor list` to see open docs)",
                p.display()
            ))),
        },
        (None, None) => Ok(None),
    }
}

fn find_open_doc_by_path(
    rpc: &RpcClient<'_>,
    session: &Session,
    target: &Path,
) -> Result<Option<DocMeta>, CliError> {
    let target_canon = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());

    let dir = session.resolve_sources_dir()?;
    let entries = fs::read_dir(&dir).map_err(|e| {
        CliError::session(format!(
            "cannot read RStudio sources directory {}: {e}",
            dir.display()
        ))
    })?;

    let ids: Vec<String> = entries
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

    let mut matches: Vec<DocMeta> = Vec::new();
    for id in &ids {
        let Some(meta) = fetch_doc_meta(rpc, id)? else {
            continue;
        };
        if meta.path.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(&meta.path);
        let candidate_canon = candidate.canonicalize().unwrap_or(candidate);
        if candidate_canon == target_canon {
            matches.push(meta);
        }
    }

    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches.into_iter().next().unwrap())),
        _ => {
            let listing: Vec<String> = matches
                .iter()
                .map(|m| format!("  --id {} (path: {})", m.id, m.path))
                .collect();
            Err(CliError::user(format!(
                "{} open documents share the path {}:\n{}\n\
                 Pass --id explicitly to disambiguate.",
                matches.len(),
                target_canon.display(),
                listing.join("\n")
            )))
        }
    }
}

fn close(
    rpc: &RpcClient<'_>,
    session: &Session,
    id: Option<&str>,
    path: Option<&Path>,
    save: &str,
) -> Result<Option<Value>, CliError> {
    let resolved = resolve_target_id(rpc, session, id, path)?
        .ok_or_else(|| CliError::user("editor close requires either <id> or --path <path>."))?;
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
    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    let r_code = format!(
        "rstudiocli::editor_close(id = {}, save = {save_arg})",
        r_quote(&resolved)
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({
        "id": resolved,
        "saved": save,
    })))
}

fn save(
    rpc: &RpcClient<'_>,
    session: &Session,
    id: Option<&str>,
    path: Option<&Path>,
) -> Result<Option<Value>, CliError> {
    // None target → resolve to the active source doc id R-side, then save.
    // We can't rely on .rs.api.documentSave's return value: it gives TRUE
    // (logical), not the id. So we capture the resolved id ourselves and
    // return that to the caller.
    let resolved = resolve_target_id(rpc, session, id, path)?;
    let id_expr = match resolved {
        Some(s) => r_quote(&s),
        None => "rstudiocli::editor_active_id(allow_console = FALSE)$id".into(),
    };
    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    let r_code = format!(
        r#"local({{
  .__id <- {id_expr}
  if (is.null(.__id)) {{
    cat("null")
    return(invisible())
  }}
  rstudiocli::editor_save(id = .__id)
  cat(jsonlite::toJSON(list(id = .__id), auto_unbox = TRUE))
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
    r_eval::run_silent(rpc, "rstudiocli::editor_save_all()")?;
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
    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    let r_code = format!(
        r#"cat(jsonlite::toJSON(
            rstudiocli::editor_new(text = {text_q}, type = {type_q}, execute = {exec_arg}),
            auto_unbox = TRUE, null = "null"
        ))"#,
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
    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    let r_code = format!(
        r#"cat(jsonlite::toJSON(
            rstudiocli::editor_active_id(allow_console = {allow_console}),
            auto_unbox = TRUE, null = "null"
        ))"#
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
    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    let r_code = format!(
        r#"cat(jsonlite::toJSON(
            rstudiocli::editor_document_path(id = {id_arg}),
            auto_unbox = TRUE, null = "null"
        ))"#
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("editor path: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn set_contents(
    rpc: &RpcClient<'_>,
    session: &Session,
    text: &str,
    id: Option<&str>,
    path: Option<&Path>,
) -> Result<Option<Value>, CliError> {
    let resolved = resolve_target_id(rpc, session, id, path)?;
    let id_arg = match resolved {
        Some(s) => r_quote(&s),
        None => "NULL".into(),
    };
    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    let r_code = format!(
        "rstudiocli::editor_set_contents(text = {}, id = {id_arg})",
        r_quote(text)
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn modify_range(
    rpc: &RpcClient<'_>,
    session: &Session,
    range: &str,
    text: &str,
    id: Option<&str>,
    path: Option<&Path>,
) -> Result<Option<Value>, CliError> {
    let ((l1, c1), (l2, c2)) = parse_range(range).ok_or_else(|| {
        CliError::user(format!("invalid range '{range}'. Expected 'L1:C1-L2:C2'."))
    })?;
    let resolved = resolve_target_id(rpc, session, id, path)?;
    let id_arg = match resolved {
        Some(s) => r_quote(&s),
        None => "NULL".into(),
    };
    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    // `document_range`/`document_position` are zero-side-effect constructors,
    // not endpoints — left as `rstudioapi::*`.
    let r_code = format!(
        "rstudiocli::editor_modify_range(\
           range = rstudioapi::document_range(\
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
    session: &Session,
    position: &str,
    id: Option<&str>,
    path: Option<&Path>,
) -> Result<Option<Value>, CliError> {
    let (line, col) = parse_line_col(position)
        .ok_or_else(|| CliError::user(format!("invalid position '{position}'. Expected 'L:C'.")))?;
    let resolved = resolve_target_id(rpc, session, id, path)?;
    let id_arg = match resolved {
        Some(s) => r_quote(&s),
        None => "NULL".into(),
    };
    // Delegated to the rstudiocli R package: see `r-package/R/editor.R`.
    let r_code = format!(
        "rstudiocli::editor_set_cursor(\
           position = rstudioapi::document_position({line}L, {col}L), \
           id = {id_arg})"
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn list_open(rpc: &RpcClient<'_>, session: &Session) -> Result<Option<Value>, CliError> {
    let dir = session.resolve_sources_dir()?;
    let entries = fs::read_dir(&dir).map_err(|e| {
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
        // Use `editor read-buffer <id>` to retrieve a specific buffer's contents,
        // `editor context --include-contents` for the active doc, or
        // `editor read <path>` for the on-disk file.
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
pub(crate) fn is_document_id(name: &str) -> bool {
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

fn set_marks(
    rpc: &RpcClient<'_>,
    name: &str,
    marker_type: &str,
    base_path: Option<&str>,
) -> Result<Option<Value>, CliError> {
    use std::io::BufRead;

    struct Hit {
        file: String,
        line: u32,
        col: u32,
        text: String,
    }

    fn parse_line(s: &str) -> Option<Hit> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        // Split into at most 4 fields on ':'.
        // Handles grep -n  (file:line:text)
        //         grep -rn (file:line:text)
        //         rg --vimgrep (file:line:col:text)
        let mut it = s.splitn(4, ':');
        let file = it.next()?.to_string();
        let line: u32 = it.next()?.trim().parse().ok()?;
        let third = it.next()?;
        let fourth = it.next();
        let (col, text) = match (third.trim().parse::<u32>(), fourth) {
            (Ok(c), Some(t)) => (c, t.trim().to_string()),
            _ => (
                1,
                match fourth {
                    Some(f) => format!("{}:{}", third, f),
                    None => third.to_string(),
                },
            ),
        };
        Some(Hit {
            file,
            line,
            col,
            text,
        })
    }

    let stdin = std::io::stdin();
    let hits: Vec<Hit> = stdin
        .lock()
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| parse_line(&l))
        .collect();

    let total = hits.len();
    if total == 0 {
        return Ok(Some(json!({ "total": 0, "name": name })));
    }

    // Serialise hits to JSON, pass to R as a quoted string, parse with jsonlite.
    let hits_json: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({
                "type":    marker_type,
                "file":    h.file,
                "line":    h.line,
                "column":  h.col,
                "message": h.text,
            })
        })
        .collect();
    let hits_json_str = serde_json::to_string(&hits_json)
        .map_err(|e| CliError::internal(format!("set-marks: JSON serialise: {e}")))?;

    let name_r = r_quote(name);
    let hits_r = r_quote(&hits_json_str);
    let base_path_r = match base_path {
        Some(p) => r_quote(p),
        None => "NULL".to_string(),
    };

    // Delegated to the rstudiocli R package's pane_markers wrapper:
    // see `r-package/R/pane.R`. autoSelect = "first" matches editor
    // set-marks's "jump to the first hit" semantics.
    let r_code = format!(
        r#"local({{
  markers <- jsonlite::fromJSON({hits_r}, simplifyVector = FALSE)
  rstudiocli::pane_markers(
    name        = {name_r},
    markers     = markers,
    base_path   = {base_path_r},
    auto_select = "first"
  )
  cat(jsonlite::toJSON(list(total = length(markers), name = {name_r}), auto_unbox = TRUE))
}})"#
    );

    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("editor set-marks: invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}
