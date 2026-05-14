use std::cell::{Cell, RefCell};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::client_id::resolve_client_id;
use crate::error::CliError;
use crate::session::{Mode, Session};
use crate::transport::{HttpResponse, request};

const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const RPC_INVALID_CLIENT_ID: i32 = 4;
const RPC_INVALID_REQUEST: i32 = 6;

pub struct RpcClient<'a> {
    session: &'a Session,
    timeout: Cell<Option<Duration>>,
    cached_client_id: RefCell<Option<String>>,
}

impl<'a> RpcClient<'a> {
    pub fn new(session: &'a Session) -> Self {
        Self {
            session,
            timeout: Cell::new(Some(DEFAULT_RPC_TIMEOUT)),
            cached_client_id: RefCell::new(None),
        }
    }

    /// Set the read timeout used for subsequent calls. `None` disables the
    /// timeout entirely — callers must ensure they're prepared to block
    /// indefinitely. Returns the previous value.
    pub fn set_timeout(&self, timeout: Option<Duration>) -> Option<Duration> {
        self.timeout.replace(timeout)
    }

    fn auth_headers(&self, csrf: &str) -> Vec<(String, String)> {
        let mut headers = vec![
            ("X-Session-Postback".into(), "1".into()),
            ("X-RStudioUserIdentity".into(), self.session.user.clone()),
            ("X-RS-CSRF-Token".into(), csrf.into()),
            (
                "Cookie".into(),
                format!("rs-csrf-token={csrf}; csrf-token={csrf}"),
            ),
        ];
        // Desktop authenticates by shared secret instead of SO_PEERCRED. The
        // header is the only thing the listener checks; the Server-style
        // headers above are harmless on Desktop (the listener ignores them).
        if self.session.mode == Mode::Desktop
            && let Some(secret) = self.session.shared_secret.as_ref()
        {
            headers.push(("X-Shared-Secret".into(), secret.clone()));
        }
        headers
    }

    /// Postbacks don't carry a clientId — random CSRF is enough. They also don't
    /// touch the active browser client, so they're safe regardless of session state.
    pub fn postback(&self, cmd: &str, body: &str) -> Result<PostbackResponse, CliError> {
        let csrf = Uuid::new_v4().to_string();
        let auth = self.auth_headers(&csrf);
        let header_refs: Vec<(&str, &str)> = auth
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(std::iter::once(("Content-Type", "text/plain")))
            .collect();

        let path = format!("/rsession-local/postback/{cmd}");
        let resp = request(
            &self.session.transport,
            "POST",
            &path,
            &header_refs,
            body.as_bytes(),
            self.timeout.get(),
        )
        .map_err(|e| CliError::rpc(0, format!("socket error during postback {cmd}: {e:#}")))?;

        if resp.status != 200 {
            return Err(CliError::rpc(
                resp.status as i32,
                format!("postback {cmd} returned HTTP {}", resp.status),
            ));
        }

        let exit_code = resp
            .header("X-Postback-ExitCode")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Ok(PostbackResponse {
            exit_code,
            body: String::from_utf8_lossy(&resp.body).into_owned(),
        })
    }

    /// JSON-RPC call. Reads the active browser client's id from the session state file
    /// and uses it as `clientId` so events are routed to the user's open tab.
    /// On `Invalid client id` (code 4), re-reads the file once and retries.
    /// On code 6 (`Invalid json-rpc request`), waits briefly and retries once —
    /// speculative fix for the race observed when the user clicks a marker in
    /// RStudio's Markers pane right before this call arrives (the click triggers a
    /// browser→rsession state transition that causes transient code-6 rejections).
    /// Remove if the root cause is identified via rsession source inspection.
    pub fn rpc(&self, method: &str, params: Vec<Value>) -> Result<Value, CliError> {
        // Ensure the `rstudiocli.mcp` companion R package is installed
        // in the active rsession before any RPC that may execute R
        // code referencing it. Memoised per-process via `OnceLock` so
        // the first call within a process pays at most one round-trip
        // (version check) plus the install if needed, and subsequent
        // calls are free. Re-entrant: ensure_installed itself calls
        // RPCs, but it sets the OnceLock before doing so, so nested
        // calls short-circuit.
        crate::r_package::ensure_installed(self)?;

        let client_id = self.client_id(false)?;
        match self.rpc_with_client_id(method, &params, &client_id) {
            Err(e) if e.code == RPC_INVALID_CLIENT_ID => {
                let refreshed = self.client_id(true)?;
                self.rpc_with_client_id(method, &params, &refreshed)
            }
            Err(e) if e.code == RPC_INVALID_REQUEST => {
                thread::sleep(Duration::from_millis(200));
                let refreshed = self.client_id(true)?;
                self.rpc_with_client_id(method, &params, &refreshed)
            }
            other => other,
        }
    }

    fn client_id(&self, force_refresh: bool) -> Result<String, CliError> {
        if !force_refresh && let Some(id) = self.cached_client_id.borrow().clone() {
            return Ok(id);
        }
        let id = resolve_client_id(self.session)?;
        *self.cached_client_id.borrow_mut() = Some(id.clone());
        Ok(id)
    }

