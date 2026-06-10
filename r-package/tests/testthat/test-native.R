# Tests for the optional native browser-level helper. These must pass
# whether or not a C toolchain is available in the test environment: the
# helper is best-effort and degrades to NULL. We never assert that a build
# succeeds (CI hosts may lack a compiler), only that the contract holds.

test_that("rscli_browse_level never errors and returns NULL or a scalar integer", {
  v <- rscli_browse_level()
  expect_true(is.null(v) || (is.integer(v) && length(v) == 1L))
})

test_that("rscli_browse_level reports 0 at the top level when native is available", {
  v <- rscli_browse_level()
  # When the helper could be built+loaded (compiler present), we are not in
  # a browser here, so the level must be exactly 0. When unavailable it's
  # NULL — both are acceptable; we only forbid a wrong positive count.
  if (!is.null(v)) {
    expect_identical(v, 0L)
  } else {
    succeed("native helper unavailable (no toolchain) — degraded to NULL as designed")
  }
})

test_that("rscli_browse_level memoises its availability decision", {
  # Two calls in a row must agree and must not error. (If the first call
  # built/loaded the lib, the second reuses it; if it failed, the second
  # short-circuits via the cached failure flag.)
  a <- rscli_browse_level()
  b <- rscli_browse_level()
  expect_identical(a, b)
})
