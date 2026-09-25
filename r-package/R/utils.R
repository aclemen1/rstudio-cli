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

#' Per-user rstudio-cli directory, with a fallback for R < 4.0
#'
#' `tools::R_user_dir()` was added in R 4.0. On older R (e.g. 3.6.3) it is
#' not exported, so a bare call errors with
#' `'R_user_dir' is not an exported object from 'namespace:tools'`. This
#' helper uses `R_user_dir()` when available and otherwise falls back to the
#' XDG base directory the same way R 4.0 would, so the resolved path matches
#' on R >= 4.0 and stays sane on older R.
#'
#' @param which One of `"data"`, `"config"`, `"cache"`.
#' @return Absolute path to the rstudio-cli directory for `which`.
#' @keywords internal
.rscli_user_dir <- function(which = c("data", "config", "cache")) {
  which <- match.arg(which)
  if (exists("R_user_dir", envir = asNamespace("tools"))) {
    return(tools::R_user_dir("rstudio-cli", which))
  }
  base <- switch(
    which,
    data = Sys.getenv("XDG_DATA_HOME", "~/.local/share"),
    config = Sys.getenv("XDG_CONFIG_HOME", "~/.config"),
    cache = Sys.getenv("XDG_CACHE_HOME", "~/.cache")
  )
  path.expand(file.path(base, "R", "rstudio-cli"))
}
