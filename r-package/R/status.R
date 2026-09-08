#' Snapshot of the RStudio session for status reporting
#'
#' Tolerant inspection used by `rstudio status`: every field is wrapped
#' in `tryCatch` so a missing/old RStudio doesn't fail the whole call.
#' Returns whatever it can.
#'
#' Deliberately asks only for what rsession can answer WITHOUT a connected
#' client. The active document is intentionally excluded:
#' `rstudioapi::documentId()` / `documentPath()` round-trip through the
#' RStudio client and block the R console until a browser tab / Desktop
#' window answers — a call that cannot be cancelled once dispatched. Agents
#' that need the active document call `editor active-id` / `editor context`
#' deliberately, when a client is present.
#'
#' @return A named list with components:
#'   * `r_version`: R version string (always present).
#'   * `rstudio_version`: RStudio version string (`NULL` if unavailable).
#'   * `active_project`: project root path (`NULL` if no project).
#' @export
status_snapshot <- function() {
  list(
    r_version = R.version$version.string,
    rstudio_version = tryCatch(
      as.character(rstudioapi::versionInfo()$version),
      error = function(e) NULL
    ),
    active_project = tryCatch(
      rstudioapi::getActiveProject(),
      error = function(e) NULL
    )
  )
}
