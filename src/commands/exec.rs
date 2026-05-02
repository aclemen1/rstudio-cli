use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::rpc::RpcClient;

#[derive(Subcommand, Debug)]
pub enum ExecCmd {
    /// Exécute du code R en silencieux (capture sortie et erreurs, pas visible
    /// dans la console utilisateur). Limité à 2s côté serveur.
    Run {
        /// Code R à exécuter.
        code: String,
    },
    /// Envoie le code à la console R utilisateur (visible) et l'exécute.
    Send {
        /// Code R à envoyer à la console.
        code: String,
    },
}

pub fn run(cmd: &ExecCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        ExecCmd::Run { code } => {
            let v = rpc.rpc("execute_r_code", vec![Value::String(code.clone())])?;
            Ok(Some(json!({ "output": v })))
        }
        ExecCmd::Send { code } => {
            let mut text = code.clone();
            if !text.ends_with('\n') {
                text.push('\n');
            }
            rpc.rpc(
                "console_input",
                vec![
                    Value::String(text),
                    Value::String(String::new()),
                    json!(0),
                ],
            )?;
            Ok(None)
        }
    }
}
