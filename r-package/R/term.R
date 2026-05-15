#' List all RStudio terminal pane sessions
#'
#' Returns one entry per terminal with the fields the agent typically
#' wants: identity (id, caption, title, working_dir, shell), lifecycle
#' (running, busy, exit_code), and dimensions (cols, rows, lines).
#'
#' @return A list of named lists, one per terminal. Empty list if none
#'   are open.
#' @export
term_list <- function() {
  ids <- rstudioapi::terminalList()
  if (length(ids) == 0) return(list())
  lapply(ids, function(id) {
    ctx <- rstudioapi::terminalContext(id)
    list(
      id = ctx$handle,
      caption = ctx$caption,
      title = ctx$title,
      working_dir = ctx$working_dir,
      shell = ctx$shell,
      running = ctx$running,
      busy = ctx$busy,
      exit_code = ctx$exit_code,
      pid = ctx$pid,
      cols = ctx$cols,
      rows = ctx$rows,
      lines = ctx$lines,
      connection = ctx$connection
    )
  })
}

#' Get the rstudioapi context object for one terminal
#'
#' Thin wrapper around [rstudioapi::terminalContext()]. The shape of
#' the return value is dictated by `rstudioapi`; downstream consumers
#' should not rely on stability beyond what `?rstudioapi::terminalContext`
#' documents.
#'
#' @param id Terminal handle.
#' @return The context list returned by `rstudioapi::terminalContext()`.
#' @export
term_context <- function(id) {
  if (!is.character(id) || length(id) != 1L) {
    stop("`id` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::terminalContext(id)
}

#' Read the buffer of one terminal
#'
#' Wraps [rstudioapi::terminalBuffer()]. Returns the lines as a
#' character vector. The CLI takes care of `tail()`-style truncation
#' before serialising — we don't truncate on this side so the function
#' is useful from R directly.
#'
#' @param id Terminal handle.
#' @param strip_ansi If `TRUE` (default), strip ANSI escape sequences;
#'   if `FALSE`, keep them (raw OSC/CSI sequences come through).
#' @return Character vector of lines.
#' @export
term_buffer <- function(id, strip_ansi = TRUE) {
  if (!is.character(id) || length(id) != 1L) {
    stop("`id` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::terminalBuffer(id, stripAnsi = strip_ansi)
}

#' Create a new terminal
#'
#' Wraps [rstudioapi::terminalCreate()].
#'
#' @param caption Optional caption shown in the terminal pane tab.
#' @param shell_type Optional shell to spawn (system default if `NULL`).
#' @param show If `TRUE`, give the new terminal focus.
#' @return The new terminal's handle (character).
#' @export
term_create <- function(caption = NULL, shell_type = NULL, show = TRUE) {
  id <- rstudioapi::terminalCreate(
    caption = caption,
    show = show,
    shellType = shell_type
  )
  .throttle()
  id
}

#' Send text to a terminal
#'
#' Wraps [rstudioapi::terminalSend()]. The text is sent verbatim — no
#' newline appended.
#'
#' @param id Terminal handle.
#' @param text Text to send.
#' @return `NULL` invisibly. Side-effect only.
#' @export
term_send <- function(id, text) {
  if (!is.character(id) || length(id) != 1L) {
    stop("`id` must be a length-1 character vector", call. = FALSE)
  }
  if (!is.character(text) || length(text) != 1L) {
    stop("`text` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::terminalSend(id, text)
  .throttle()
  invisible(NULL)
}

#' Send text to a terminal followed by a newline (i.e. "execute it")
#'
#' Convenience over [term_send()] that ensures a trailing newline.
#'
#' @inheritParams term_send
#' @return `NULL` invisibly. Side-effect only.
#' @export
term_exec <- function(id, text) {
  if (!is.character(id) || length(id) != 1L) {
    stop("`id` must be a length-1 character vector", call. = FALSE)
  }
  if (!is.character(text) || length(text) != 1L) {
    stop("`text` must be a length-1 character vector", call. = FALSE)
  }
  if (!endsWith(text, "\n")) text <- paste0(text, "\n")
  rstudioapi::terminalSend(id, text)
  .throttle()
  invisible(NULL)
}

#' Kill a terminal
#'
#' Wraps [rstudioapi::terminalKill()]. The process is terminated and
#' the terminal pane tab is removed.
#'
#' @param id Terminal handle.
#' @return `NULL` invisibly. Side-effect only.
#' @export
term_kill <- function(id) {
  if (!is.character(id) || length(id) != 1L) {
    stop("`id` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::terminalKill(id)
  .throttle()
  invisible(NULL)
}

#' Clear a terminal buffer
#'
#' Wraps [rstudioapi::terminalClear()].
#'
#' @param id Terminal handle.
#' @return `NULL` invisibly. Side-effect only.
#' @export
term_clear <- function(id) {
  if (!is.character(id) || length(id) != 1L) {
    stop("`id` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::terminalClear(id)
  .throttle()
  invisible(NULL)
}

#' Activate (give focus to) a terminal
#'
#' Wraps [rstudioapi::terminalActivate()].
#'
#' @param id Terminal handle.
#' @return `NULL` invisibly. Side-effect only.
#' @export
term_activate <- function(id) {
  if (!is.character(id) || length(id) != 1L) {
    stop("`id` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::terminalActivate(id)
  .throttle()
  invisible(NULL)
}

#' Is the given terminal still running?
#'
#' "Running" here means the shell is still alive, distinct from
#' "busy" (which means a foreground command is currently executing).
#' Wraps [rstudioapi::terminalRunning()].
#'
#' @param id Terminal handle.
#' @return A list with `running` (logical).
#' @export
term_running <- function(id) {
  list(running = rstudioapi::terminalRunning(id))
}

#' Is the given terminal currently executing a foreground command?
#'
#' Distinct from `term_running()` — a terminal can be "running" (shell
#' alive) without being "busy" (no foreground command). Wraps
#' [rstudioapi::terminalBusy()].
#'
#' @param id Terminal handle.
#' @return A list with `busy` (logical).
#' @export
term_busy <- function(id) {
  list(busy = rstudioapi::terminalBusy(id))
}

#' Exit code of the last foreground command in a terminal
#'
#' Wraps [rstudioapi::terminalExitCode()]. Returns `NULL` (in the
#' `exit_code` field) if no command has finished yet.
#'
#' @param id Terminal handle.
#' @return A list with `exit_code` (integer or `NULL`).
#' @export
term_exit_code <- function(id) {
  c <- rstudioapi::terminalExitCode(id)
  list(exit_code = c)
}

#' Identifier of the currently-visible terminal
#'
#' Wraps [rstudioapi::terminalVisible()].
#'
#' @return A list with `id` set to the visible terminal's handle, or
#'   `NULL` if no terminal is open.
#' @export
term_visible <- function() {
  id <- rstudioapi::terminalVisible()
  list(id = id)
}

#' Execute a one-shot command in a fresh terminal
#'
#' Wraps [rstudioapi::terminalExecute()] (spawns a new terminal whose
#' entire purpose is to run `command`). Compare with [term_exec()],
#' which sends to an *existing* terminal.
#'
#' @param command Command line to run.
#' @param working_dir Optional working directory. `NULL` uses the
#'   project root (or HOME if no project).
#' @param env Optional named character vector of environment variables
#'   to set in the spawned terminal (e.g. `c(FOO = "bar")`).
#' @param show If `TRUE` (default), bring the new terminal to the front.
#' @return The new terminal's handle (character).
#' @export
term_run <- function(command, working_dir = NULL, env = NULL, show = TRUE) {
  if (!is.character(command) || length(command) != 1L) {
    stop("`command` must be a length-1 character vector", call. = FALSE)
  }
  id <- rstudioapi::terminalExecute(
    command = command,
    workingDir = working_dir,
    env = env,
    show = show
  )
  .throttle()
  id
}
