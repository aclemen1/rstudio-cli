#' List variables in the active R environment
#'
#' Mirrors the MCP / CLI surface `env.list`. Returns one record per
#' variable visible in `globalenv()` (or in the environment currently
#' pinned in RStudio's Environment pane, if different), with the
#' metadata an agent typically wants: type, class, length, size in
#' bytes, and a short one-line description.
#'
#' @param pattern Optional regex; only variables whose name matches are
#'   returned. Default `NULL` (no filter).
#' @return A list with one component `vars`, a list of records sorted
#'   by name. Each record carries `name`, `type`, `class`, `length`,
#'   `size_bytes`, `description`, `is_data`.
#' @export
env_list <- function(pattern = NULL) {
  if (!is.null(pattern) && (!is.character(pattern) || length(pattern) != 1L)) {
    stop("`pattern` must be NULL or a length-1 character vector", call. = FALSE)
  }
  env <- .resolve_env()
  names_all <- ls(envir = env, all.names = FALSE)
  if (!is.null(pattern) && nzchar(pattern)) {
    names_all <- grep(pattern, names_all, value = TRUE)
  }
  recs <- lapply(sort(names_all), function(n) {
    obj <- tryCatch(get(n, envir = env, inherits = FALSE), error = function(e) NULL)
    if (is.null(obj)) {
      return(NULL)
    }
    list(
      name = n,
      type = typeof(obj),
      class = as.character(class(obj)),
      length = length(obj),
      size_bytes = as.numeric(object.size(obj)),
      description = .short_description(obj),
      is_data = is.data.frame(obj) || is.matrix(obj) || is.array(obj)
    )
  })
  list(vars = Filter(Negate(is.null), recs))
}

#' Pretty-printed contents of a variable
#'
#' Mirrors the MCP / CLI surface `env.contents`. Returns the lines that
#' a `print()` of the variable would emit, captured as a character
#' vector. Useful when an agent needs to see the actual values but
#' doesn't want to round-trip them through `r_exec`.
#'
#' @param name Variable name (length-1 character).
#' @return A list with `name` and `contents` (character vector of
#'   formatted lines).
#' @export
env_contents <- function(name) {
  if (!is.character(name) || length(name) != 1L || !nzchar(name)) {
    stop("`name` must be a non-empty length-1 character vector", call. = FALSE)
  }
  env <- .resolve_env()
  if (!exists(name, envir = env, inherits = FALSE)) {
    stop(sprintf("no variable named '%s' in the active environment", name),
         call. = FALSE)
  }
  obj <- get(name, envir = env, inherits = FALSE)
  lines <- utils::capture.output(print(obj))
  list(name = name, contents = as.list(lines))
}

#' Metadata-only inspection of a variable (no contents printed)
#'
#' Mirrors the MCP / CLI surface `env.info`. Returns shape information
#' (class, typeof, length, dim, size in bytes) without ever evaluating
#' the value's print method — safe for very large objects.
#'
#' @param name Variable name (length-1 character).
#' @return A list with `name`, `class`, `typeof`, `length`, `dim`
#'   (`NULL` for atomic vectors), `size_bytes`.
#' @export
env_info <- function(name) {
  if (!is.character(name) || length(name) != 1L || !nzchar(name)) {
    stop("`name` must be a non-empty length-1 character vector", call. = FALSE)
  }
  env <- .resolve_env()
  if (!exists(name, envir = env, inherits = FALSE)) {
    stop(sprintf("no variable named '%s' in the active environment", name),
         call. = FALSE)
  }
  obj <- get(name, envir = env, inherits = FALSE)
  list(
    name = name,
    class = as.character(class(obj)),
    typeof = typeof(obj),
    length = length(obj),
    dim = if (is.null(dim(obj))) NULL else as.integer(dim(obj)),
    size_bytes = as.numeric(object.size(obj))
  )
}

# Pick the environment RStudio's Environment pane is currently showing
# (so env_* honour `attach()`-pushed environments, namespaces, etc.).
# Falls back to globalenv() when the pane setting isn't reachable.
.resolve_env <- function() {
  name <- tryCatch(
    rstudioapi::callFun("get_environment_state")$environment_name,
    error = function(e) ".GlobalEnv"
  )
  if (is.null(name) || identical(name, ".GlobalEnv")) {
    return(globalenv())
  }
  pos <- tryCatch(match(name, search()), error = function(e) NA_integer_)
  if (is.na(pos)) {
    return(globalenv())
  }
  as.environment(pos)
}

# A short one-line summary, similar to what utils::str()'s first line
# emits. Used in env_list to give an agent a hint without dumping the
# whole object. Caps at ~120 chars.
.short_description <- function(x) {
  if (is.function(x)) {
    return(paste0("function(", paste(names(formals(x)), collapse = ", "), ")"))
  }
  if (is.environment(x)) {
    return("<environment>")
  }
  if (is.data.frame(x)) {
    return(sprintf("data.frame [%d x %d]", nrow(x), ncol(x)))
  }
  if (is.matrix(x) || is.array(x)) {
    return(sprintf("%s [%s]", class(x)[[1L]], paste(dim(x), collapse = " x ")))
  }
  # Atomic / list: format first few values, truncate.
  out <- tryCatch(paste(format(utils::head(x, 5L)), collapse = " "),
                  error = function(e) "")
  if (nchar(out) > 120L) {
    out <- paste0(substr(out, 1L, 117L), "...")
  }
  out
}
