use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval::{self, EvalTimeout};
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
        name: "preview",
        summary: "Render and preview a Markdown / R Markdown / Quarto document in the Viewer pane.",
        description: "Auto-detects the format from the file extension: .md → markdown::mark_html(), \
                      .Rmd/.rmd → rmarkdown::render(), .qmd → system2(\"quarto\", \"render\"). \
                      The rendered HTML opens in the Viewer pane unless --no-view is supplied. \
                      Use the explicit preview-md / preview-rmd / preview-qmd sub-commands \
                      for additional control (e.g. --output-dir).",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Path to the document (.md, .Rmd, .rmd, or .qmd).",
            },
            ParamSpec {
                name: "--no-view",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Render but do not load the result in the Viewer pane.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio pane preview README.md",
                explanation: "Renders README.md to HTML and opens it in the Viewer pane.",
            },
            ExampleSpec {
                cmd: "rstudio pane preview report.Rmd",
                explanation: "Knits the R Markdown document and opens it in the Viewer pane.",
            },
            ExampleSpec {
                cmd: "rstudio pane preview slides.qmd --no-view",
                explanation: "Renders the Quarto document without opening the Viewer.",
            },
        ],
        returns: "{input: string, output: string, format: string, viewer_loaded: bool}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "File not found or extension not recognised (.md/.Rmd/.rmd/.qmd required).",
            },
            ErrorSpec {
                kind: "r_error",
                when: "Rendering failed (required R package not installed, or document has errors).",
            },
        ],
        rstudioapi_fn: Some("viewer"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pane",
        name: "preview-md",
        summary: "Render a Markdown file to HTML and open it in the Viewer pane.",
        description: "Renders via markdown::mark_html(). Requires the markdown R package \
                      (pre-installed with RStudio). The HTML lands in tempdir() by default; \
                      use --output-dir to redirect.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Path to the .md file.",
            },
            ParamSpec {
                name: "--output-dir",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Directory for the rendered HTML (default: system temp dir).",
            },
            ParamSpec {
                name: "--no-view",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Render but do not load the result in the Viewer pane.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio pane preview-md README.md",
                explanation: "Renders README.md to HTML and opens it in the Viewer pane.",
            },
            ExampleSpec {
                cmd: "rstudio pane preview-md docs/guide.md --output-dir /tmp",
                explanation: "Renders to /tmp/guide.html without opening it.",
            },
        ],
        returns: "{input: string, output: string, format: string, viewer_loaded: bool}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "File not found.",
            },
            ErrorSpec {
                kind: "r_error",
                when: "markdown package not installed or document has errors.",
            },
        ],
        rstudioapi_fn: Some("viewer"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pane",
        name: "preview-rmd",
        summary: "Knit an R Markdown file to HTML and open it in the Viewer pane.",
        description: "Renders via rmarkdown::render(output_format = \"html_document\"). \
                      Requires the rmarkdown R package (pre-installed with RStudio). \
                      The socket timeout is lifted; rendering time depends on the document.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Path to the .Rmd or .rmd file.",
            },
            ParamSpec {
                name: "--output-dir",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Directory for the rendered HTML (default: same directory as the source).",
            },
            ParamSpec {
                name: "--no-view",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Render but do not load the result in the Viewer pane.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio pane preview-rmd analysis.Rmd",
                explanation: "Knits analysis.Rmd and opens the HTML in the Viewer pane.",
            },
            ExampleSpec {
                cmd: "rstudio pane preview-rmd report.Rmd --output-dir /tmp",
                explanation: "Knits and saves the HTML to /tmp/report.html.",
            },
        ],
        returns: "{input: string, output: string, format: string, viewer_loaded: bool}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "File not found.",
            },
            ErrorSpec {
                kind: "r_error",
                when: "rmarkdown not installed or document knitting failed.",
            },
        ],
        rstudioapi_fn: Some("viewer"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pane",
        name: "preview-qmd",
        summary: "Render a Quarto document to HTML and open it in the Viewer pane.",
        description: "Renders via system2(\"quarto\", c(\"render\", path, \"--to\", \"html\")). \
                      Requires Quarto to be installed on the system PATH. Output lands next \
                      to the source file ({stem}.html). The socket timeout is lifted; \
                      rendering time depends on the document.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Path to the .qmd file.",
            },
            ParamSpec {
                name: "--no-view",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Render but do not load the result in the Viewer pane.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio pane preview-qmd slides.qmd",
                explanation: "Renders slides.qmd to HTML and opens it in the Viewer pane.",
            },
            ExampleSpec {
                cmd: "rstudio pane preview-qmd report.qmd --no-view",
                explanation: "Renders report.qmd without opening the Viewer.",
            },
        ],
        returns: "{input: string, output: string, format: string, viewer_loaded: bool}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "File not found.",
            },
            ErrorSpec {
                kind: "r_error",
                when: "Quarto not on PATH or document rendering failed.",
            },
        ],
        rstudioapi_fn: Some("viewer"),
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
    /// Render and preview a document (auto-detect .md/.Rmd/.qmd) in the Viewer pane.
    Preview {
        path: PathBuf,
        /// Render but do not open the Viewer pane.
        #[arg(long)]
        no_view: bool,
    },
    /// Render a Markdown (.md) file to HTML and open it in the Viewer pane.
    PreviewMd {
        path: PathBuf,
        /// Output directory for the rendered HTML (default: system temp dir).
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Render but do not open the Viewer pane.
        #[arg(long)]
        no_view: bool,
    },
    /// Knit an R Markdown (.Rmd) file to HTML and open it in the Viewer pane.
    PreviewRmd {
        path: PathBuf,
        /// Output directory for the rendered HTML (default: same directory as source).
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Render but do not open the Viewer pane.
        #[arg(long)]
        no_view: bool,
    },
    /// Render a Quarto (.qmd) file to HTML and open it in the Viewer pane.
    PreviewQmd {
        path: PathBuf,
        /// Render but do not open the Viewer pane.
        #[arg(long)]
        no_view: bool,
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
        PaneCmd::Preview { path, no_view } => preview(rpc, path, *no_view),
        PaneCmd::PreviewMd {
            path,
            output_dir,
            no_view,
        } => preview_md(rpc, path, output_dir.as_deref(), *no_view),
        PaneCmd::PreviewRmd {
            path,
            output_dir,
            no_view,
        } => preview_rmd(rpc, path, output_dir.as_deref(), *no_view),
        PaneCmd::PreviewQmd { path, no_view } => preview_qmd(rpc, path, *no_view),
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
    // Delegated to the rstudiocli R package: see `r-package/R/pane.R`.
    let r_code = format!("rstudiocli::pane_viewer({})", r_quote(&resolved));
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({ "target": resolved })))
}

