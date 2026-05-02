use std::cell::RefCell;
use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::client_id::read_active_client_id;
use crate::error::CliError;
use crate::session::Session;
use crate::socket::{HttpResponse, request};

const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const RPC_INVALID_CLIENT_ID: i32 = 4;

pub struct RpcClient<'a> {
    session: &'a Session,
    timeout: Duration,
    cached_client_id: RefCell<Option<String>>,
}

impl<'a> RpcClient<'a> {
    pub fn new(session: &'a Session) -> Self {
        Self::with_timeout(session, DEFAULT_RPC_TIMEOUT)
    }

    pub fn with_timeout(session: &'a Session, timeout: Duration) -> Self {
        Self {
            session,
            timeout,
            cached_client_id: RefCell::new(None),
        }
    }

    fn auth_headers(&self, csrf: &str) -> [(String, String); 4] {
        [
            ("X-Session-Postback".into(), "1".into()),
            ("X-RStudioUserIdentity".into(), self.session.user.clone()),
            ("X-RS-CSRF-Token".into(), csrf.into()),
            (
                "Cookie".into(),
                format!("rs-csrf-token={csrf}; csrf-token={csrf}"),
            ),
        ]
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
            self.session.socket(),
            "POST",
            &path,
            &header_refs,
            body.as_bytes(),
            self.timeout,
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
    pub fn rpc(&self, method: &str, params: Vec<Value>) -> Result<Value, CliError> {
        let client_id = self.client_id(false)?;
        match self.rpc_with_client_id(method, &params, &client_id) {
            Err(e) if e.code == RPC_INVALID_CLIENT_ID => {
                let refreshed = self.client_id(true)?;
                self.rpc_with_client_id(method, &params, &refreshed)
            }
            other => other,
        }
    }

    fn client_id(&self, force_refresh: bool) -> Result<String, CliError> {
        if !force_refresh {
            if let Some(id) = self.cached_client_id.borrow().clone() {
                return Ok(id);
            }
        }
        let state_path = self.session.require_state_path()?;
        let id = read_active_client_id(state_path)?;
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
            self.session.socket(),
            "POST",
            &path,
            &header_refs,
            body.as_bytes(),
            self.timeout,
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
        return Err(CliError::rpc(code, format!("jsonrpc error {code} ({message})")));
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
