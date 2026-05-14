#' Show a modal information dialog (blocking)
#'
#' Thin wrapper around [rstudioapi::showDialog()]. The current R session
#' is blocked until the user dismisses the dialog.
#'
#' @param title Dialog title.
#' @param message Body text. Markdown-style links accepted by RStudio.
#' @param url Optional URL displayed as a button in the dialog.
#' @return `NULL` invisibly. Side-effect only.
#' @export
ui_dialog <- function(title, message, url = "") {
  if (!is.character(title) || length(title) != 1L) {
    stop("`title` must be a length-1 character vector", call. = FALSE)
  }
  if (!is.character(message) || length(message) != 1L) {
    stop("`message` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::showDialog(title = title, message = message, url = url)
  invisible(NULL)
}

#' Update an open dialog (must be called from inside a dialog callback)
#'
#' Wraps [rstudioapi::updateDialog()]. Mutates the content of the
#' currently-displayed dialog without re-opening it. Only meaningful
#' from inside a callback; calling it outside a dialog flow is a no-op.
#'
#' @param ... Named arguments passed through to `updateDialog`.
#' @return `NULL` invisibly. Side-effect only.
#' @export
ui_dialog_update <- function(...) {
  rstudioapi::updateDialog(...)
  invisible(NULL)
}

#' Show a modal text prompt (blocking) and return the user input
#'
#' Wraps [rstudioapi::showPrompt()]. Returns `NULL` if the user cancels.
#'
#' @param title Dialog title.
#' @param message Prompt message.
#' @param default Pre-filled value (default: empty).
#' @return A list with `value` set to the typed string, or `NULL` if
#'   the user cancelled.
#' @export
ui_prompt <- function(title, message, default = NULL) {
  v <- rstudioapi::showPrompt(title = title, message = message, default = default)
  list(value = v)
}

#' Show a modal yes/no question (blocking)
#'
#' Wraps [rstudioapi::showQuestion()]. The "ok" button maps to `TRUE`,
#' "cancel" to `FALSE`.
#'
#' @param title Dialog title.
#' @param message Question message.
#' @param ok Label for the affirmative button (default `"OK"`).
#' @param cancel Label for the cancel button (default `"Cancel"`).
#' @return A list with `answer` set to `TRUE` or `FALSE`.
#' @export
ui_question <- function(title, message, ok = "OK", cancel = "Cancel") {
  a <- rstudioapi::showQuestion(title = title, message = message, ok = ok, cancel = cancel)
  list(answer = a)
}

#' Show a modal file picker (blocking)
#'
#' Wraps [rstudioapi::selectFile()]. Returns `NULL` if cancelled.
#'
#' @param caption Dialog caption.
#' @param label Label of the confirm button.
#' @param path Initial directory. `NULL` uses the active project, when
#'   any (via [rstudioapi::getActiveProject()]).
#' @param filter File filter (e.g. `"R files (*.R)"`).
#' @param existing If `TRUE`, only allow selecting an existing file
#'   (open semantics). If `FALSE`, allow typing a new filename (save
#'   semantics).
#' @return A list with `path` set to the chosen file, or `NULL` if
#'   cancelled.
#' @export
ui_select_file <- function(caption, label, path = NULL, filter = "All files (*)", existing = TRUE) {
  if (is.null(path)) path <- rstudioapi::getActiveProject()
  p <- rstudioapi::selectFile(
    caption = caption, label = label, path = path,
    filter = filter, existing = existing
  )
  list(path = p)
}

#' Show a modal directory picker (blocking)
#'
#' Wraps [rstudioapi::selectDirectory()]. Returns `NULL` if cancelled.
#'
#' @inheritParams ui_select_file
#' @return A list with `path` set to the chosen directory, or `NULL`
#'   if cancelled.
#' @export
ui_select_dir <- function(caption, label, path = NULL) {
  if (is.null(path)) path <- rstudioapi::getActiveProject()
  p <- rstudioapi::selectDirectory(caption = caption, label = label, path = path)
  list(path = p)
}

#' Ask for a password (blocking)
#'
#' Wraps [rstudioapi::askForPassword()]. The typed value is returned
#' in cleartext — caller is responsible for not exposing it
#' inadvertently.
#'
#' @param prompt Prompt label shown to the user.
#' @return A list with `value` set to the password, or `NULL` if
#'   cancelled.
#' @export
ui_ask_password <- function(prompt) {
  v <- rstudioapi::askForPassword(prompt = prompt)
  list(value = v)
}

#' Ask for a secret, cached in the system keyring (blocking on first call)
#'
#' Wraps [rstudioapi::askForSecret()]. After the first invocation
#' that records a value for `name`, subsequent calls return the cached
#' value silently — no further prompt.
#'
#' @param name Keyring name under which the secret is stored.
#' @param message Optional prompt message.
#' @param title Optional dialog title.
#' @return A list with `value` set to the secret, or `NULL` if
#'   cancelled.
#' @export
ui_ask_secret <- function(name, message = NULL, title = NULL) {
  v <- rstudioapi::askForSecret(name = name, message = message, title = title)
  list(value = v)
}
