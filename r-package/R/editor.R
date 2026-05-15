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
  .throttle()
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
#' specific line/column.
#'
#' @param path Path to the file to open.
#' @param line Optional 1-based line number to navigate to after
#'   opening. Use `-1L` (default) to leave the cursor where it was.
#' @param col Optional 1-based column. Use `-1L` (default) to leave it.
#' @param move_cursor If `TRUE` (default), actually move the cursor;
#'   if `FALSE`, the file opens but cursor position is untouched.
#' @return A list with components:
#'   * `path`: the absolute path of the opened file.
#'   * `id`: the document id assigned by RStudio.
#' @export
editor_open <- function(path, line = -1L, col = -1L, move_cursor = TRUE) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  abs_path <- normalizePath(path, mustWork = TRUE)
  doc_id <- rstudioapi::documentOpen(
    abs_path,
    line = as.integer(line),
    col = as.integer(col),
    moveCursor = isTRUE(move_cursor)
  )
  .throttle()
  list(path = abs_path, id = doc_id)
}

#' Close one or more documents in the Source pane
#'
#' Wraps `.rs.api.documentClose()` — `rstudioapi` doesn't expose a
#' public `documentClose` at the time of writing.
#'
#' @param id Document id. When `NULL`, the active document is closed.
#' @param save Save behaviour when the document is dirty:
#'   * `TRUE`: save before closing.
#'   * `FALSE`: discard changes.
#'   * `"ask"`: prompt the user (modal).
#' @return `NULL` invisibly. Side-effect only.
#' @export
editor_close <- function(id = NULL, save = "ask") {
  .rs.api.documentClose(id = id, save = save)
  .throttle()
  invisible(NULL)
}

#' Save a document
#'
#' Wraps `.rs.api.documentSave()`. `rstudioapi::documentSave` is
#' available too — we use the internal symmetric variant of
#' `.rs.api.documentClose` for stylistic consistency.
#'
#' @param id Document id. When `NULL`, the active document is saved.
#' @return `NULL` invisibly. Side-effect only.
#' @export
editor_save <- function(id = NULL) {
  .rs.api.documentSave(id = id)
  .throttle()
  invisible(NULL)
}

#' Save every open document
#'
#' Wraps `.rs.api.documentSaveAll()`.
#'
#' @return `NULL` invisibly. Side-effect only.
#' @export
editor_save_all <- function() {
  .rs.api.documentSaveAll()
  .throttle()
  invisible(NULL)
}

#' Create a new (unsaved) document in the Source pane
#'
#' Wraps [rstudioapi::documentNew()].
#'
#' @param text Initial contents (default: empty).
#' @param type Document type (`"r"`, `"rmarkdown"`, `"sql"`, ...).
#' @param execute If `TRUE`, run the contents as R code immediately
#'   after creating the document.
#' @return A list with `id` set to the new document's id and the
#'   `type` it was created as.
#' @export
editor_new <- function(text = "", type = "r", execute = FALSE) {
  id <- rstudioapi::documentNew(text = text, type = type, execute = execute)
  .throttle()
  list(id = id, type = type)
}

#' Insert text at the cursor (or replace the current selection)
#'
#' Wraps [rstudioapi::insertText()].
#'
#' @param text Text to insert.
#' @param id Document id. When `NULL`, the active document is targeted.
#' @return `NULL` invisibly. Side-effect only.
#' @export
editor_insert <- function(text, id = NULL) {
  if (!is.character(text) || length(text) != 1L) {
    stop("`text` must be a length-1 character vector", call. = FALSE)
  }
  if (is.null(id)) {
    rstudioapi::insertText(text = text)
  } else {
    rstudioapi::insertText(text = text, id = id)
  }
  .throttle()
  invisible(NULL)
}

