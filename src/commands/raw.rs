use clap::Args;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::rpc::RpcClient;

#[derive(Args, Debug)]
pub struct RpcCmd {
    /// Nom de la méthode JSON-RPC (ex: console_input, get_environment_state).
    pub method: String,
    /// Paramètres comme tableau JSON. Ex: --params '["1+1"]' ou '[{"path": "x"}]'.
    #[arg(long, default_value = "[]")]
    pub params: String,
}

#[derive(Args, Debug)]
pub struct PostbackCmd {
    /// Nom du postback (ex: editfile, browser, pdfviewer).
    pub command: String,
    /// Corps brut envoyé en text/plain.
    pub body: String,
}

pub fn run_rpc(cmd: &RpcCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
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
