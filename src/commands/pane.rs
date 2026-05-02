use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "pane",
        name: "viewer",
        summary: "Show a local HTML file or URL in the Viewer pane.",
        description: "Wraps rstudioapi::viewer(url). Local paths are resolved \
                      to absolute via canonicalize before being passed to R.",
        params: &[ParamSpec {
            name: "target",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Local path or URL (http://, https://).",
        }],
        examples: &[
            ExampleSpec {
                cmd: "rstudio pane viewer ~/reports/coverage.html",
                explanation: "Opens the file in the Viewer pane.",
            },
            ExampleSpec {
                cmd: "rstudio pane viewer https://example.com",
                explanation: "Loads the remote page (subject to browser policy).",
            },
        ],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Local path not found.",
        }],
        rstudioapi_fn: Some("viewer"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pane",
        name: "files",
        summary: "Navigate the Files pane to a directory.",
        description: "Wraps rstudioapi::filesPaneNavigate(path).",
        params: &[ParamSpec {
            name: "path",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Target directory.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio pane files ~/projects/my-project",
            explanation: "Points the Files pane at this directory.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Path not found.",
        }],
        rstudioapi_fn: Some("filesPaneNavigate"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pane",
        name: "markers",
        summary: "Display a Markers pane (lint-style) with a list of issues.",
        description: "Wraps rstudioapi::sourceMarkers(name, markers, autoSelect). \
                      Markers are passed as a JSON array via --markers or stdin: \
                      [{type, file, line, column?, message}, ...]. \
                      type ∈ {error, warning, info, style, usage, box}.",
        params: &[
            ParamSpec {
                name: "--name",
                kind: ParamKind::String,
                required: false,
                default: Some("rstudio-cli"),
                allowed: &[],
                description: "Collection name shown as the Markers pane title.",
            },
            ParamSpec {
                name: "--markers",
                kind: ParamKind::Json,
                required: false,
                default: None,
                allowed: &[],
                description: "Inline JSON array. If absent, read from stdin.",
            },
            ParamSpec {
                name: "--auto-select",
                kind: ParamKind::Enum,
                required: false,
                default: Some("none"),
                allowed: &["none", "first", "error"],
                description: "Which marker to auto-activate after creation.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio pane markers --name 'lint' --markers '[{\"type\":\"warning\",\"file\":\"~/p/foo.R\",\"line\":12,\"message\":\"Unused var\"}]'",
                explanation: "Surfaces a warning on foo.R line 12.",
            },
            ExampleSpec {
                cmd: "cat markers.json | rstudio pane markers --name 'lint'",
                explanation: "Reads markers from stdin (a JSON array).",
            },
        ],
        returns: "{count: int, name: string}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "Invalid JSON or empty markers array.",
            },
            ErrorSpec {
                kind: "r_error",
                when: "Required field missing or invalid type.",
            },
        ],
        rstudioapi_fn: Some("sourceMarkers"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum PaneCmd {
    /// Show a local HTML file or URL in the Viewer pane.
    Viewer { target: String },
    /// Navigate the Files pane to a directory.
    Files { path: PathBuf },
    /// Display markers (lint-style) in the Markers pane.
    Markers {
        /// Collection name shown as the pane title.
        #[arg(long, default_value = "rstudio-cli")]
        name: String,
        /// Markers as inline JSON array (otherwise read from stdin).
        #[arg(long)]
        markers: Option<String>,
        /// Which marker to auto-activate after creation.
        #[arg(long, default_value = "none")]
        auto_select: String,
    },
}

pub fn run(cmd: &PaneCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        PaneCmd::Viewer { target } => viewer(rpc, target),
        PaneCmd::Files { path } => files(rpc, path),
        PaneCmd::Markers {
            name,
            markers,
            auto_select,
        } => markers_cmd(rpc, name, markers.as_deref(), auto_select),
    }
}

fn viewer(rpc: &RpcClient<'_>, target: &str) -> Result<Option<Value>, CliError> {
    let resolved = if target.starts_with("http://") || target.starts_with("https://") {
        target.to_string()
    } else {
        let p = std::path::Path::new(target)
            .canonicalize()
            .map_err(|e| CliError::user(format!("cannot resolve {target}: {e}")))?;
        p.to_string_lossy().into_owned()
    };
    let r_code = format!("rstudioapi::viewer({})", r_quote(&resolved));
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({ "target": resolved })))
}

fn files(rpc: &RpcClient<'_>, path: &PathBuf) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();
    let r_code = format!("rstudioapi::filesPaneNavigate({})", r_quote(&abs_str));
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({ "path": abs_str })))
}

fn markers_cmd(
    rpc: &RpcClient<'_>,
    name: &str,
    markers_inline: Option<&str>,
    auto_select: &str,
) -> Result<Option<Value>, CliError> {
    if !["none", "first", "error"].contains(&auto_select) {
        return Err(CliError::user(format!(
            "invalid --auto-select '{auto_select}'. Expected: none, first, error."
        )));
    }
    let json_str = match markers_inline {
        Some(s) => s.to_string(),
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| CliError::user(format!("read stdin: {e}")))?;
            if buf.trim().is_empty() {
                return Err(CliError::user(
                    "no markers given: pass --markers or pipe a JSON array on stdin",
                ));
            }
            buf
        }
    };

    let parsed: Value = serde_json::from_str(&json_str)
        .map_err(|e| CliError::user(format!("invalid markers JSON: {e}")))?;
    let arr = parsed
        .as_array()
        .ok_or_else(|| CliError::user("markers must be a JSON array"))?;
    if arr.is_empty() {
        return Err(CliError::user("markers array is empty"));
    }
    let count = arr.len();

    let r_code = format!(
        r#"local({{
  m <- jsonlite::fromJSON({json_q}, simplifyDataFrame = TRUE)
  if (!is.data.frame(m)) m <- as.data.frame(m, stringsAsFactors = FALSE)
  if (is.null(m$column)) m$column <- 1L
  rstudioapi::sourceMarkers(
    name = {name_q},
    markers = m,
    autoSelect = {auto_q}
  )
}})"#,
        json_q = r_quote(&json_str),
        name_q = r_quote(name),
        auto_q = r_quote(auto_select),
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({ "count": count, "name": name })))
}
