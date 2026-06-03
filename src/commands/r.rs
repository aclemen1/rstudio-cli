use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval::{self, EvalTimeout};
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

const SOCKET_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);
const SEND_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
        name: "kill",
        summary: "Terminate a background R job started with `r exec --async`.",
        description: "Looks up the job handle stored in the R session's \
                      .rstudio_cli_async_jobs environment and calls \
                      callr's process$kill() (SIGTERM). With --tree, kills \
                      the process AND all its descendants (process$kill_tree()) \
                      — useful when the async code spawned child processes \
                      via system(), processx, etc. The job entry is removed \
                      from the registry regardless of whether the process \
                      had already finished. Idempotent: killing an already-\
                      dead job returns {status: \"already-done\"}.",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Job ID returned by `r exec --async`.",
            },
            ParamSpec {
                name: "--tree",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Kill the process tree (descendants too), not just the root process.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio r kill job_20260601_123456_789",
                explanation: "SIGTERM the async job. Returns {id, status: \"killed\"}.",
            },
            ExampleSpec {
                cmd: "rstudio r kill job_20260601_123456_789 --tree",
                explanation: "Also terminate any child processes the job spawned.",
            },
        ],
        returns: "{id: string, status: \"killed\" | \"already-done\"}",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Job ID not found in the active R session.",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "r",
        name: "interrupt",
        summary: "Interrupt the R code currently running in the main console.",
        description: "Equivalent of pressing the Stop button in RStudio's \
                      console pane, or sending SIGINT to the rsession's R \
                      interpreter. Targets the foreground R execution — the \
                      one that `r send` (or a user-typed expression) is \
                      blocked on. Fires the rsession `interrupt` JSON-RPC \
                      and returns immediately; the blocked `r send` (in \
                      another shell / agent) will return with kind=r_error \
                      and message=\"R execution was interrupted\". Has no \
                      effect on async jobs (use `r kill`) or on jobs in \
                      the Jobs pane (use `job kill`).",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio r interrupt",
            explanation: "From a second shell, interrupt whatever long expression \
                          the user (or another agent's `r send`) is running.",
        }],
        returns: "{interrupted: true}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("interrupt"),
    },
    ActionSpec {
        category: "r",
        name: "send",
        summary: "Send R code to the user's visible console and capture its output.",
        description: "Installs a helper `ℝ` in the session, then sends `ℝ(~{ code })` \
                      via console_input so the user sees it run. `ℝ` takes a formula \
                      (`~expr`) and extracts its RHS for evaluation — no NSE quoting in \
                      the call site. The helper captures stdout (cat, print, auto-print) \
                      via sink(split=TRUE) and messages via withCallingHandlers, writing \
                      the result as JSON to a tempfile that the CLI polls. Uses \
                      sink.number() to restore sink depth safely even when the code calls \
                      source() or opens its own sinks. Pass --no-capture for fire-and- \
                      forget behaviour (no output returned). Pass --timeout to bound the wait.",
        params: &[
            ParamSpec {
                name: "code",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "R code to execute visibly.",
            },
            ParamSpec {
                name: "--no-capture",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Skip capture; send code as-is and return nothing (fire-and-forget).",
            },
            ParamSpec {
                name: "--timeout",
                kind: ParamKind::Number,
                required: false,
                default: None,
                allowed: &[],
                description: "Seconds to wait for the result before giving up. \
                              Default: no limit. Ignored with --no-capture.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio r send 'sqrt(144)'",
                explanation: "Returns {stdout: \"[1] 12\", messages: [], error: null}; \
                              user sees ℝ(~{ sqrt(144) }) in their console.",
            },
            ExampleSpec {
                cmd: "rstudio r send 'message(\"hi\"); 1+1'",
                explanation: "Returns {stdout: \"[1] 2\", messages: [\"hi\\n\"], error: null}.",
            },
            ExampleSpec {
                cmd: "rstudio r send --no-capture 'print(Sys.time())'",
                explanation: "Fire-and-forget; code runs visibly, nothing returned.",
            },
        ],
        returns: "{stdout: string, messages: string[], error: string|null} \
                  or void with --no-capture",
        errors: &[
            ErrorSpec {
                kind: "r_error",
                when: "The R code raised a condition (stop, syntax error, ...).",
            },
            ErrorSpec {
                kind: "timeout",
                when: "Waiting for the result exceeded --timeout seconds.",
            },
        ],
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
    /// Terminate a background R job started with `r exec --async` (callr SIGTERM).
    Kill {
        id: String,
        /// Kill the whole process tree (descendants too), not just the root.
        #[arg(long)]
        tree: bool,
    },
    /// Interrupt the R code currently running in the main console (equivalent of Stop).
    Interrupt,
    /// Send R code to the user's visible console and capture its output.
    Send {
        code: String,
        /// Skip capture; send code as-is and return nothing (fire-and-forget).
        #[arg(long)]
        no_capture: bool,
        /// Seconds to wait for the captured result. Default: no limit.
        #[arg(long, short = 't')]
        timeout: Option<f64>,
    },
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
        RCmd::Kill { id, tree } => kill_async(rpc, id, *tree),
        RCmd::Interrupt => interrupt(rpc),
        RCmd::Send {
            code,
            no_capture,
            timeout,
        } => {
            if *no_capture {
                console_input(rpc, code)?;
                Ok(None)
            } else {
                send_with_capture(rpc, code, *timeout)
            }
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

fn console_input(rpc: &RpcClient<'_>, code: &str) -> Result<(), CliError> {
    // Do NOT append a trailing '\n' — rsession's console_input already
    // terminates the input with a newline before pushing it to the R input
    // queue. Appending our own would inject a blank line.
    rpc.rpc(
        "console_input",
        vec![
            Value::String(code.to_string()),
            Value::String(String::new()),
            json!(0),
        ],
    )?;
    Ok(())
}

fn current_environment_name(rpc: &RpcClient<'_>) -> String {
    rpc.rpc("get_environment_state", vec![])
        .ok()
        .and_then(|v| {
            v.get("environment_name")
                .and_then(|n| n.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| ".GlobalEnv".to_string())
}

fn send_with_capture(
    rpc: &RpcClient<'_>,
    code: &str,
    timeout: Option<f64>,
) -> Result<Option<Value>, CliError> {
    // Stable rsession PID — used both as the result-file name and for crash
    // detection. One fixed file per rsession lifetime means no orphan
    // proliferation: a leftover from a previous killed call is cleaned up
    // at the start of the next one.
    let rsession_pid: Option<u32> = r_eval::run(rpc, "Sys.getpid()")
        .ok()
        .and_then(|s| s.trim().strip_prefix("[1] ").and_then(|n| n.parse().ok()));

    // Result file path. The CLI and rsession share the same filesystem
    // (Desktop or Server-on-same-host), so a single path serves both for
    // polling on the CLI side and for the R wrapper's writeLines target.
    let filename = match rsession_pid {
        Some(pid) => format!("rstudio_cap_{pid}.json"),
        None => format!(
            "rstudio_cap_fallback_{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ),
    };
    let result_path = std::env::temp_dir().join(&filename);
    let result_path_for_r = result_path.to_string_lossy().into_owned();

    // Remove any leftover from a previously killed call.
    let _ = std::fs::remove_file(&result_path);

    // Resolve the active environment in the RStudio Environment pane so that
    // eval() targets the same scope the user is looking at (e.g. after attach).
    let env_name = current_environment_name(rpc);

    // Install R() with the result path and target environment baked in.
    r_eval::run_silent(rpc, &build_capture_fn(&result_path_for_r, &env_name))?;

    // Send the wrapper via console_input — visible to the user, no quotes or
    // path argument exposed. Single-line code stays on one line; multi-line
    // code gets a brace block.
    let call = if code.contains('\n') {
        format!("ℝ(~{{\n{code}\n}})")
    } else {
        format!("ℝ(~{{ {code} }})")
    };
    console_input(rpc, &call)?;

    // Poll the filesystem until R() writes the result file. Two early-exit
    // signals: the result file appears (normal completion or R-side interrupt
    // sentinel), or rsession is no longer alive (crash / kill).
    let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t));
    loop {
        std::thread::sleep(SEND_POLL_INTERVAL);
        if result_path.exists() {
            break;
        }
        if rsession_pid.is_some_and(|pid| !process_alive(pid)) {
            let _ = std::fs::remove_file(&result_path);
            return Err(CliError::r(
                "rsession process died while waiting for r send result".to_string(),
            ));
        }
        if deadline.is_some_and(|d| Instant::now() > d) {
            let _ = std::fs::remove_file(&result_path);
            return Err(CliError::timeout(
                "r send: timed out waiting for capture result",
            ));
        }
    }

    let content = std::fs::read_to_string(&result_path)
        .map_err(|e| CliError::internal(format!("r send: failed to read result: {e}")))?;
    let _ = std::fs::remove_file(&result_path);

    let result: Value = serde_json::from_str(&content).map_err(|e| {
        CliError::internal(format!("r send: invalid JSON result: {e}; raw: {content}"))
    })?;

    if result
        .get("interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(CliError::r("R execution was interrupted".to_string()));
    }

    if let Some(err_msg) = result.get("error").and_then(Value::as_str) {
        return Err(CliError::r(err_msg.to_string()));
    }

    Ok(Some(result))
}

fn process_alive(pid: u32) -> bool {
    // kill(pid, 0) returns 0 if the process exists and we have permission,
    // -1 with ESRCH if it does not exist.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn build_capture_fn(result_path: &str, env_name: &str) -> String {
    let path_r = r_quote(result_path);
    let env_name_r = r_quote(env_name);
    format!(
        r#"local({{
  .env_name <- {env_name_r}
  .eval_env <- if (.env_name == ".GlobalEnv") {{
    globalenv()
  }} else {{
    tryCatch(as.environment(match(.env_name, search())), error = function(e) globalenv())
  }}
  assign("ℝ", function(f) {{
    .expr <- f[[2]]
    .start <- sink.number()
    .out <- tempfile()
    .oc <- file(.out, "w+")
    .msgs <- character()
    .ok <- TRUE
    .err <- NULL
    .interrupted <- TRUE
    on.exit({{
      while (sink.number() > .start) sink(NULL)
      try(close(.oc), silent = TRUE)
      .stdout <- paste(readLines(.out, warn = FALSE), collapse = "\n")
      try(unlink(.out), silent = TRUE)
      .payload <- if (.interrupted) {{
        '{{"interrupted":true}}'
      }} else {{
        jsonlite::toJSON(list(
          stdout = .stdout,
          messages = as.list(.msgs),
          error = if (.ok) NULL else .err
        ), auto_unbox = TRUE, null = "null")
      }}
      try(writeLines(.payload, {path_r}), silent = TRUE)
      try(suppressWarnings(rm("ℝ", envir = globalenv())), silent = TRUE)
    }}, add = TRUE)
    tryCatch({{
      sink(.oc, split = TRUE)
      withCallingHandlers({{
        .v <- withVisible(eval(.expr, envir = .eval_env))
        if (.v$visible) print(.v$value)
      }}, message = function(m) {{
        .msgs <<- c(.msgs, conditionMessage(m))
      }})
    }}, error = function(e) {{
      .ok <<- FALSE
      .err <<- conditionMessage(e)
    }})
    .interrupted <- FALSE
    invisible(NULL)
  }}, envir = globalenv())
}})"#
    )
}