fn files(rpc: &RpcClient<'_>, path: &Path) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();
    // Delegated to the rstudiocli R package: see `r-package/R/pane.R`.
    let r_code = format!("rstudiocli::pane_files({})", r_quote(&abs_str));
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({ "path": abs_str })))
}

fn preview_rd(rpc: &RpcClient<'_>, path: &Path) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();
    // Delegated to the rstudiocli R package: see `r-package/R/pane.R`.
    let r = format!("rstudiocli::pane_preview_rd({})", r_quote(&abs_str));
    r_eval::run_silent(rpc, &r)?;
    Ok(Some(json!({ "path": abs_str })))
}

fn preview_sql(rpc: &RpcClient<'_>, conn_expr: &str, sql: &str) -> Result<Option<Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/pane.R`.
    // The `conn` argument is an R expression (e.g. `con`, `pool::poolCheckout(p)`),
    // evaluated lazily by R in the active env — pass it inline, don't quote.
    let r = format!(
        "rstudiocli::pane_preview_sql(conn = ({}), statement = {})",
        conn_expr,
        r_quote(sql)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

#[derive(Debug, Clone, Copy)]
enum DocFormat {
    Md,
    Rmd,
    Qmd,
}

fn detect_format(path: &Path) -> Result<DocFormat, CliError> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("md") => Ok(DocFormat::Md),
        Some("Rmd") | Some("rmd") => Ok(DocFormat::Rmd),
        Some("qmd") => Ok(DocFormat::Qmd),
        other => Err(CliError::user(format!(
            "unrecognised extension '{}': expected .md, .Rmd, .rmd, or .qmd",
            other.unwrap_or("<none>")
        ))),
    }
}

