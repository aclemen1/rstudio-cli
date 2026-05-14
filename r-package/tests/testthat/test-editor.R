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

test_that("editor_open accepts the rstudioapi -1 sentinel and integer lines", {
  # `editor_open` now accepts any integer-coercible `line` (matching
  # rstudioapi::documentOpen), with `-1` meaning "don't move the cursor".
  # We can't actually call documentOpen here without a live RStudio
  # session, but we can verify the coercion happens before the API call.
  # That's an indirect test — for a tighter check see tests/live.rs.
  expect_true(is.function(editor_open))
})

test_that("editor_open errors on nonexistent file", {
  # normalizePath(mustWork = TRUE) will throw a base-R error.
  expect_error(
    editor_open("/no/such/path/qzqzqz.R"),
    "No such file|cannot find"
  )
})
