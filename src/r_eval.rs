use serde_json::Value;

use crate::error::CliError;
use crate::rpc::{RpcClient, r_quote};

const TIMEOUT_MARKER: &str = "reached elapsed time limit";

/// Maximum elapsed time allowed for the R evaluation.
#[derive(Debug, Clone, Copy)]
pub enum EvalTimeout {
    /// Don't inject `setTimeLimit` — keep the server-imposed default (2s).
    ServerDefault,
    /// Inject `setTimeLimit(elapsed = secs, transient = TRUE)` before the user code.
    Limit(f64),
    /// Inject `setTimeLimit(elapsed = Inf, transient = TRUE)`. Caller is
    /// responsible for bumping the socket timeout (likely to `None`).
    NoLimit,
}

/// Evaluate user-supplied R code via `execute_r_code`, distinguishing success
/// from R errors. On success returns the captured output (auto-print of visible
/// values + `cat` / `message` / `print` side-effects). On R error returns
/// `CliError::r(message)`, or `CliError::timeout(...)` when the server's
/// elapsed-time limit was hit.
pub fn run(rpc: &RpcClient<'_>, user_code: &str) -> Result<String, CliError> {
    run_with_timeout(rpc, user_code, EvalTimeout::ServerDefault)
}

pub fn run_with_timeout(
    rpc: &RpcClient<'_>,
    user_code: &str,
    timeout: EvalTimeout,
) -> Result<String, CliError> {
    let wrapped = wrap_for_eval(user_code, timeout);
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

const WRAPPER_TEMPLATE: &str = r#"local({
  __TIMEOUT_SETUP__
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

fn wrap_for_eval(user_code: &str, timeout: EvalTimeout) -> String {
    let setup = match timeout {
        EvalTimeout::ServerDefault => String::new(),
        EvalTimeout::Limit(secs) => {
            format!("setTimeLimit(elapsed = {secs}, transient = TRUE)")
        }
        EvalTimeout::NoLimit => "setTimeLimit(elapsed = Inf, transient = TRUE)".to_string(),
    };
    WRAPPER_TEMPLATE
        .replace("__TIMEOUT_SETUP__", &setup)
        .replace("__ESCAPED__", &r_quote(user_code))
}

fn parse_output(raw: &str) -> Result<String, CliError> {
    let (status, payload) = raw.split_once('\n').unwrap_or((raw, ""));
    match status {
        "OK" => Ok(payload.to_string()),
        "ER" if payload == TIMEOUT_MARKER => Err(CliError::timeout(format!(
            "R evaluation exceeded elapsed time limit (default 2s; pass --timeout to override)"
        ))),
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
        assert!(matches!(err.kind, crate::error::ErrorKind::RError));
    }

    #[test]
    fn maps_timeout_marker_to_timeout_kind() {
        let err = parse_output(&format!("ER\n{TIMEOUT_MARKER}")).unwrap_err();
        assert!(matches!(err.kind, crate::error::ErrorKind::Timeout));
    }

    #[test]
    fn rejects_unknown_format() {
        let err = parse_output("plain text").unwrap_err();
        assert!(err.message.contains("unexpected format"));
    }

    #[test]
    fn wrapper_with_server_default_has_no_setup() {
        let wrapped = wrap_for_eval("1 + 1", EvalTimeout::ServerDefault);
        assert!(!wrapped.contains("setTimeLimit"));
        assert!(wrapped.contains("\"1 + 1\""));
    }

    #[test]
    fn wrapper_with_limit_injects_setTimeLimit() {
        let wrapped = wrap_for_eval("Sys.sleep(5)", EvalTimeout::Limit(10.0));
        assert!(wrapped.contains("setTimeLimit(elapsed = 10, transient = TRUE)"));
    }

    #[test]
    fn wrapper_with_no_limit_uses_inf() {
        let wrapped = wrap_for_eval("x", EvalTimeout::NoLimit);
        assert!(wrapped.contains("setTimeLimit(elapsed = Inf, transient = TRUE)"));
    }
}
