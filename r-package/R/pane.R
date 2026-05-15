#' Display a URL/path in the Viewer pane
#'
#' Thin wrapper around [rstudioapi::viewer()] for the common case of
#' pointing the Viewer pane at a local HTML file or a URL.
#'
#' @param target File path or URL to render.
#' @return `NULL` invisibly. Side-effect only.
#' @export
pane_viewer <- function(target) {
  if (!is.character(target) || length(target) != 1L || !nzchar(target)) {
    stop("`target` must be a non-empty length-1 character vector", call. = FALSE)
  }
  rstudioapi::viewer(target)
  .throttle()
  invisible(NULL)
}

#' Navigate the Files pane to a directory
#'
#' Wraps [rstudioapi::filesPaneNavigate()]. Mirrors the MCP / CLI
#' surface `pane.files`.
#'
#' @param path Absolute directory path.
#' @return `NULL` invisibly. Side-effect only.
#' @export
pane_files <- function(path) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  rstudioapi::filesPaneNavigate(path)
  .throttle()
  invisible(NULL)
}

#' Render an .Rd file in the Help pane
#'
#' Wraps [rstudioapi::previewRd()].
#'
#' @param path Absolute path to an `.Rd` file.
#' @return `NULL` invisibly. Side-effect only.
#' @export
pane_preview_rd <- function(path) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  rstudioapi::previewRd(path)
  .throttle()
  invisible(NULL)
}

#' Preview a SQL statement against a DBI connection
#'
#' Wraps [rstudioapi::previewSql()]. The connection is passed as an
#' R expression evaluated in the active environment by the caller —
#' typically `con` or a `pool::poolCheckout(p)` form.
#'
#' @param conn A live DBI connection object.
#' @param statement SQL statement (character).
#' @return `NULL` invisibly. Side-effect only.
#' @export
pane_preview_sql <- function(conn, statement) {
  if (!is.character(statement) || length(statement) != 1L) {
    stop("`statement` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::previewSql(conn = conn, statement = statement)
  .throttle()
  invisible(NULL)
}

#' Save the current Plots-pane plot as an image
#'
#' Wraps [rstudioapi::savePlotAsImage()]. Format is inferred from
#' the file extension if not given (RStudio handles png/jpeg/svg/pdf).
#'
#' @param file Output file path.
#' @param format Image format (`"png"`, `"jpeg"`, `"svg"`, `"pdf"`).
#' @param width,height Output dimensions in pixels.
#' @return `NULL` invisibly. Side-effect only.
#' @export
pane_save_plot <- function(file, format, width, height) {
  if (!is.character(file) || length(file) != 1L || !nzchar(file)) {
    stop("`file` must be a non-empty length-1 character vector", call. = FALSE)
  }
  rstudioapi::savePlotAsImage(
    file = file,
    format = format,
    width = as.integer(width),
    height = as.integer(height)
  )
  .throttle()
  invisible(NULL)
}

#' Drive RStudio's UI highlight overlay
#'
#' Wraps [rstudioapi::highlightUi()]. Useful for onboarding overlays
#' and interactive tutorials.
#'
#' @param queries A list/data.frame of query objects per the rstudioapi
#'   documentation (typically each element has `query`, `element`,
#'   `parent`).
#' @return `NULL` invisibly. Side-effect only.
#' @export
pane_highlight_ui <- function(queries) {
  rstudioapi::highlightUi(queries = queries)
  .throttle()
  invisible(NULL)
}

#' Drop a set of markers in the Markers pane
#'
#' Wraps [rstudioapi::sourceMarkers()]. Each marker is a list with
#' `type` (one of `error`, `warning`, `info`, `style`, `usage`, `box`),
#' `file`, `line`, `message`, and optionally `column`.
#'
#' Markers are typically lint-style feedback an agent or tool wants
#' surfaced to the user in the IDE.
#'
#' @param name Logical group name shown in the Markers pane.
#' @param markers List of marker objects (as above).
#' @param base_path Optional base path; relative `file` entries are
#'   resolved against it.
#' @param auto_select Whether to auto-jump to the first marker.
#'   `"none"` (default), `"first"`, or `"error"`.
#' @return `NULL` invisibly. Side-effect only.
#' @export
pane_markers <- function(name, markers, base_path = NULL, auto_select = "none") {
  if (!is.character(name) || length(name) != 1L) {
    stop("`name` must be a length-1 character vector", call. = FALSE)
  }
  # Normalise each marker: ensure line/column are integers. Accept both
  # a data.frame and a list-of-lists shape — that's what rstudioapi itself
  # supports, and our CLI hands us data.frame via jsonlite::fromJSON.
  if (is.data.frame(markers)) {
    markers$line <- as.integer(markers$line)
    markers$column <- if (is.null(markers$column)) {
      rep(1L, nrow(markers))
    } else {
      as.integer(markers$column)
    }
  } else {
    markers <- lapply(markers, function(m) {
      m$line <- as.integer(m$line)
      m$column <- if (is.null(m$column)) 1L else as.integer(m$column)
      m
    })
  }
  rstudioapi::sourceMarkers(
    name = name,
    markers = markers,
    basePath = base_path,
    autoSelect = auto_select
  )
  .throttle()
  invisible(NULL)
}

#' Render a Markdown / R Markdown / Quarto document and preview it
#'
#' Mirrors the MCP / CLI surface `pane.preview`. Auto-detects the
#' format from the file extension (`.md` / `.Rmd` / `.qmd`) and delegates
#' to [pane_preview_md()] / [pane_preview_rmd()] / [pane_preview_qmd()].
#'
#' @param path Path to the source document.
#' @param no_view If `TRUE`, render but skip [pane_viewer()] (the html
#'   file is still produced).
#' @return A list with `input`, `output`, `format`, `viewer_loaded`.
#' @export
pane_preview <- function(path, no_view = FALSE) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  ext <- tolower(tools::file_ext(path))
  switch(
    ext,
    md = pane_preview_md(path = path, no_view = no_view),
    rmd = pane_preview_rmd(path = path, no_view = no_view),
    qmd = pane_preview_qmd(path = path, no_view = no_view),
    stop(sprintf("unsupported extension '.%s' (expected .md / .Rmd / .qmd)", ext),
         call. = FALSE)
  )
}

#' Render a Markdown file to HTML and preview it in the Viewer pane
#'
#' Mirrors the MCP / CLI surface `pane.preview-md`. Uses the `markdown`
#' package (`mark_html()` on >= 1.0, `markdownToHTML()` otherwise).
#'
#' @param path Path to the `.md` source.
#' @param output_dir Optional output directory. Default: `tempdir()`.
#' @param no_view If `TRUE`, render but skip [pane_viewer()].
#' @return A list with `input`, `output`, `format = "html"`, `viewer_loaded`.
#' @export
pane_preview_md <- function(path, output_dir = NULL, no_view = FALSE) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  abs_path <- normalizePath(path, mustWork = TRUE)
  out_dir <- if (is.null(output_dir)) tempdir() else normalizePath(output_dir, mustWork = TRUE)
  stem <- tools::file_path_sans_ext(basename(abs_path))
  out_path <- file.path(out_dir, paste0(stem, ".html"))
  if (utils::packageVersion("markdown") >= "1.0") {
    markdown::mark_html(abs_path, output = out_path)
  } else {
    markdown::markdownToHTML(abs_path, output = out_path)
  }
  if (!isTRUE(no_view)) pane_viewer(out_path)
  list(
    input = abs_path,
    output = out_path,
    format = "html",
    viewer_loaded = !isTRUE(no_view)
  )
}