fn exec_async(rpc: &RpcClient<'_>, code: &str) -> Result<Option<Value>, CliError> {
    // `callr` is a hard precheck (see `r_package::R_HARD_DEPS`): the CLI
    // refuses to dispatch any RPC if it's not installed, so by the time
    // this code reaches the rsession the package is guaranteed loadable.
    // No defensive `requireNamespace("callr", ...)` guard needed here.
    let code_quoted = r_quote(code);
    let r_code = format!(
        r#"local({{
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

fn build_kill_async_code(id: &str, tree: bool) -> String {
    // Mirrors poll_async()'s lookup, then dispatches to process$kill() /
    // process$kill_tree() depending on --tree. We don't error on
    // is_alive()==FALSE: a kill against an already-finished job is a no-op
    // and returns {status:"already-done"} so callers can be idempotent.
    let id_quoted = r_quote(id);
    let kill_call = if tree {
        "proc$kill_tree()"
    } else {
        "proc$kill()"
    };
    format!(
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
  status <- if (proc$is_alive()) {{
    try({kill_call}, silent = TRUE)
    "killed"
  }} else {{
    "already-done"
  }}
  rm(list = job_id, envir = jobs)
  cat(jsonlite::toJSON(list(id = job_id, status = status), auto_unbox = TRUE))
}})"#
    )
}

