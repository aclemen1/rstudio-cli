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
#' Wraps [rstudioapi::filesPaneNavigate()].
#'
#' @param path Absolute directory path.
#' @return `NULL` invisibly. Side-effect only.
#' @export
pane_files_navigate <- function(path) {
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