fn preview(rpc: &RpcClient<'_>, path: &Path, no_view: bool) -> Result<Option<Value>, CliError> {
    match detect_format(path)? {
        DocFormat::Md => preview_md(rpc, path, None, no_view),
        DocFormat::Rmd => preview_rmd(rpc, path, None, no_view),
        DocFormat::Qmd => preview_qmd(rpc, path, no_view),
    }
}

fn preview_md(
    rpc: &RpcClient<'_>,
    path: &Path,
    output_dir: Option<&Path>,
    no_view: bool,
) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();

    let stem = abs
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let out_dir = match output_dir {
        Some(d) => d.canonicalize().map_err(|e| {
            CliError::user(format!("cannot resolve --output-dir {}: {e}", d.display()))
        })?,
        None => std::env::temp_dir(),
    };
    let out_path = out_dir.join(format!("{stem}.html"));
    let out_str = out_path.to_string_lossy().into_owned();

    let viewer_call = if no_view {
        String::new()
    } else {
        format!("rstudiocli::pane_viewer({})\n  ", r_quote(&out_str))
    };

    // mark_html() was introduced in markdown >= 1.0 (API rewrite).
    // Older installations only export markdownToHTML().
    let r_code = format!(
        r#"local({{
  f <- normalizePath({path_r}, mustWork = TRUE)
  if (utils::packageVersion("markdown") >= "1.0") {{
    markdown::mark_html(f, output = {out_r})
  }} else {{
    markdown::markdownToHTML(f, output = {out_r})
  }}
  {viewer}invisible(NULL)
}})"#,
        path_r = r_quote(&abs_str),
        out_r = r_quote(&out_str),
        viewer = viewer_call,
    );

    rpc.set_timeout(None);
    r_eval::run_with_timeout(rpc, &r_code, EvalTimeout::NoLimit)?;

    Ok(Some(json!({
        "input": abs_str,
        "output": out_str,
        "format": "html",
        "viewer_loaded": !no_view,
    })))
}

fn preview_rmd(
    rpc: &RpcClient<'_>,
    path: &Path,
    output_dir: Option<&Path>,
    no_view: bool,
) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();

    let stem = abs
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let out_html = format!("{stem}.html");

    let (out_dir_path, out_dir_r) = match output_dir {
        Some(d) => {
            let d_abs = d.canonicalize().map_err(|e| {
                CliError::user(format!("cannot resolve --output-dir {}: {e}", d.display()))
            })?;
            let r = r_quote(&d_abs.to_string_lossy());
            (d_abs, r)
        }
        None => {
            let parent = abs.parent().unwrap_or(abs.as_ref()).to_path_buf();
            (parent, "NULL".to_string())
        }
    };

    let out_path = out_dir_path.join(&out_html);
    let out_str = out_path.to_string_lossy().into_owned();

    let viewer_call = if no_view {
        String::new()
    } else {
        format!("rstudiocli::pane_viewer({})\n  ", r_quote(&out_str))
    };

    let r_code = format!(
        r#"local({{
  f <- normalizePath({path_r}, mustWork = TRUE)
  rmarkdown::render(f,
    output_format = "html_document",
    output_file = {file_r},
    output_dir = {dir_r},
    quiet = TRUE)
  {viewer}invisible(NULL)
}})"#,
        path_r = r_quote(&abs_str),
        file_r = r_quote(&out_html),
        dir_r = out_dir_r,
        viewer = viewer_call,
    );

    rpc.set_timeout(None);
    r_eval::run_with_timeout(rpc, &r_code, EvalTimeout::NoLimit)?;

    Ok(Some(json!({
        "input": abs_str,
        "output": out_str,
        "format": "html",
        "viewer_loaded": !no_view,
    })))
}

