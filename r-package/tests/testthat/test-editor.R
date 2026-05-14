# Input-validation tests that don't require a live RStudio session.
# Behaviour against a real rsession is covered by tests/live.rs on the
# Rust side (the same RStudio is exercised by both sides).

test_that("editor_set_contents validates `text`", {
  expect_error(editor_set_contents(123), "must be a length-1 character")
  expect_error(editor_set_contents(c("a", "b")), "must be a length-1 character")
  expect_error(editor_set_contents(NULL), "must be a length-1 character")
})

test_that("editor_open validates `path`", {
  expect_error(editor_open(123), "non-empty length-1 character")
  expect_error(editor_open(""), "non-empty length-1 character")
  expect_error(editor_open(c("a", "b")), "non-empty length-1 character")
})

test_that("editor_open validates `line`", {
  # A path that exists so we get past the path check.
  tmp <- tempfile(fileext = ".R")
  writeLines("1 + 1", tmp)
  on.exit(unlink(tmp))
  expect_error(editor_open(tmp, line = -1), "positive integer")
  expect_error(editor_open(tmp, line = "ten"), "positive integer")
  expect_error(editor_open(tmp, line = c(1, 2)), "positive integer")
})

test_that("editor_open errors on nonexistent file", {
  # normalizePath(mustWork = TRUE) will throw a base-R error.
  expect_error(
    editor_open("/no/such/path/qzqzqz.R"),
    "No such file|cannot find"
  )
})
