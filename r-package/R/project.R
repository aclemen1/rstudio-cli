#' Active project path
#'
#' Returns the path of the currently-open RStudio project, or `NULL`
#' when no project is active. Thin wrapper around
#' [rstudioapi::getActiveProject()].
#'
#' @return A character path or `NULL`.
#' @export
project_current <- function() {
  rstudioapi::getActiveProject()
}

#' Open an RStudio project (destructive)
#'
#' Opens an existing `.Rproj`. Restarts the R session unless
#' `new_session = TRUE`.
#'
#' @param path Path to the project directory or to a `.Rproj` file.
#' @param new_session If `TRUE`, open in a new RStudio window/session
#'   without restarting the current one.
#' @return `NULL` invisibly. Side-effect only.
#' @export
project_open <- function(path, new_session = FALSE) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  if (!is.logical(new_session) || length(new_session) != 1L) {
    stop("`new_session` must be a length-1 logical", call. = FALSE)
  }
  rstudioapi::openProject(path = path, newSession = new_session)
  invisible(NULL)
}
