use std::time::Duration;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval::{self, EvalTimeout};
use crate::rpc::RpcClient;

const SOCKET_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);

#[derive(Subcommand, Debug)]
pub enum ExecCmd {
    /// Exécute du code R en silencieux (capture sortie et erreurs, pas visible
    /// dans la console utilisateur).
    ///
    /// Sans --timeout, la limite d'évaluation est celle imposée par le serveur
    /// (2s d'elapsed time). Passer --timeout pour la dépasser.
    Run {
        /// Code R à exécuter.
        code: String,
        /// Limite d'evaluation en secondes (float). 0 = pas de limite (le CLI
        /// bloque jusqu'à la fin du code, Ctrl-C reste possible).
        #[arg(long, short = 't')]
        timeout: Option<f64>,
    },
    /// Envoie le code à la console R utilisateur (visible) et l'exécute.
    Send {
        /// Code R à envoyer à la console.
        code: String,
    },
}

pub fn run(cmd: &ExecCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        ExecCmd::Run { code, timeout } => {
            let eval_timeout = match timeout {
                None => EvalTimeout::ServerDefault,
                Some(t) if *t <= 0.0 => EvalTimeout::NoLimit,
                Some(t) if !t.is_finite() => EvalTimeout::NoLimit,
                Some(t) => EvalTimeout::Limit(*t),
            };
            apply_socket_timeout(rpc, &eval_timeout);
            let output = r_eval::run_with_timeout(rpc, code, eval_timeout)?;
            Ok(Some(json!({ "output": output })))
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

fn apply_socket_timeout(rpc: &RpcClient<'_>, eval_timeout: &EvalTimeout) {
    match eval_timeout {
        EvalTimeout::ServerDefault => {} // keep RpcClient default
        EvalTimeout::NoLimit => {
            rpc.set_timeout(None);
        }
        EvalTimeout::Limit(secs) => {
            let wall = Duration::from_secs_f64(*secs) + SOCKET_TIMEOUT_MARGIN;
            rpc.set_timeout(Some(wall));
        }
    }
}
