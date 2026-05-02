use clap::Args;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::rpc::RpcClient;

#[derive(Args, Debug)]
pub struct RpcCmd {
    /// JSON-RPC method name (e.g. console_input, get_environment_state).
    pub method: String,
    /// Parameters as a JSON array. Ex: --params '["1+1"]' or '[{"path": "x"}]'.
    #[arg(long, default_value = "[]")]
    pub params: String,
}

#[derive(Args, Debug)]
pub struct PostbackCmd {
    /// Postback name (e.g. editfile, browser, pdfviewer).
    pub command: String,
    /// Raw body sent as text/plain.
    pub body: String,
}

/// Methods that must never be invoked through the raw escape hatch — calling
/// them invalidates the user's browser client and forces a session reload.
const FORBIDDEN_METHODS: &[&str] = &["client_init"];

pub fn run_rpc(cmd: &RpcCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    if FORBIDDEN_METHODS.contains(&cmd.method.as_str()) {
        return Err(CliError::user(format!(
            "rpc method '{}' is blacklisted: it would invalidate the active browser client \
             and reset the user's RStudio session. There is no diagnostic value in calling it.",
            cmd.method
        )));
    }
    let params: Value = serde_json::from_str(&cmd.params)
        .map_err(|e| CliError::user(format!("--params is not valid JSON: {e}")))?;
    let params = match params {
        Value::Array(a) => a,
        other => {
            return Err(CliError::user(format!(
                "--params must be a JSON array, got: {other}"
            )));
        }
    };
    let result = rpc.rpc(&cmd.method, params)?;
    Ok(Some(result))
}

pub fn run_postback(cmd: &PostbackCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let resp = rpc.postback(&cmd.command, &cmd.body)?;
    Ok(Some(json!({
        "exit_code": resp.exit_code,
        "body": resp.body,
    })))
}
