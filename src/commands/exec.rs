use std::time::Duration;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval::{self, EvalTimeout};
use crate::rpc::RpcClient;
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

const SOCKET_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "exec",
        name: "run",
        summary: "Exécute du code R en silencieux et retourne sortie + erreurs.",
        description: "Wrappe le code dans un tryCatch + capture.output côté R. \
                      Le code n'apparaît PAS dans la console visible. \
                      Limite serveur 2s par défaut, override avec --timeout.",
        params: &[
            ParamSpec {
                name: "code",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Code R à évaluer (peut être multi-instructions).",
            },
            ParamSpec {
                name: "--timeout",
                kind: ParamKind::Number,
                required: false,
                default: None,
                allowed: &[],
                description: "Limite elapsed en secondes (>0). 0 = pas de limite. \
                              Sans flag : limite serveur 2s.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio exec run '1+1'",
                explanation: "Retourne {output: \"[1] 2\"}.",
            },
            ExampleSpec {
                cmd: "rstudio exec run --timeout 30 'Sys.sleep(10); summary(mtcars)'",
                explanation: "Bypass la limite 2s pour un calcul plus long.",
            },
            ExampleSpec {
                cmd: "rstudio exec run 'stop(\"boom\")'",
                explanation: "Retourne kind=r_error avec message=\"boom\".",
            },
        ],
        returns: "{output: string}",
        errors: &[
            ErrorSpec {
                kind: "r_error",
                when: "Le code R lève une erreur (stop, syntax, etc.).",
            },
            ErrorSpec {
                kind: "timeout",
                when: "Le code dépasse la limite (default 2s ou --timeout).",
            },
        ],
    },
    ActionSpec {
        category: "exec",
        name: "send",
        summary: "Tape du code dans la console R utilisateur et l'exécute (visible).",
        description: "Passe par console_input. L'user voit la commande arriver et son \
                      résultat dans sa console R, comme s'il l'avait tapé. Pas de retour \
                      structuré, fire-and-forget.",
        params: &[
            ParamSpec {
                name: "code",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Code R à envoyer (un newline final est ajouté si absent).",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio exec send 'print(Sys.time())'",
                explanation: "L'user verra `print(Sys.time())` dans sa console et l'exécution.",
            },
        ],
        returns: "void",
        errors: &[],
    },
];

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
