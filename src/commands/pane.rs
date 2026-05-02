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
        name: "preview-rd",
        summary: "Preview a .Rd help file in the Help pane.",
        description: "Wraps rstudioapi::previewRd(rdFile). Renders the Rd to HTML and shows it.",
        params: &[ParamSpec {
            name: "path",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: ".Rd file path.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio pane preview-rd ~/projects/foo/man/foo.Rd",
            explanation: "Renders foo.Rd in the Help pane.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Rd file not found.",
        }],
        rstudioapi_fn: Some("previewRd"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pane",
        name: "preview-sql",
        summary: "Preview a SQL statement against a DBI connection in the Viewer.",
        description: "Wraps rstudioapi::previewSql(conn, statement). The connection is \
                      identified by --conn-expr, an R expression that resolves to a live \
                      DBI connection in the active environment (e.g. 'con', 'pool::poolCheckout(p)').",
        params: &[
            ParamSpec {
                name: "--conn-expr",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "R expression evaluating to a DBI connection.",
            },
            ParamSpec {
                name: "--sql",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "SQL statement to preview.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio pane preview-sql --conn-expr 'con' --sql 'select 1 as x'",
            explanation: "Show the result of the query in the Viewer pane.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "conn-expr does not resolve to a live DBI connection.",
        }],
        rstudioapi_fn: Some("previewSql"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pane",
        name: "save-plot",
        summary: "Save the currently displayed plot to an image file.",
        description: "Wraps rstudioapi::savePlotAsImage(file, format, width, height).",
        params: &[
            ParamSpec {
                name: "file",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Output image path.",
            },
            ParamSpec {
                name: "--image-format",
                kind: ParamKind::Enum,
                required: false,
                default: Some("png"),
                allowed: &["png", "jpeg", "bmp", "tiff", "emf", "svg", "eps"],
                description: "Image format (named --image-format to avoid colliding with the global --format).",
            },
            ParamSpec {
                name: "--width",
                kind: ParamKind::Integer,
                required: false,
                default: Some("800"),
                allowed: &[],
                description: "Width in pixels.",
            },
            ParamSpec {
                name: "--height",
                kind: ParamKind::Integer,
                required: false,
                default: Some("600"),
                allowed: &[],
                description: "Height in pixels.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio pane save-plot /tmp/last-plot.png --width 1200 --height 800",
            explanation: "Save the current Plots pane content as a 1200x800 PNG.",
        }],
        returns: "{file: string, format: string, width: int, height: int}",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "No active plot or write failure.",
        }],
        rstudioapi_fn: Some("savePlotAsImage"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pane",
        name: "highlight-ui",
        summary: "Highlight RStudio UI elements (a developer/debug helper).",
        description: "Wraps rstudioapi::highlightUi(queries). Pass a JSON array of \
                      query objects per the rstudioapi docs. Niche; useful for \
                      onboarding overlays / tutorials.",
        params: &[ParamSpec {
            name: "--queries-json",
            kind: ParamKind::Json,
            required: true,
            default: None,
            allowed: &[],
            description: "JSON array of UI query objects (forwarded as-is to rstudioapi).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio pane highlight-ui --queries-json '[{\"parent\":\"#rstudio_workbench_panel_console\"}]'",
            explanation: "Highlight the console panel.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Invalid JSON.",
        }],
        rstudioapi_fn: Some("highlightUi"),
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
    /// Preview a .Rd help file in the Help pane.
    PreviewRd { path: PathBuf },
    /// Preview a SQL statement against a DBI connection in the Viewer pane.
    PreviewSql {
        /// R expression resolving to a live DBI connection.
        #[arg(long)]
        conn_expr: String,
        /// SQL statement.
        #[arg(long)]
        sql: String,
    },
    /// Save the current plot to an image file.
    SavePlot {
        file: PathBuf,
        /// Image format (named --image-format to avoid colliding with the global --format).
        #[arg(long = "image-format", default_value = "png")]
        image_format: String,
        /// Width in pixels.
        #[arg(long, default_value_t = 800)]
        width: u32,
        /// Height in pixels.
        #[arg(long, default_value_t = 600)]
        height: u32,
    },
    /// Highlight RStudio UI elements (developer helper).
    HighlightUi {
        /// JSON array of query objects.
        #[arg(long)]
        queries_json: String,
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
        PaneCmd::PreviewRd { path } => preview_rd(rpc, path),
        PaneCmd::PreviewSql { conn_expr, sql } => preview_sql(rpc, conn_expr, sql),
        PaneCmd::SavePlot {
            file,
            image_format,
            width,
            height,
        } => save_plot(rpc, file, image_format, *width, *height),
        PaneCmd::HighlightUi { queries_json } => highlight_ui(rpc, queries_json),
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

fn preview_rd(rpc: &RpcClient<'_>, path: &PathBuf) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();
    let r = format!("rstudioapi::previewRd({})", r_quote(&abs_str));
    r_eval::run_silent(rpc, &r)?;
    Ok(Some(json!({ "path": abs_str })))
}

fn preview_sql(
    rpc: &RpcClient<'_>,
    conn_expr: &str,
    sql: &str,
) -> Result<Option<Value>, CliError> {
    let r = format!(
        "rstudioapi::previewSql(conn = ({}), statement = {})",
        conn_expr,
        r_quote(sql)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn save_plot(
    rpc: &RpcClient<'_>,
    file: &PathBuf,
    format: &str,
    width: u32,
    height: u32,
) -> Result<Option<Value>, CliError> {
    if !["png", "jpeg", "bmp", "tiff", "emf", "svg", "eps"].contains(&format) {
        return Err(CliError::user(format!(
            "invalid --format '{format}'. Expected: png, jpeg, bmp, tiff, emf, svg, eps."
        )));
    }
    // The output path may not exist yet; just absolutize relative paths.
    let abs = if file.is_absolute() {
        file.clone()
    } else {
        std::env::current_dir()
            .map_err(|e| CliError::internal(format!("getcwd: {e}")))?
            .join(file)
    };
    let abs_str = abs.to_string_lossy().into_owned();
    let r = format!(
        "rstudioapi::savePlotAsImage(file = {}, format = {}, width = {width}L, height = {height}L)",
        r_quote(&abs_str),
        r_quote(format),
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(Some(json!({
        "file": abs_str,
        "format": format,
        "width": width,
        "height": height,
    })))
}

fn highlight_ui(rpc: &RpcClient<'_>, queries_json: &str) -> Result<Option<Value>, CliError> {
    // Validate JSON CLI-side.
    let _: Value = serde_json::from_str(queries_json)
        .map_err(|e| CliError::user(format!("invalid --queries-json: {e}")))?;
    let r = format!(
        "rstudioapi::highlightUi(queries = jsonlite::fromJSON({}, simplifyDataFrame = FALSE))",
        r_quote(queries_json)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
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
