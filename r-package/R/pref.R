#' Read a user preference
#'
#' Wraps [rstudioapi::readPreference()]. User preferences are
#' project-scoped or user-scoped key-value entries, distinct from
#' built-in RStudio preferences (see [pref_read_rstudio()]).
#'
#' @param name Preference name.
#' @param default Fallback value when the preference doesn't exist.
#'   Returned as-is (any R type) — the CLI side serialises with
#'   `jsonlite::toJSON`.
#' @return A list with `name` and `value` fields.
#' @export
pref_read <- function(name, default = NULL) {
  if (!is.character(name) || length(name) != 1L) {
    stop("`name` must be a length-1 character vector", call. = FALSE)
  }
  list(
    name = name,
    value = rstudioapi::readPreference(name, default = default)
  )
}

#' Write a user preference
#'
#' Wraps [rstudioapi::writePreference()].
#'
#' @param name Preference name.
#' @param value Any R value (lists, scalars, vectors). Stored as-is.
#' @return `NULL` invisibly. Side-effect only.
#' @export
pref_write <- function(name, value) {
  if (!is.character(name) || length(name) != 1L) {
    stop("`name` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::writePreference(name, value = value)
  invisible(NULL)
}

#' Read a built-in RStudio preference
#'
#' Built-in preferences are the ones surfaced in Tools > Global Options;
#' use [pref_read()] for arbitrary user-defined settings.
#'
#' @inheritParams pref_read
#' @return A list with `name` and `value` fields.
#' @export
pref_read_rstudio <- function(name, default = NULL) {
  if (!is.character(name) || length(name) != 1L) {
    stop("`name` must be a length-1 character vector", call. = FALSE)
  }
  list(
    name = name,
    value = rstudioapi::readRStudioPreference(name, default = default)
  )
}

#' Write a built-in RStudio preference
#'
#' @inheritParams pref_write
#' @return `NULL` invisibly. Side-effect only.
#' @export
pref_write_rstudio <- function(name, value) {
  if (!is.character(name) || length(name) != 1L) {
    stop("`name` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::writeRStudioPreference(name, value = value)
  invisible(NULL)
}

#' Read a persistent value
#'
#' Persistent values live in RStudio's session-storage key-value area,
#' separate from preferences. Wraps [rstudioapi::getPersistentValue()].
#'
#' @param name Persistent value name.
#' @return A list with `name` and `value` fields.
#' @export
pref_get_persistent <- function(name) {
  if (!is.character(name) || length(name) != 1L) {
    stop("`name` must be a length-1 character vector", call. = FALSE)
  }
  list(name = name, value = rstudioapi::getPersistentValue(name))
}

#' Write a persistent value
#'
#' Wraps [rstudioapi::setPersistentValue()].
#'
#' @inheritParams pref_write
#' @return `NULL` invisibly. Side-effect only.
#' @export
pref_set_persistent <- function(name, value) {
  if (!is.character(name) || length(name) != 1L) {
    stop("`name` must be a length-1 character vector", call. = FALSE)
  }
  rstudioapi::setPersistentValue(name, value = value)
  invisible(NULL)
}