#' Knit an R Markdown file to HTML and preview it in the Viewer pane
#'
#' Mirrors the MCP / CLI surface `pane.preview-rmd`. Wraps
#' [rmarkdown::render()] with `output_format = "html_document"`.
#'
#' @param path Path to the `.Rmd` source.
#' @param output_dir Optional output directory. Default: same directory
#'   as the source.
#' @param no_view If `TRUE`, render but skip [pane_viewer()].
#' @return A list with `input`, `output`, `format = "html"`, `viewer_loaded`.
#' @export
pane_preview_rmd <- function(path, output_dir = NULL, no_view = FALSE) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  if (!requireNamespace("rmarkdown", quietly = TRUE)) {
    stop("pane_preview_rmd: package 'rmarkdown' is required", call. = FALSE)
  }
  abs_path <- normalizePath(path, mustWork = TRUE)
  out_dir <- if (is.null(output_dir)) dirname(abs_path) else normalizePath(output_dir, mustWork = TRUE)
  out_path <- rmarkdown::render(
    input = abs_path,
    output_format = "html_document",
    output_dir = out_dir,
    quiet = TRUE
  )
  if (!isTRUE(no_view)) pane_viewer(out_path)
  list(
    input = abs_path,
    output = out_path,
    format = "html",
    viewer_loaded = !isTRUE(no_view)
  )
}

#' Render a Quarto file to HTML and preview it in the Viewer pane
#'
#' Mirrors the MCP / CLI surface `pane.preview-qmd`. Uses the `quarto`
#' package's [quarto::quarto_render()].
#'
#' @param path Path to the `.qmd` source.
#' @param no_view If `TRUE`, render but skip [pane_viewer()].
#' @return A list with `input`, `output`, `format = "html"`, `viewer_loaded`.
#' @export
pane_preview_qmd <- function(path, no_view = FALSE) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  if (!requireNamespace("quarto", quietly = TRUE)) {
    stop("pane_preview_qmd: package 'quarto' is required", call. = FALSE)
  }
  abs_path <- normalizePath(path, mustWork = TRUE)
  quarto::quarto_render(input = abs_path, quiet = TRUE)
  out_path <- sub("\\.qmd$", ".html", abs_path, ignore.case = TRUE)
  if (!isTRUE(no_view) && file.exists(out_path)) pane_viewer(out_path)
  list(
    input = abs_path,
    output = out_path,
    format = "html",
    viewer_loaded = !isTRUE(no_view) && file.exists(out_path)
  )
}
