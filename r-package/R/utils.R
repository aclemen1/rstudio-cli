#' Throttle UI-mutating operations
#'
#' Sleeps for a short, configurable interval. Called by wrappers that
#' mutate the editor / pane state through `rstudioapi` so that the GWT
#' client (Chrome / Electron / etc.) has time to acknowledge each event
#' before the next call lands. Without it, back-to-back RPCs like
#' `editor_open → editor_set_contents → editor_close` can saturate
#' rsession's event channel and wedge subsequent calls.
#'
#' Resolution order, first match wins:
#'   1. `options(rstudiocli.throttle_ms = ...)` (R session-level override)
#'   2. `Sys.getenv("RSTUDIOCLI_THROTTLE_MS")` (env-var override, useful
#'      for CI / Docker)
#'   3. default = 200 ms
#'
#' A value of 0 disables the throttle entirely.
#'
#' @return `NULL` invisibly.
#' @keywords internal
.throttle <- function() {
  ms <- getOption("rstudiocli.throttle_ms", NA_integer_)
  if (is.na(ms)) {
    env_val <- Sys.getenv("RSTUDIOCLI_THROTTLE_MS", unset = NA_character_)
    ms <- if (is.na(env_val) || !nzchar(env_val)) {
      500L
    } else {
      suppressWarnings(as.integer(env_val))
    }
  }
  if (is.na(ms) || !is.numeric(ms) || ms <= 0L) {
    return(invisible(NULL))
  }
  Sys.sleep(as.numeric(ms) / 1000)
  invisible(NULL)
}
