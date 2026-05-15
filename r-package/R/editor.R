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

#' Read the live editor buffer of a Source pane document
#'
#' Returns the live editor buffer (not the on-disk file) for an open
#' Source pane document, along with its metadata. Mirrors the MCP /
#' CLI surface `editor.read-buffer`.
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
editor_read_buffer <- function(id = NULL) {
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
  # Create the document empty first, wait for the GWT client to stabilise,
  # then populate via setDocumentContents. documentNew(text = ...) internally
  # does the same two-step (create + insert) without a pause between them,
  # which leaves ghost lines in the editor buffer.
  id <- rstudioapi::documentNew(text = "", type = type, execute = FALSE)
  .throttle()
  if (nzchar(text)) {
    rstudioapi::setDocumentContents(text = text, id = id)
    .throttle()
  }
  if (isTRUE(execute)) {
    rstudioapi::sendToConsole(text, execute = TRUE)
  }
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
#' Wraps [rstudioapi::setSelectionRanges()]. Mirrors the MCP / CLI
#' surface `editor.select`.
#'
#' @param ranges A `document_range` (or list of them) describing the
#'   new selection.
#' @param id Document id. When `NULL`, the active document is targeted.
#' @return `NULL` invisibly. Side-effect only.
#' @export
editor_select <- function(ranges, id = NULL) {
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
#' Wraps [rstudioapi::documentPath()]. Mirrors the MCP / CLI surface
#' `editor.path`.
#'
#' @param id Document id. When `NULL`, the active document is used.
#' @return A list with `path` set to the file path, or `NULL` for an
#'   unsaved buffer.
#' @export
editor_path <- function(id = NULL) {
  p <- if (is.null(id)) {
    rstudioapi::documentPath()
  } else {
    rstudioapi::documentPath(id = id)
  }
  list(path = p)
}

#' Read the on-disk contents of a file
#'
#' Reads a file from disk (NOT the live editor buffer — use
#' [editor_read_buffer()] for that). Mirrors the MCP / CLI surface
#' `editor.read`.
#'
#' @param path File path. Tilde and relative paths are normalised.
#' @param encoding Encoding passed to [readLines()] (default `"UTF-8"`).
#' @return A list with components:
#'   * `path`: the canonicalised absolute path.
#'   * `contents`: the file's contents as a single character string
#'     (lines joined with `\n`).
#' @export
editor_read <- function(path, encoding = "UTF-8") {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  abs_path <- normalizePath(path, mustWork = TRUE)
  con <- file(abs_path, encoding = encoding)
  on.exit(close(con), add = TRUE)
  lines <- readLines(con, warn = FALSE)
  list(path = abs_path, contents = paste(lines, collapse = "\n"))
}

#' List every document currently open in the Source pane
#'
#' RStudio doesn't expose an `rstudioapi` getter that enumerates open
#' documents; the catalog is shipped to the GWT client through
#' `client_init` (which we must NOT call). We scan the on-disk source
#' database directory (filenames matching `^[0-9A-F]{8}$`) and pair each
#' id with whatever metadata `getSourceEditorContext(id = ...)` returns.
#' Mirrors the MCP / CLI surface `editor.list`.
#'
#' @return A list with a single component `documents`, a list of
#'   per-document records. Each record carries at least `id` and `path`
#'   (path may be the empty string for an unsaved buffer).
#' @export
editor_list <- function() {
  active <- list.files(
    path = file.path(
      Sys.getenv("HOME", unset = "~"),
      ".local", "share", "rstudio", "sessions", "active"
    ),
    full.names = FALSE,
    no.. = TRUE
  )
  # Pick the lone live session. Multiple session-* entries imply other
  # sessions; we still scan only the one rsession is in (no env var
  # exposes its id, but there should be only one when this R code runs).
  if (length(active) == 0L) {
    return(list(documents = list()))
  }
  sources_dir <- file.path(
    Sys.getenv("HOME", unset = "~"),
    ".local", "share", "rstudio", "sources", active[[1L]]
  )
  if (!dir.exists(sources_dir)) {
    return(list(documents = list()))
  }
  files <- list.files(sources_dir, full.names = FALSE, no.. = TRUE)
  ids <- files[grepl("^[0-9A-F]{8}$", files)]
  docs <- lapply(ids, function(id) {
    ctx <- tryCatch(
      rstudioapi::getSourceEditorContext(id = id),
      error = function(e) NULL
    )
    if (is.null(ctx)) {
      list(id = id, path = "")
    } else {
      list(id = ctx$id, path = ctx$path %||% "")
    }
  })
  list(documents = docs)
}

#' Reload a document from disk, replacing the live buffer
#'
#' Mirrors the MCP / CLI surface `editor.reload`. Useful when an
#' external tool (shell, git, your own write-to-disk) has updated a
#' file that's open in RStudio — without this call the user's buffer
#' would silently shadow the change until they manually reload.
#'
#' The document id stays the same so cached references remain valid;
#' the dirty flag is cleared as a side-effect.
#'
#' @param id Document id (length-1 character). Required: we don't pick
#'   the active document implicitly because reloading a doc the user
#'   isn't expecting would be surprising.
#' @param if_clean If `TRUE`, no-op when the buffer has unsaved
#'   changes (instead of overwriting them). Default `FALSE`.
#' @return `NULL` invisibly. Side-effect only.
#' @export
editor_reload <- function(id, if_clean = FALSE) {
  if (!is.character(id) || length(id) != 1L || !nzchar(id)) {
    stop("`id` must be a non-empty length-1 character vector", call. = FALSE)
  }
  if (isTRUE(if_clean)) {
    ctx <- tryCatch(
      rstudioapi::getSourceEditorContext(id = id),
      error = function(e) NULL
    )
    if (!is.null(ctx) && isTRUE(ctx$dirty)) {
      return(invisible(NULL))
    }
  }
  # rstudioapi has no public `revertDocument` at the time of writing;
  # the internal `.rs.api.documentClose(save = "asis")` + reopen would
  # change the id, which we don't want. Use the private hook directly.
  if (exists(".rs.api.documentRevert", mode = "function")) {
    .rs.api.documentRevert(id = id)
  } else {
    # Fallback: read from disk and setDocumentContents. Loses the
    # benefit of preserving cursor/scroll the official RPC gives us,
    # but works on any RStudio version.
    p <- rstudioapi::documentPath(id = id)
    if (is.null(p) || !nzchar(p) || !file.exists(p)) {
      stop(
        "editor_reload: document ",
        id,
        " has no on-disk path to reload from",
        call. = FALSE
      )
    }
    rstudioapi::setDocumentContents(
      text = paste(readLines(p, warn = FALSE), collapse = "\n"),
      id = id
    )
  }
  .throttle()
  invisible(NULL)
}

#' Show grep-style markers in the Markers pane
#'
#' Convenience wrapper around [pane_markers()] for the common case of
#' surfacing grep / ripgrep / ag output in the IDE. Accepts a character
#' vector of grep-format lines (`file:line:text` or `file:line:col:text`)
#' and turns them into the `markers` argument
#' [rstudioapi::sourceMarkers()] expects. Lines that don't match the
#' grep pattern are silently skipped. Mirrors the MCP / CLI surface
#' `editor.set-marks`.
#'
#' @param lines Character vector of grep-format lines.
#' @param name Marker pane title (default `"rstudio-cli"`).
#' @param type Severity applied to every marker. One of `"info"`,
#'   `"warning"`, `"error"`. Default `"info"`.
#' @param base_path Optional base directory for resolving relative
#'   `file` entries. Default `NULL` (let rstudioapi resolve).
#' @return The list of markers passed to `sourceMarkers()` (invisibly).
#' @export
editor_set_marks <- function(lines,
                             name = "rstudio-cli",
                             type = "info",
                             base_path = NULL) {
  if (!is.character(lines)) {
    stop("`lines` must be a character vector", call. = FALSE)
  }
  type <- match.arg(type, c("info", "warning", "error"))
  # Match grep -n / rg --vimgrep: file:line[:col]:text
  pat <- "^([^:]+):([0-9]+)(?::([0-9]+))?:(.*)$"
  m <- regmatches(lines, regexec(pat, lines))
  parsed <- Filter(function(x) length(x) == 5L, m)
  if (length(parsed) == 0L) {
    return(invisible(list()))
  }
  markers <- lapply(parsed, function(x) {
    list(
      type = type,
      file = x[[2L]],
      line = as.integer(x[[3L]]),
      column = if (nzchar(x[[4L]])) as.integer(x[[4L]]) else 1L,
      message = x[[5L]]
    )
  })
  pane_markers(name = name, markers = markers, base_path = base_path)
  invisible(markers)
}

# Local %||% so we don't depend on rlang.
`%||%` <- function(x, y) if (is.null(x)) y else x