    fn rpc_with_client_id(
        &self,
        method: &str,
        params: &[Value],
        client_id: &str,
    ) -> Result<Value, CliError> {
        let csrf = Uuid::new_v4().to_string();
        let body = json!({
            "method": method,
            "params": params,
            "id": 1,
            "clientId": client_id,
        })
        .to_string();

        let auth = self.auth_headers(&csrf);
        let header_refs: Vec<(&str, &str)> = auth
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(std::iter::once(("Content-Type", "application/json")))
            .collect();

        let path = format!("/rpc/{method}");
        let resp = request(
            &self.session.transport,
            "POST",
            &path,
            &header_refs,
            body.as_bytes(),
            self.timeout.get(),
        )
        .map_err(|e| CliError::rpc(0, format!("socket error during rpc {method}: {e:#}")))?;

        parse_rpc_envelope(method, &resp)
    }
}

fn parse_rpc_envelope(method: &str, resp: &HttpResponse) -> Result<Value, CliError> {
    if resp.status != 200 {
        return Err(CliError::rpc(
            resp.status as i32,
            format!("rpc {method} returned HTTP {}", resp.status),
        ));
    }

    let envelope: Value = serde_json::from_slice(&resp.body).map_err(|e| {
        let preview = String::from_utf8_lossy(&resp.body);
        let preview = preview.chars().take(200).collect::<String>();
        CliError::rpc(
            0,
            format!("rpc {method}: invalid JSON envelope: {e}; body starts with: {preview}"),
        )
    })?;

    if let Some(err) = envelope.get("error") {
        let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown rpc error")
            .to_string();
        if code == 100 {
            let detail = err
                .get("error")
                .and_then(|v| v.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or(&message)
                .to_string();
            return Err(CliError::r(detail));
        }
        return Err(CliError::rpc(
            code,
            format!("jsonrpc error {code} ({message})"),
        ));
    }

    // Desktop's TCP listener returns an asyncHandle when an RPC is queued
    // behind a busy R session: the actual result is later delivered through
    // a kAsyncCompletion event on /events/get_events, keyed on the desktop
    // client id. The CLI does not poll that event channel (and cannot mint
    // its own client id since `client_init` is blacklisted, see
    // src/commands/raw.rs), so we surface a clean session_unavailable
    // instead of falling through to a Value::Null result that downstream
    // callers like r_eval would later reject as "non-string: null". Server's
    // unix-socket listener never takes this path (it keeps the HTTP
    // response open until the result is ready), so this branch only fires
    // on Desktop. See DESKTOP_TEST_RESULTS.md "B1 — wire capture".
    if let Some(handle) = envelope.get("asyncHandle").and_then(|v| v.as_str()) {
        return Err(CliError::session(format!(
            "Desktop rsession queued this {method} call \
             (asyncHandle={handle}); the CLI does not poll the \
             kAsyncCompletion event channel. Serialise r exec calls \
             externally, or wait for async support to land. Server is \
             unaffected."
        )));
    }

    Ok(envelope.get("result").cloned().unwrap_or(Value::Null))
}

#[derive(Debug)]
pub struct PostbackResponse {
    pub exit_code: i32,
    pub body: String,
}

pub fn r_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn resp_with_body(body: &str) -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn parses_plain_result() {
        let resp = resp_with_body(r#"{"result":"42","ep":"false"}"#);
        let value = parse_rpc_envelope("execute_r_code", &resp).expect("ok");
        assert_eq!(value, Value::String("42".into()));
    }

    #[test]
    fn surfaces_async_handle_as_session_unavailable() {
        let resp = resp_with_body(
            r#"{"asyncHandle":"22e9ffe3-b62a-41c1-9909-f7f883cca9fc","ep":"false"}"#,
        );
        let err = parse_rpc_envelope("execute_r_code", &resp)
            .expect_err("asyncHandle must be rejected, not silently null");
        assert!(matches!(err.kind, ErrorKind::SessionUnavailable));
        assert!(
            err.message.contains("22e9ffe3-b62a-41c1-9909-f7f883cca9fc"),
            "message must name the handle, got: {}",
            err.message
        );
        assert!(
            err.message.contains("execute_r_code"),
            "message must name the method, got: {}",
            err.message
        );
    }

    #[test]
    fn rpc_error_still_surfaces_as_rpc_error() {
        let resp = resp_with_body(
            r#"{"error":{"code":4,"message":"jsonrpc error 4 (Invalid client id)","error":null}}"#,
        );
        let err = parse_rpc_envelope("execute_r_code", &resp).expect_err("err");
        assert!(matches!(err.kind, ErrorKind::RpcError));
        assert_eq!(err.code, 4);
    }

    #[test]
    fn r_error_still_surfaces_as_r_error() {
        let resp = resp_with_body(
            r#"{"error":{"code":100,"message":"R eval error","error":{"message":"intentional"}}}"#,
        );
        let err = parse_rpc_envelope("execute_r_code", &resp).expect_err("err");
        assert!(matches!(err.kind, ErrorKind::RError));
        assert_eq!(err.message, "intentional");
    }

    // Fix #2: the race-condition retry is triggered on code 6.
    // parse_rpc_envelope must surface code 6 as an RpcError (not swallowed)
    // so that RpcClient::rpc() can match on it.
    #[test]
    fn rpc_error_6_surfaces_as_rpc_error() {
        let resp = resp_with_body(
            r#"{"error":{"code":6,"message":"Invalid json-rpc request","error":null}}"#,
        );
        let err = parse_rpc_envelope("execute_r_code", &resp).expect_err("err");
        assert!(matches!(err.kind, ErrorKind::RpcError));
        assert_eq!(err.code, 6);
        assert!(err.message.contains("6"));
    }
}