fn kill_async(rpc: &RpcClient<'_>, id: &str, tree: bool) -> Result<Option<Value>, CliError> {
    let r_code = build_kill_async_code(id, tree);
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("r kill: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn interrupt(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    // The `interrupt` RPC is handled by rsession on a side channel — it
    // signals the R interpreter to abort its current evaluation (same as
    // the Stop button in the console pane). It returns essentially
    // immediately even if R is busy. We discard the (typically empty)
    // result and return a canonical envelope so agents can assert success.
    rpc.rpc("interrupt", Vec::new())?;
    Ok(Some(json!({ "interrupted": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(path: &str) -> String {
        build_capture_fn(path, ".GlobalEnv")
    }

    #[test]
    fn capture_fn_suppresses_rm_warning() {
        let code = cap("/tmp/test.json");
        assert!(
            code.contains(r#"suppressWarnings(rm("ℝ""#),
            "rm(\"ℝ\") must be wrapped in suppressWarnings to avoid a warning \
             when user code runs rm(list = ls()) and removes ℝ first"
        );
    }

    #[test]
    fn capture_fn_sentinel_write_before_cleanup() {
        let code = cap("/tmp/test.json");
        let write_pos = code.find("writeLines").expect("writeLines not found");
        let rm_pos = code
            .find(r#"suppressWarnings(rm("ℝ""#)
            .expect("rm(ℝ) not found");
        assert!(
            write_pos < rm_pos,
            "sentinel must be written before ℝ is removed from globalenv"
        );
    }

    #[test]
    fn capture_fn_uses_globalenv_by_default() {
        let code = cap("/tmp/test.json");
        assert!(
            code.contains(r#"".GlobalEnv""#) && code.contains("globalenv()"),
            "default env_name must resolve to globalenv()"
        );
    }

    #[test]
    fn registry_exposes_kill_and_interrupt() {
        // Catalog contract: `rstudio schema r kill` and `rstudio schema r
        // interrupt` must succeed. Guards against accidental removal from
        // the ACTIONS slice — also worth catching here as well as in the
        // mcp dispatch table test, since these actions are visible to
        // agents through both surfaces.
        assert!(
            ACTIONS.iter().any(|a| a.name == "kill"),
            "r.kill must be in the registry"
        );
        assert!(
            ACTIONS.iter().any(|a| a.name == "interrupt"),
            "r.interrupt must be in the registry"
        );
    }

    #[test]
    fn interrupt_action_documents_rpc_method_interrupt() {
        // The RPC method name was empirically validated against rsession —
        // pin it so a typo refactor (e.g. "interrupt_r") is caught at test
        // time instead of at runtime against a live session.
        let interrupt = ACTIONS
            .iter()
            .find(|a| a.name == "interrupt")
            .expect("r.interrupt action");
        assert_eq!(
            interrupt.rpc_method,
            Some("interrupt"),
            "r.interrupt must declare the rsession RPC method it issues"
        );
    }

    #[test]
    fn kill_async_default_uses_kill_not_kill_tree() {
        let code = build_kill_async_code("job_abc", false);
        assert!(
            code.contains("proc$kill()"),
            "without --tree, must call proc$kill(): {code}"
        );
        assert!(
            !code.contains("proc$kill_tree()"),
            "without --tree, must NOT call proc$kill_tree(): {code}"
        );
    }

    #[test]
    fn kill_async_tree_uses_kill_tree() {
        let code = build_kill_async_code("job_abc", true);
        assert!(
            code.contains("proc$kill_tree()"),
            "with --tree, must call proc$kill_tree(): {code}"
        );
        assert!(
            !code.contains("proc$kill()"),
            "with --tree, must NOT call proc$kill() alone: {code}"
        );
    }

    #[test]
    fn kill_async_quotes_id_safely() {
        // The id flows from CLI args — any quotes/backslashes in it must be
        // escaped so the R parser doesn't reinterpret them as a string boundary
        // or a control sequence. Without r_quote, an id of `"; system("rm -rf");"`
        // would be a trivially exploitable injection.
        let code = build_kill_async_code(r#"abc"def\xyz"#, false);
        assert!(
            code.contains(r#""abc\"def\\xyz""#),
            "id must be R-quoted: {code}"
        );
    }

    #[test]
    fn kill_async_emits_idempotent_status_branches() {
        let code = build_kill_async_code("job_abc", false);
        // Both branches must be present: running → "killed", finished → "already-done".
        // The contract documented in the ActionSpec and the skill depends on it.
        assert!(code.contains("\"killed\""), "missing killed branch: {code}");
        assert!(
            code.contains("\"already-done\""),
            "missing already-done branch: {code}"
        );
    }

    #[test]
    fn capture_fn_uses_custom_env_name() {
        let code = build_capture_fn("/tmp/test.json", "mydf");
        assert!(
            code.contains(r#""mydf""#) && code.contains("as.environment(match("),
            "non-global env_name must use as.environment(match(...))"
        );
    }
}