fn preview_qmd(rpc: &RpcClient<'_>, path: &Path, no_view: bool) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();

    let stem = abs
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let out_path = abs
        .parent()
        .unwrap_or(abs.as_ref())
        .join(format!("{stem}.html"));
    let out_str = out_path.to_string_lossy().into_owned();

    let viewer_call = if no_view {
        String::new()
    } else {
        format!("rstudiocli::pane_viewer({})\n  ", r_quote(&out_str))
    };

    let r_code = format!(
        r#"local({{
  f <- normalizePath({path_r}, mustWork = TRUE)
  err_file <- tempfile()
  rc <- system2("quarto", c("render", f, "--to", "html"),
                stdout = FALSE, stderr = err_file)
  if (rc != 0) {{
    msg <- paste(readLines(err_file, warn = FALSE), collapse = "\n")
    stop(paste("quarto render failed:", msg))
  }}
  {viewer}invisible(NULL)
}})"#,
        path_r = r_quote(&abs_str),
        viewer = viewer_call,
    );

    rpc.set_timeout(None);
    r_eval::run_with_timeout(rpc, &r_code, EvalTimeout::NoLimit)?;

    Ok(Some(json!({
        "input": abs_str,
        "output": out_str,
        "format": "html",
        "viewer_loaded": !no_view,
    })))
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
    let abs: PathBuf = if file.is_absolute() {
        file.clone()
    } else {
        std::env::current_dir()
            .map_err(|e| CliError::internal(format!("getcwd: {e}")))?
            .join(file)
    };
    let abs_str = abs.to_string_lossy().into_owned();
    // Delegated to the rstudiocli R package: see `r-package/R/pane.R`.
    let r = format!(
        "rstudiocli::pane_save_plot(file = {}, format = {}, width = {width}L, height = {height}L)",
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
    // Delegated to the rstudiocli R package: see `r-package/R/pane.R`.
    let r = format!(
        "rstudiocli::pane_highlight_ui(queries = jsonlite::fromJSON({}, simplifyDataFrame = FALSE))",
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
    let (r_code, count) = build_markers_r_code(name, &json_str, auto_select)?;
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({ "count": count, "name": name })))
}

