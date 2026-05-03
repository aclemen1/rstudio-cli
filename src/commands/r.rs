use std::time::Duration;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval::{self, EvalTimeout};
use crate::rpc::{RpcClient, r_quote};
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
                      --timeout to override (or --timeout 0 to disable). \
                      Pass --async to launch via callr::r_bg() and return immediately \
                      with a job {id}; use `r poll <id>` to retrieve the result. \
                      The background process runs in a fresh R environment — \
                      packages must be loaded inside the code argument.",
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
                              Without the flag the rsession server limit (2 s) applies. \
                              Ignored when --async is set.",
            },
            ParamSpec {
                name: "--async",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Launch via callr::r_bg() and return immediately. \
                              Requires the callr package. Use `r poll <id>` to check status.",
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
                cmd: "rstudio r exec --async 'Sys.sleep(5); paste(\"done\", Sys.time())'",
                explanation: "Returns {id, status:\"running\"} immediately; poll for result.",
            },
            ExampleSpec {
                cmd: "rstudio r exec 'stop(\"boom\")'",
                explanation: "Returns kind=r_error with message=\"boom\".",
            },
        ],
        returns: "{output: string} or {id: string, status: \"running\"} when --async",
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
        name: "poll",
        summary: "Check the status of a background R job started with `r exec --async`.",
        description: "Looks up the job handle stored in the R session's \
                      .rstudio_cli_async_jobs environment. Returns {status: \"running\"} \
                      if still alive, {status: \"done\", output} on success, or \
                      {status: \"error\", message, stderr} on failure. \
                      The job entry is removed once done or errored.",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Job ID returned by `r exec --async`.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio r poll job_20260503_123456_789",
            explanation: "Returns {id, status, output} or {id, status: \"running\"}.",
        }],
        returns: "{id: string, status: \"running\"|\"done\"|\"error\", \
                  output?: string, message?: string, stderr?: string}",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Job ID not found in the active R session.",
        }],
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
            description: "R code to send. Pass it without a trailing newline — \
                          rsession adds the one that triggers execution.",
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
        /// Elapsed-time limit in seconds. 0 = no limit. Ignored with --async.
        #[arg(long, short = 't')]
        timeout: Option<f64>,
        /// Launch via callr::r_bg() and return immediately with a job id.
        #[arg(long)]
        r#async: bool,
    },
    /// Check the status of a background R job started with `r exec --async`.
    Poll { id: String },
    /// Send R code to the user's console (visible) and execute it.
    Send { code: String },
}

pub fn run(cmd: &RCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        RCmd::Exec {
            code,
            timeout,
            r#async,
        } => {
            if *r#async {
                return exec_async(rpc, code);
            }
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
        RCmd::Poll { id } => poll_async(rpc, id),
        RCmd::Send { code } => {
            // Do NOT append a trailing '\n' — rsession's console_input
            // already terminates the input with a newline before pushing it
            // to the R input queue. Appending our own would inject a blank
            // line between the typed command and its output.
            rpc.rpc(
                "console_input",
                vec![
                    Value::String(code.clone()),
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

fn exec_async(rpc: &RpcClient<'_>, code: &str) -> Result<Option<Value>, CliError> {
    let code_quoted = r_quote(code);
    let r_code = format!(
        r#"local({{
  if (!requireNamespace("callr", quietly = TRUE)) {{
    stop("callr is required for async execution. Install with: install.packages('callr')")
  }}
  if (!exists(".rstudio_cli_async_jobs", envir = globalenv())) {{
    assign(".rstudio_cli_async_jobs", new.env(parent = emptyenv()), envir = globalenv())
  }}
  jobs <- get(".rstudio_cli_async_jobs", envir = globalenv())
  job_id <- paste0("job_", format(Sys.time(), "%Y%m%d%H%M%OS3"), "_", sample.int(999999L, 1L))
  proc <- callr::r_bg(
    function(code) {{
      out <- capture.output(eval(parse(text = code), envir = globalenv()))
      paste(out, collapse = "\n")
    }},
    args = list(code = {code_quoted}),
    stderr = "|"
  )
  assign(job_id, proc, envir = jobs)
  cat(jsonlite::toJSON(list(id = job_id, status = "running"), auto_unbox = TRUE))
}})"#
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("r exec --async: invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

fn poll_async(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    let id_quoted = r_quote(id);
    let r_code = format!(
        r#"local({{
  job_id <- {id_quoted}
  if (!exists(".rstudio_cli_async_jobs", envir = globalenv())) {{
    stop(paste0("Job not found: ", job_id))
  }}
  jobs <- get(".rstudio_cli_async_jobs", envir = globalenv())
  if (!exists(job_id, envir = jobs)) {{
    stop(paste0("Job not found: ", job_id))
  }}
  proc <- get(job_id, envir = jobs)
  if (proc$is_alive()) {{
    cat(jsonlite::toJSON(list(id = job_id, status = "running"), auto_unbox = TRUE))
  }} else {{
    stderr_out <- paste(proc$read_all_error_lines(), collapse = "\n")
    result <- tryCatch(proc$get_result(), error = function(e) e)
    payload <- if (inherits(result, "error")) {{
      list(id = job_id, status = "error",
           message = conditionMessage(result),
           stderr = stderr_out)
    }} else {{
      list(id = job_id, status = "done",
           output = if (is.null(result)) "" else as.character(result),
           stderr = stderr_out)
    }}
    rm(list = job_id, envir = jobs)
    cat(jsonlite::toJSON(payload, auto_unbox = TRUE))
  }}
}})"#
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("r poll: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}
