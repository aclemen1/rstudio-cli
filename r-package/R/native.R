# Native helper: the R browser nesting level (the N of "Browse[N]>").
#
# R does not expose this value to the language layer (browser() is a
# .Primitive; sys.calls()/sys.nframe() don't reflect browser nesting), and
# rsession reduces the prompt string to a boolean. The only way to recover N
# is a tiny C walk of R's context stack — see inst/native/browse_level.c.
#
# We ship the .c as a data file (inst/native/), NOT as a package src/ unit, so
# the package installs everywhere (no toolchain required at install time).
# This module compiles it lazily, on first use, ONLY if a C toolchain is
# available, caches the resulting shared object under R_user_dir(..., "cache")
# keyed by R version + arch, dyn.load()s it, and calls it. Every step is
# best-effort and tryCatch-guarded: if anything is missing or fails, we return
# NULL and the caller degrades to "browse_level unavailable".

# Per-session state, so we attempt compilation at most once and remember the
# loaded symbol / failure.
.rscli_native <- new.env(parent = emptyenv())

#' R browser nesting level (the N of `Browse[N]>`)
#'
#' Returns the number of active `browser()` contexts on R's interpreter
#' stack — the same N shown in the `Browse[N]>` prompt — by walking R's
#' context stack in C (see `inst/native/browse_level.c`). Returns `NULL` when
#' the native helper is unavailable (no C toolchain to build it, or a build /
#' load failure). Returns `0L` when no browser is active.
#'
#' This is best-effort and never errors: all failure modes collapse to `NULL`.
#'
#' @return Integer browser level (`0L` at top level, `>= 1L` inside a
#'   browser), or `NULL` if the native helper could not be built or loaded.
#' @export
rscli_browse_level <- function() {
  sym <- .rscli_browse_level_symbol()
  if (is.null(sym)) {
    return(NULL)
  }
  tryCatch(
    .Call(sym),
    error = function(e) NULL
  )
}

# Resolve (compiling + loading on first use) the native routine. Returns a
# NativeSymbolInfo usable with .Call(), or NULL if unavailable. Memoised.
.rscli_browse_level_symbol <- function() {
  if (!is.null(.rscli_native$symbol)) {
    return(.rscli_native$symbol)
  }
  if (isTRUE(.rscli_native$failed)) {
    return(NULL)
  }
  sym <- tryCatch(.rscli_build_and_load(), error = function(e) NULL)
  if (is.null(sym)) {
    .rscli_native$failed <- TRUE # don't retry every call this session
    return(NULL)
  }
  .rscli_native$symbol <- sym
  sym
}

.rscli_build_and_load <- function() {
  src <- system.file("native", "browse_level.c", package = "rstudiocli")
  if (!nzchar(src) || !file.exists(src)) {
    return(NULL)
  }

  # Cache the built shared object, keyed by R version + arch so an R upgrade
  # (or a different arch on a shared home dir) recompiles cleanly.
  key <- paste0("R", getRversion(), "-", R.version$arch)
  cache_dir <- file.path(
    tools::R_user_dir("rstudio-cli", "cache"), "native", key
  )
  so <- file.path(cache_dir, paste0("browse_level", .Platform$dynlib.ext))

  if (!file.exists(so)) {
    if (!.rscli_have_compiler()) {
      return(NULL)
    }
    dir.create(cache_dir, recursive = TRUE, showWarnings = FALSE)
    # Compile inside the cache dir so the .o / .so land beside a private copy
    # of the source (never write into the installed package tree).
    csrc <- file.path(cache_dir, "browse_level.c")
    if (!file.copy(src, csrc, overwrite = TRUE)) {
      return(NULL)
    }
    ok <- .rscli_compile(csrc)
    if (!isTRUE(ok) || !file.exists(so)) {
      return(NULL)
    }
  }

  # dyn.load is idempotent across calls but we guard so a second session-level
  # call doesn't reload. getNativeSymbolInfo scoped to this DLL avoids the
  # ambiguity warning .Call("name") would raise when multiple DLLs are loaded.
  dll <- tryCatch(dyn.load(so), error = function(e) NULL)
  if (is.null(dll)) {
    return(NULL)
  }
  tryCatch(
    getNativeSymbolInfo("rscli_browse_level", PACKAGE = dll),
    error = function(e) NULL
  )
}

# Is a C compiler plausibly available? We check the CC that R itself would use
# (via `R CMD config CC`), falling back to common names. Cheap and best-effort;
# the real test is the compile itself, whose failure we also tolerate.
.rscli_have_compiler <- function() {
  cc <- tryCatch(
    {
      out <- suppressWarnings(system2(
        file.path(R.home("bin"), "R"),
        c("CMD", "config", "CC"),
        stdout = TRUE, stderr = FALSE
      ))
      paste(out, collapse = " ")
    },
    error = function(e) ""
  )
  # CC can be e.g. "clang -std=gnu23" or "gcc"; take the program name.
  prog <- sub("\\s.*$", "", trimws(cc))
  if (!nzchar(prog)) {
    prog <- "cc"
  }
  nzchar(Sys.which(prog))
}

# Compile `csrc` to a shared object via `R CMD SHLIB` (uses R's own Makeconf,
# so the flags and the libR link are correct). Returns TRUE on success.
.rscli_compile <- function(csrc) {
  status <- tryCatch(
    suppressWarnings(system2(
      file.path(R.home("bin"), "R"),
      c("CMD", "SHLIB", shQuote(csrc)),
      stdout = FALSE, stderr = FALSE
    )),
    error = function(e) 1L
  )
  identical(as.integer(status), 0L)
}