#' Replace the text inside a specific range
#'
#' Wraps [rstudioapi::modifyRange()].
#'
#' @param range A `document_range` object (typically constructed via
#'   [rstudioapi::document_range()] — see `?rstudioapi::document_range`).
#' @param text Replacement text.
#' @param id Document id. When `NULL`, the active document is targeted.
#' @return `NULL` invisibly. Side-effect only.
#' @export
editor_modify_range <- function(range, text, id = NULL) {
  if (!is.character(text) || length(text) != 1L) {
    stop("`text` must be a length-1 character vector", call. = FALSE)
  }
  if (is.null(id)) {
    rstudioapi::modifyRange(location = range, text = text)
  } else {
    rstudioapi::modifyRange(location = range, text = text, id = id)
  }
  .throttle()
  invisible(NULL)
}

#' Move the cursor in a document
#'
#' Wraps [rstudioapi::setCursorPosition()].
#'
#' @param position A `document_position` (or a length-2 integer vector
#'   `c(row, column)`).
#' @param id Document id. When `NULL`, the active document is targeted.
#' @return `NULL` invisibly. Side-effect only.
#' @export
editor_set_cursor <- function(position, id = NULL) {
  if (is.numeric(position) && length(position) == 2L) {
    position <- rstudioapi::document_position(
      as.integer(position[1L]),
      as.integer(position[2L])
    )
  }
  if (is.null(id)) {
    rstudioapi::setCursorPosition(position = position)
  } else {
    rstudioapi::setCursorPosition(position = position, id = id)
  }
  .throttle()
  invisible(NULL)
}

#' Set the selection in a document
#'
#' Wraps [rstudioapi::setSelectionRanges()].
#'
#' @param ranges A `document_range` (or list of them) describing the
#'   new selection.
#' @param id Document id. When `NULL`, the active document is targeted.
#' @return `NULL` invisibly. Side-effect only.
#' @export
editor_select_range <- function(ranges, id = NULL) {
  if (is.null(id)) {
    rstudioapi::setSelectionRanges(ranges = ranges)
  } else {
    rstudioapi::setSelectionRanges(ranges = ranges, id = id)
  }
  .throttle()
  invisible(NULL)
}

#' Active document context (cursor, selection, contents, path)
#'
#' Wraps [rstudioapi::getSourceEditorContext()] /
#' [rstudioapi::getActiveDocumentContext()]. Use `console = TRUE` to
#' also consider the console as the "active" document (which RStudio
#' does by default for `getActiveDocumentContext`).
#'
#' @param id Document id. When `NULL`, the active document is used.
#' @param console If `TRUE`, the console pane is a valid "active"
#'   document (default `FALSE` — most CLI callers want the Source
#'   pane only).
#' @return A list as returned by `rstudioapi` — fields `id`, `path`,
#'   `contents`, `selection`, etc.
#' @export
editor_context <- function(id = NULL, console = FALSE) {
  ctx <- if (console) {
    rstudioapi::getActiveDocumentContext()
  } else if (is.null(id)) {
    rstudioapi::getSourceEditorContext()
  } else {
    rstudioapi::getSourceEditorContext(id = id)
  }
  ctx
}

#' Active document id
#'
#' Wraps [rstudioapi::documentId()].
#'
#' @param allow_console If `TRUE`, the console is a valid "active"
#'   document; otherwise only Source pane documents qualify.
#' @return A list with `id` set to the active id, or `NULL` when
#'   there is no active document.
#' @export
editor_active_id <- function(allow_console = TRUE) {
  id <- rstudioapi::documentId(allowConsole = allow_console)
  list(id = id)
}

#' Path of an open document
#'
#' Wraps [rstudioapi::documentPath()].
#'
#' @param id Document id. When `NULL`, the active document is used.
#' @return A list with `path` set to the file path, or `NULL` for an
#'   unsaved buffer.
#' @export
editor_document_path <- function(id = NULL) {
  p <- if (is.null(id)) {
    rstudioapi::documentPath()
  } else {
    rstudioapi::documentPath(id = id)
  }
  list(path = p)
}

# Local %||% so we don't depend on rlang.
`%||%` <- function(x, y) if (is.null(x)) y else x
