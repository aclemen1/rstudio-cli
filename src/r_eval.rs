use serde_json::Value;

use crate::error::CliError;
use crate::rpc::{RpcClient, r_quote};

/// Evaluate user-supplied R code via `execute_r_code`, distinguishing success
/// from R errors. On success returns the captured output (auto-print of visible
/// values + `cat` / `message` / `print` side-effects). On R error returns
/// `CliError::r(message)`.
///
/// The wrapper runs the code under `tryCatch` and prints a status line
/// (`OK\n...` or `ER\n...`) so we can disambiguate; the raw `execute_r_code`
/// channel otherwise swallows R errors silently.
pub fn run(rpc: &RpcClient<'_>, user_code: &str) -> Result<String, CliError> {
    let wrapped = wrap_for_eval(user_code);
    let raw = rpc.rpc("execute_r_code", vec![Value::String(wrapped)])?;
    let raw_str = raw
        .as_str()
        .ok_or_else(|| CliError::internal(format!("execute_r_code returned non-string: {raw}")))?;
    parse_output(raw_str)
}

/// Like `run` but discards the captured output — useful for side-effect-only
/// `rstudioapi` calls (`navigateToFile`, `viewer`, `sourceMarkers`, ...).
pub fn run_silent(rpc: &RpcClient<'_>, user_code: &str) -> Result<(), CliError> {
    run(rpc, user_code).map(|_| ())
}

// R wrapper template. `__ESCAPED__` is replaced with the user code as an R
// double-quoted string literal. The wrapper writes:
//   "OK\n" + captured stdout    on success
//   "ER\n" + condition message  on R error
const WRAPPER_TEMPLATE: &str = r#"local({
  .__r <- tryCatch({
    .__c <- capture.output({
      .__w <- withVisible(eval(parse(text = __ESCAPED__)))
      if (.__w$visible) print(.__w$value)
    })
    paste0("OK\n", paste(.__c, collapse = "\n"))
  }, error = function(e) {
    paste0("ER\n", conditionMessage(e))
  })
  cat(.__r, sep = "")
})"#;

fn wrap_for_eval(user_code: &str) -> String {
    WRAPPER_TEMPLATE.replace("__ESCAPED__", &r_quote(user_code))
}

fn parse_output(raw: &str) -> Result<String, CliError> {
    let (status, payload) = raw.split_once('\n').unwrap_or((raw, ""));
    match status {
        "OK" => Ok(payload.to_string()),
        "ER" => Err(CliError::r(payload.to_string())),
        _ => Err(CliError::internal(format!(
            "execute_r_code returned unexpected format (no OK/ER status line): {raw:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ok_payload() {
        assert_eq!(parse_output("OK\nhello\nworld").unwrap(), "hello\nworld");
    }

    #[test]
    fn parses_empty_ok_payload() {
        assert_eq!(parse_output("OK\n").unwrap(), "");
    }

    #[test]
    fn parses_error_payload() {
        let err = parse_output("ER\nboom").unwrap_err();
        assert_eq!(err.message, "boom");
    }

    #[test]
    fn rejects_unknown_format() {
        let err = parse_output("plain text").unwrap_err();
        assert!(err.message.contains("unexpected format"));
    }

    #[test]
    fn wrapper_inlines_user_code() {
        let wrapped = wrap_for_eval("1 + 1");
        assert!(wrapped.contains("\"1 + 1\""), "user code is quoted as R string");
        assert!(wrapped.contains("tryCatch"));
        assert!(wrapped.contains("withVisible"));
        assert!(wrapped.contains("OK\\n"));
        assert!(wrapped.contains("ER\\n"));
    }
}
