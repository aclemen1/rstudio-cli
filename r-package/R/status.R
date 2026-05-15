#' Snapshot of the RStudio session for status reporting
#'
#' Tolerant inspection used by `rstudio status`: every field is wrapped
#' in `tryCatch` so a missing/old RStudio doesn't fail the whole call.
#' Returns whatever it can.
#'
#' @return A named list with components:
#'   * `r_version`: R version string (always present).
#'   * `rstudio_version`: RStudio version string (`NULL` if unavailable).
#'   * `active_project`: project root path (`NULL` if no project).
#'   * `active_doc_id`: id of the active Source pane document
#'     (`NULL` if none, allowConsole = FALSE).
#'   * `active_doc_path`: path of the active document (`NULL` if none).
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
    ),
    active_doc_id = tryCatch(
      rstudioapi::documentId(allowConsole = FALSE),
      error = function(e) NULL
    ),
    active_doc_path = tryCatch(
      rstudioapi::documentPath(),
      error = function(e) NULL
    )
  )
}
