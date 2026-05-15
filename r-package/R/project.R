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

#' Create a NEW directory + RStudio project (destructive if open_after=TRUE)
#'
#' Mirrors the MCP / CLI surface `project.new`. Refuses to act when
#' the target directory already exists (use [project_init()] for that
#' case).
#'
#' @param path Target directory (must not exist).
#' @param scaffold If `TRUE`, also create `R/`, `README.md`, and
#'   `.gitignore` skeletons.
#' @param git If `TRUE`, `git init` the directory.
#' @param open_after If `TRUE` (default), open the project after
#'   creation. **Destructive**: restarts the R session unless
#'   `new_session = TRUE`.
#' @param new_session If `TRUE`, open in a new RStudio window/session
#'   without restarting the current one. Ignored when `open_after = FALSE`.
#' @return A list with `path`, `rproj` (path to the `.Rproj`),
#'   `scaffolded`, `git_initialized`, `opened`.
#' @export
project_new <- function(path,
                        scaffold = FALSE,
                        git = FALSE,
                        open_after = TRUE,
                        new_session = FALSE) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  if (file.exists(path)) {
    stop(sprintf(
      "%s already exists; use project_init() for an existing directory",
      path
    ), call. = FALSE)
  }
  dir.create(path, recursive = TRUE)
  abs_path <- normalizePath(path, mustWork = TRUE)
  rproj <- .write_rproj(abs_path)
  if (isTRUE(scaffold)) {
    .scaffold_dir(abs_path)
  }
  if (isTRUE(git)) {
    .git_init(abs_path)
  }
  opened <- FALSE
  if (isTRUE(open_after)) {
    project_open(path = abs_path, new_session = new_session)
    opened <- TRUE
  }
  list(
    path = abs_path,
    rproj = rproj,
    scaffolded = isTRUE(scaffold),
    git_initialized = isTRUE(git),
    opened = opened
  )
}

#' Make an EXISTING directory an RStudio project (destructive if open_after=TRUE)
#'
#' Mirrors the MCP / CLI surface `project.init`. Refuses to act when
#' the directory already has a `.Rproj` file (no overwrite).
#'
#' @param path Target directory (must exist).
#' @param scaffold If `TRUE`, ALSO create missing `R/`, `README.md`,
#'   `.gitignore` skeletons (existing files are not overwritten).
#' @param git If `TRUE`, run `git init` if the directory is not yet a
#'   git repo.
#' @param open_after,new_session See [project_new()].
#' @return Same shape as [project_new()].
#' @export
project_init <- function(path,
                         scaffold = FALSE,
                         git = FALSE,
                         open_after = TRUE,
                         new_session = FALSE) {
  if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
    stop("`path` must be a non-empty length-1 character vector", call. = FALSE)
  }
  if (!dir.exists(path)) {
    stop(sprintf(
      "%s does not exist or is not a directory; use project_new() to create it",
      path
    ), call. = FALSE)
  }
  abs_path <- normalizePath(path, mustWork = TRUE)
  rproj <- .write_rproj(abs_path)
  if (isTRUE(scaffold)) {
    .scaffold_dir(abs_path)
  }
  if (isTRUE(git) && !dir.exists(file.path(abs_path, ".git"))) {
    .git_init(abs_path)
  }
  opened <- FALSE
  if (isTRUE(open_after)) {
    project_open(path = abs_path, new_session = new_session)
    opened <- TRUE
  }
  list(
    path = abs_path,
    rproj = rproj,
    scaffolded = isTRUE(scaffold),
    git_initialized = isTRUE(git),
    opened = opened
  )
}

#' Clone a git repository and add a `.Rproj` (destructive if open_after=TRUE)
#'
#' Mirrors the MCP / CLI surface `project.clone`. `git clone <url>` into
#' `path` (or auto-derived from the URL), then write a `.Rproj` if the
#' cloned tree doesn't already carry one.
#'
#' @param url Git repository URL (passed verbatim to `git clone`).
#' @param path Optional destination directory. When `NULL`, git's
#'   default is used (the basename of the URL).
#' @param open_after,new_session See [project_new()].
#' @return Same shape as [project_new()] plus `git_initialized = TRUE`
#'   (we cloned, so the result is necessarily a git repo).
#' @export
project_clone <- function(url, path = NULL, open_after = TRUE, new_session = FALSE) {
  if (!is.character(url) || length(url) != 1L || !nzchar(url)) {
    stop("`url` must be a non-empty length-1 character vector", call. = FALSE)
  }
  args <- c("clone", url)
  if (!is.null(path)) {
    if (!is.character(path) || length(path) != 1L || !nzchar(path)) {
      stop("`path` must be NULL or a non-empty length-1 character vector",
           call. = FALSE)
    }
    args <- c(args, path)
  }
  result <- system2("git", args, stdout = TRUE, stderr = TRUE)
  status <- attr(result, "status")
  if (!is.null(status) && status != 0L) {
    stop("git clone failed: ", paste(result, collapse = " | "), call. = FALSE)
  }
  # Determine the resulting directory: if path was given, that's it;
  # otherwise it's the URL's basename minus a trailing .git.
  dest <- if (!is.null(path)) path else sub("\\.git$", "", basename(url))
  abs_path <- normalizePath(dest, mustWork = TRUE)
  rproj_existing <- list.files(abs_path, pattern = "\\.Rproj$", full.names = TRUE)
  rproj <- if (length(rproj_existing) > 0L) {
    rproj_existing[[1L]]
  } else {
    .write_rproj(abs_path)
  }
  opened <- FALSE
  if (isTRUE(open_after)) {
    project_open(path = abs_path, new_session = new_session)
    opened <- TRUE
  }
  list(
    path = abs_path,
    rproj = rproj,
    scaffolded = FALSE,
    git_initialized = TRUE,
    opened = opened
  )
}

# Internal helpers, kept private (underscore prefix doesn't apply in R;
# we use a dot prefix to signal "internal").
.write_rproj <- function(dir) {
  rproj_path <- file.path(dir, paste0(basename(dir), ".Rproj"))
  if (!file.exists(rproj_path)) {
    writeLines(c(
      "Version: 1.0",
      "",
      "RestoreWorkspace: Default",
      "SaveWorkspace: Default",
      "AlwaysSaveHistory: Default",
      "",
      "EnableCodeIndexing: Yes",
      "UseSpacesForTab: Yes",
      "NumSpacesForTab: 2",
      "Encoding: UTF-8",
      "",
      "RnwWeave: Sweave",
      "LaTeX: pdfLaTeX"
    ), rproj_path)
  }
  rproj_path
}

.scaffold_dir <- function(dir) {
  r_dir <- file.path(dir, "R")
  if (!dir.exists(r_dir)) dir.create(r_dir)
  readme <- file.path(dir, "README.md")
  if (!file.exists(readme)) {
    writeLines(c(paste0("# ", basename(dir)), ""), readme)
  }
  gitignore <- file.path(dir, ".gitignore")
  if (!file.exists(gitignore)) {
    writeLines(c(
      ".Rproj.user",
      ".Rhistory",
      ".RData",
      ".Ruserdata"
    ), gitignore)
  }
  invisible(NULL)
}

.git_init <- function(dir) {
  status <- suppressWarnings(system2(
    "git",
    c("-C", shQuote(dir), "init"),
    stdout = FALSE,
    stderr = FALSE
  ))
  if (status != 0L) {
    stop(sprintf("git init failed in %s (exit %d)", dir, status), call. = FALSE)
  }
  invisible(NULL)
}
