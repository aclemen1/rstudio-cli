#' Replace the contents of a Source pane document
#'
#' Thin wrapper around [rstudioapi::setDocumentContents()] that returns
#' a structured list (instead of `NULL` invisibly) so callers can verify
#' what was written.
#'
#' @param text Character string. The new contents of the document. Single
#'   newlines separate lines.
#' @param id Document id (e.g. `"D4F4972F"`). When `NULL`, the active
#'   Source pane document is targeted.
#' @return A list with components:
#'   * `id`: the resolved document id (always a non-empty string).
#'   * `bytes_written`: `nchar(text, type = "bytes")` of the new content.
#' @export
#' @examples
#' \dontrun{
#' editor_set_contents("# new file\n1 + 1\n")
#' editor_set_contents("...", id = "D4F4972F")
#' }
editor_set_contents <- function(text, id = NULL) {
  if (!is.character(text) || length(text) != 1L) {
    stop("`text` must be a length-1 character vector", call. = FALSE)
  }
  resolved_id <- if (is.null(id)) {
    rstudioapi::documentId(allowConsole = FALSE)
  } else {
    id
  }
  if (is.null(resolved_id) || !nzchar(resolved_id)) {
    stop(
      "no active Source pane document; pass `id` explicitly",
      call. = FALSE
    )
  }
  rstudioapi::setDocumentContents(text = text, id = resolved_id)
  list(
    id = resolved_id,
    bytes_written = nchar(text, type = "bytes")
  )
}

#' Read the contents of a Source pane document
#'
#' Returns the live editor buffer (not the on-disk file) for an open
#' Source pane document, along with its metadata.
#'
#' @param id Document id. When `NULL`, the active Source pane document
#'   is targeted.
#' @return A list with components:
#'   * `id`: the resolved document id.
#'   * `path`: the file path of the document (empty string for an
#'     unsaved buffer).
#'   * `contents`: the live editor buffer as a single character string
#'     (lines separated by `\n`).
#' @export
#' @examples
#' \dontrun{
#' buf <- editor_get_contents()
#' nchar(buf$contents)
#' }
editor_get_contents <- function(id = NULL) {
  ctx <- if (is.null(id)) {
    rstudioapi::getSourceEditorContext()
  } else {
    rstudioapi::getSourceEditorContext(id = id)
  }
  if (is.null(ctx)) {
    stop(
      if (is.null(id)) "no active Source pane document"
      else sprintf("no open document with id '%s'", id),
      call. = FALSE
    )
  }
  list(
    id = ctx$id,
    path = ctx$path %||% "",
    contents = paste(ctx$contents, collapse = "\n")
  )
}

#' Open a file in the Source pane
#'
#' Non-modal: opens the file as a new document in the Source pane (or
#' switches focus to it if already open) and optionally jumps to a
#' specific line.
#'
#' @param path Path to the file to open.
#' @param line Optional 1-based line number to navigate to after opening.
#' @return A list with components:
#'   * `path`: the absolute path of the opened file.
#'   * `id`: the document id assigned by RStudio.
#' @export
#' @examples
#' \dontrun{
#' editor_open("~/projects/foo/R/main.R", line = 42)
#' }
editor_open <- function(path, line = NULL) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  abs_path <- normalizePath(path, mustWork = TRUE)
  doc_id <- if (is.null(line)) {
    rstudioapi::documentOpen(abs_path)
  } else {
    if (!is.numeric(line) || length(line) != 1L || line < 1L) {
      stop("`line` must be a positive integer", call. = FALSE)
    }
    rstudioapi::documentOpen(abs_path, line = as.integer(line))
  }
  list(path = abs_path, id = doc_id)
}

# Local %||% so we don't depend on rlang.
`%||%` <- function(x, y) if (is.null(x)) y else x
