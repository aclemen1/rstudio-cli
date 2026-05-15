#' Aggregate session info
#'
#' One-shot inspection of the active RStudio session: R/RStudio versions,
#' user identity, color-console support, active project. Wraps the
#' `rstudioapi` introspection functions into a single list.
#'
#' @return A named list with components:
#'   * `version`: RStudio short version string (character).
#'   * `long_version`: long version string (character).
#'   * `release_name`: release codename, when present.
#'   * `r_version`: R version (character, from `R.version.string`).
#'   * `mode`: `"desktop"` or `"server"`.
#'   * `user_identity`: identity reported by RStudio.
#'   * `system_username`: OS-level username.
#'   * `has_color_console`: whether the console supports ANSI colour.
#'   * `active_project`: project root path (NA when no project is active).
#' @export
session_info <- function() {
  vi <- rstudioapi::versionInfo()
  proj <- rstudioapi::getActiveProject()
  list(
    version = as.character(vi$version),
    long_version = vi$long_version,
    release_name = vi$release_name,
    r_version = R.version.string,
    mode = vi$mode,
    user_identity = rstudioapi::userIdentity(),
    system_username = rstudioapi::systemUsername(),
    has_color_console = rstudioapi::hasColorConsole(),
    active_project = if (is.null(proj)) NA else proj
  )
}

#' Restart the R session (destructive)
#'
#' Drops all in-memory R objects. Optionally runs an R command after
#' restart.
#'
#' @param command Optional R code to evaluate once the session is back up
#'   (passed straight through to `rstudioapi::restartSession`). Empty
#'   string (default) means "just restart, no follow-up".
#' @return `NULL` invisibly. Side-effect only.
#' @export
session_restart <- function(command = "") {
  if (!is.character(command) || length(command) != 1L) {
    stop("`command` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::restartSession(command = command)
  invisible(NULL)
}

#' List all active RStudio Server rsession sockets for the current user
#'
#' Mirrors the MCP / CLI surface `session.list`. Scans
#' `/var/run/rstudio-server/rstudio-rsession/` (or the path in the
#' `RS_SESSION_TMP_DIR` env var) for socket files. Useful when an
#' agent / CLI is running outside RStudio and needs to discover which
#' sessions it could connect to.
#'
#' Empty list on platforms where the directory doesn't exist (Desktop
#' on macOS, Server with a non-standard config, etc.).
#'
#' @return A list with one component `sessions`, a list of records
#'   each carrying `socket` (the absolute path to the rsession Unix
#'   domain socket).
#' @export
session_list <- function() {
  dir <- Sys.getenv("RS_SESSION_TMP_DIR",
                    unset = "/var/run/rstudio-server/rstudio-rsession")
  if (!dir.exists(dir)) {
    return(list(sessions = list()))
  }
  entries <- list.files(dir, full.names = TRUE, no.. = TRUE)
  socks <- character()
  for (p in entries) {
    # The CLI side filters by uid via stat(); R lacks a portable uid
    # check. Use the same name-based filter (skip .pid files) and let
    # downstream connect() report 'permission denied' if a socket
    # belongs to someone else.
    if (endsWith(p, ".pid")) next
    info <- file.info(p)
    if (isTRUE(info$isdir)) next
    socks <- c(socks, p)
  }
  list(sessions = lapply(socks, function(p) list(socket = p)))
}