/// Validate markers JSON and build the R code string for `sourceMarkers`.
/// Extracted for unit-testability (the caller owns stdin/RPC plumbing).
fn build_markers_r_code(
    name: &str,
    json_str: &str,
    auto_select: &str,
) -> Result<(String, usize), CliError> {
    if !["none", "first", "error"].contains(&auto_select) {
        return Err(CliError::user(format!(
            "invalid --auto-select '{auto_select}'. Expected: none, first, error."
        )));
    }
    let parsed: Value = serde_json::from_str(json_str)
        .map_err(|e| CliError::user(format!("invalid markers JSON: {e}")))?;
    let arr = parsed
        .as_array()
        .ok_or_else(|| CliError::user("markers must be a JSON array"))?;
    if arr.is_empty() {
        return Err(CliError::user("markers array is empty"));
    }
    let count = arr.len();
    // Delegated to the rstudiocli R package: see `r-package/R/pane.R`.
    // The R wrapper normalises line/column integers and forwards to
    // rstudioapi::sourceMarkers. We pass the parsed-and-coerced data.frame
    // form because sourceMarkers accepts both list-of-lists and data.frame,
    // and the data.frame route is what the previous inline R built.
    let r_code = format!(
        r#"local({{
  m <- jsonlite::fromJSON({json_q}, simplifyDataFrame = TRUE)
  if (!is.data.frame(m)) m <- as.data.frame(m, stringsAsFactors = FALSE)
  rstudiocli::pane_markers(
    name = {name_q},
    markers = m,
    auto_select = {auto_q}
  )
}})"#,
        json_q = r_quote(json_str),
        name_q = r_quote(name),
        auto_q = r_quote(auto_select),
    );
    Ok((r_code, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detect_format_md() {
        assert!(matches!(
            detect_format(Path::new("README.md")),
            Ok(DocFormat::Md)
        ));
    }

    #[test]
    fn detect_format_rmd_uppercase() {
        assert!(matches!(
            detect_format(Path::new("report.Rmd")),
            Ok(DocFormat::Rmd)
        ));
    }

    #[test]
    fn detect_format_rmd_lowercase() {
        assert!(matches!(
            detect_format(Path::new("report.rmd")),
            Ok(DocFormat::Rmd)
        ));
    }

    #[test]
    fn detect_format_qmd() {
        assert!(matches!(
            detect_format(Path::new("slides.qmd")),
            Ok(DocFormat::Qmd)
        ));
    }

    #[test]
    fn detect_format_unknown_extension() {
        let err = detect_format(Path::new("script.R")).unwrap_err();
        assert!(err.message.contains("unrecognised extension"));
        assert!(err.message.contains(".R"));
    }

    #[test]
    fn detect_format_no_extension() {
        let err = detect_format(Path::new("Makefile")).unwrap_err();
        assert!(err.message.contains("unrecognised extension"));
        assert!(err.message.contains("<none>"));
    }

    #[test]
    fn detect_format_deep_path() {
        assert!(matches!(
            detect_format(Path::new("/home/user/projects/analysis/report.Rmd")),
            Ok(DocFormat::Rmd)
        ));
    }

    #[test]
    fn preview_md_output_path_in_tempdir() {
        // Verify that the computed output path uses the file stem + .html in tempdir.
        let stem = PathBuf::from("README.md")
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let out = std::env::temp_dir().join(format!("{stem}.html"));
        assert!(out.to_string_lossy().ends_with("README.html"));
    }

    #[test]
    fn preview_rmd_output_path_respects_output_dir() {
        let path = Path::new("/home/user/analysis.Rmd");
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let out_dir = PathBuf::from("/tmp");
        let out = out_dir.join(format!("{stem}.html"));
        assert_eq!(out, PathBuf::from("/tmp/analysis.html"));
    }

    #[test]
    fn preview_qmd_output_path_next_to_source() {
        let abs = PathBuf::from("/home/user/slides.qmd");
        let stem = abs.file_stem().unwrap().to_string_lossy().into_owned();
        let out = abs.parent().unwrap().join(format!("{stem}.html"));
        assert_eq!(out, PathBuf::from("/home/user/slides.html"));
    }

    // build_markers_r_code tests — no RPC required.

    #[test]
    fn markers_rejects_invalid_json() {
        let err = build_markers_r_code("lint", "not-json", "none").unwrap_err();
        assert!(err.message.contains("invalid markers JSON"));
    }

    #[test]
    fn markers_rejects_json_object_not_array() {
        let err = build_markers_r_code("lint", r#"{"type":"error"}"#, "none").unwrap_err();
        assert!(err.message.contains("JSON array"));
    }

    #[test]
    fn markers_rejects_empty_array() {
        let err = build_markers_r_code("lint", "[]", "none").unwrap_err();
        assert!(err.message.contains("empty"));
    }

    #[test]
    fn markers_rejects_invalid_auto_select() {
        let json = r#"[{"type":"warning","file":"x.R","line":1,"message":"x"}]"#;
        let err = build_markers_r_code("lint", json, "bad").unwrap_err();
        assert!(err.message.contains("auto-select"));
        assert!(err.message.contains("bad"));
    }

    #[test]
    fn markers_count_matches_array_length() {
        let json = r#"[
            {"type":"error","file":"a.R","line":1,"message":"e1"},
            {"type":"warning","file":"b.R","line":2,"message":"w1"}
        ]"#;
        let (_, count) = build_markers_r_code("lint", json, "none").unwrap();
        assert_eq!(count, 2);
    }

    // Fix #4 (historical): line/column must be coerced to R integers before
    // sourceMarkers receives them. The coercion has since moved into the
    // `rstudiocli::pane_markers()` R wrapper, so we now just check
    // that the generated R code delegates to that wrapper.
    #[test]
    fn markers_r_code_delegates_to_rstudiocli_mcp() {
        let json = r#"[{"type":"error","file":"a.R","line":5,"column":3,"message":"e"}]"#;
        let (r_code, _) = build_markers_r_code("lint", json, "none").unwrap();
        assert!(
            r_code.contains("rstudiocli::pane_markers"),
            "R code must call the rstudiocli wrapper; got:\n{r_code}"
        );
    }
}
