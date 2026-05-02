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
        category: "r",
        name: "exec",
        summary: "Run R code silently and return the captured output and any error.",
        description: "Wraps the user code in a tryCatch + capture.output sent through \
                      execute_r_code. The code does NOT appear in the user's visible \
                      console. Default elapsed limit is the server-imposed 2 s; pass \
                      --timeout to override (or --timeout 0 to disable, see the \
                      concurrency notes).",
        params: &[
            ParamSpec {
                name: "code",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "R code to evaluate (may contain multiple statements).",
            },
            ParamSpec {
                name: "--timeout",
                kind: ParamKind::Number,
                required: false,
                default: None,
                allowed: &[],
                description: "Elapsed-time limit in seconds (>0). 0 = no limit. \
                              Without the flag the rsession server limit (2 s) applies.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio r exec '1+1'",
                explanation: "Returns {output: \"[1] 2\"}.",
            },
            ExampleSpec {
                cmd: "rstudio r exec --timeout 30 'Sys.sleep(10); summary(mtcars)'",
                explanation: "Bypasses the 2 s limit for a longer computation.",
            },
            ExampleSpec {
                cmd: "rstudio r exec 'stop(\"boom\")'",
                explanation: "Returns kind=r_error with message=\"boom\".",
            },
        ],
        returns: "{output: string}",
        errors: &[
            ErrorSpec {
                kind: "r_error",
                when: "The R code raised a condition (stop, syntax error, ...).",
            },
            ErrorSpec {
                kind: "timeout",
                when: "The code exceeded the limit (default 2 s or --timeout).",
            },
        ],
        rstudioapi_fn: None,
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "r",
        name: "send",
        summary: "Type R code into the user's console and execute it (visible).",
        description: "Uses the console_input RPC. The user sees the command appear \
                      and run, exactly as if they had typed it. Fire-and-forget; no \
                      structured return value.",
        params: &[ParamSpec {
            name: "code",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "R code to send (a trailing newline is appended if absent).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio r send 'print(Sys.time())'",
            explanation: "User sees print(Sys.time()) typed into their R console and executed.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("console_input"),
    },
];

#[derive(Subcommand, Debug)]
pub enum RCmd {
    /// Run R code silently (capture output and errors; not visible in the user's console).
    Exec {
        code: String,
        /// Elapsed-time limit in seconds. 0 = no limit.
        #[arg(long, short = 't')]
        timeout: Option<f64>,
    },
    /// Send R code to the user's console (visible) and execute it.
    Send { code: String },
}

pub fn run(cmd: &RCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        RCmd::Exec { code, timeout } => {
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
        RCmd::Send { code } => {
            let mut text = code.clone();
            if !text.ends_with('\n') {
                text.push('\n');
            }
            rpc.rpc(
                "console_input",
                vec![Value::String(text), Value::String(String::new()), json!(0)],
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
