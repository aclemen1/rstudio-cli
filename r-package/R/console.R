#' Activate the R console pane (give it focus)
#'
#' Wraps the RStudio named command `activateConsole` via
#' [rstudioapi::executeCommand()]. Symmetric to `term activate <id>` for
#' the console (singleton, no id).
#'
#' @return `NULL` invisibly. Side-effect only.
#' @export
console_activate <- function() {
  rstudioapi::executeCommand("activateConsole", quiet = TRUE)
  invisible(NULL)
}

#' Most recent console commands typed by the user
#'
#' Mirrors the MCP / CLI surface `console.history`. Calls
#' [utils::savehistory()] to a tempfile and reads it back. Note: the
#' RStudio Server / Desktop console history is what gets persisted —
#' not the rstudio-cli's own `r_send` / `r_exec` calls.
#'
#' @param limit Maximum number of recent commands to return (default 50).
#' @return A list with one component `commands`, a character vector of
#'   the most recent commands (newest last).
#' @export
console_history <- function(limit = 50L) {
  if (!is.numeric(limit) || length(limit) != 1L || limit <= 0L) {
    stop("`limit` must be a positive integer", call. = FALSE)
  }
  tf <- tempfile()
  on.exit(unlink(tf), add = TRUE)
  tryCatch(
    utils::savehistory(tf),
    error = function(e) {
      stop("console_history: cannot read history (", conditionMessage(e), ")",
           call. = FALSE)
    }
  )
  lines <- tryCatch(readLines(tf, warn = FALSE), error = function(e) character())
  if (length(lines) > limit) {
    lines <- utils::tail(lines, n = as.integer(limit))
  }
  list(commands = as.list(lines))
}

#' Context of the R console (cursor, current input buffer, selection)
#'
#' Mirrors the MCP / CLI surface `console.context`. Wraps
#' [rstudioapi::getConsoleEditorContext()] — returns whatever the user
#' currently has typed (but not yet entered) in the console, plus
#' selections / cursor coordinates.
#'
#' @return A list with `id`, `path`, `contents`, `selections`, or
#'   `NULL` if the console is not the focused pane.
#' @export
console_context <- function() {
  ctx <- tryCatch(
    rstudioapi::getConsoleEditorContext(),
    error = function(e) NULL
  )
  if (is.null(ctx)) {
    return(NULL)
  }
  selections <- lapply(ctx$selection %||% list(), function(s) {
    list(
      start_row = as.integer(s$range$start[[1L]]),
      start_col = as.integer(s$range$start[[2L]]),
      end_row = as.integer(s$range$end[[1L]]),
      end_col = as.integer(s$range$end[[2L]]),
      text = s$text
    )
  })
  list(
    id = ctx$id %||% "",
    path = ctx$path %||% "",
    contents = paste(ctx$contents %||% character(), collapse = "\n"),
    selections = selections
  )
}

# Local %||% so we don't depend on rlang.
`%||%` <- function(x, y) if (is.null(x)) y else x
