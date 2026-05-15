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
