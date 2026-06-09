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
                explanation: "Returns {output: \"[1] 2\", eval_env: {kind: \"top_level\"}}.",
            },
            ExampleSpec {
                cmd: "rstudio r exec --timeout 30 'Sys.sleep(10); summary(mtcars)'",
                explanation: "Bypasses the 2 s limit for a longer computation.",
            },
            ExampleSpec {
                cmd: "rstudio r exec --async 'Sys.sleep(5); paste(\"done\", Sys.time())'",
                explanation: "Returns {id, status:\"running\", eval_env:{kind:\"background_job\"}}; poll for result.",
            },
            ExampleSpec {
                cmd: "rstudio r exec 'ls()'   # while R is at a Browse[n]> prompt",
                explanation: "Auto-detects the browser frame: ls() lists the locals of the function being debugged. Response carries eval_env={kind:\"browser_frame\", function:\"<fn>\", depth:N}.",
            },
            ExampleSpec {
                cmd: "rstudio r exec 'stop(\"boom\")'",
                explanation: "Returns kind=r_error with message=\"boom\".",
            },
        ],
        returns: "{output: string, eval_env: {kind, ...}} or {id: string, status: \"running\", eval_env: {kind: \"background_job\"}} when --async",
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
                      source() or opens its own sinks. \
                      Browser-aware: when R is at a Browse[n]> prompt (active debugger), \
                      the helper evaluates in `parent.frame()` — the frame of the function \
                      being debugged — so the code sees the same locals as `n`/`s` would. \
                      Mutations to those locals persist after `c`. The `eval_env` field of \
                      the response reports where evaluation actually landed. \
                      Pass --no-capture for fire-and-forget behaviour (no output returned). \
                      Pass --timeout to bound the wait.",
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
                explanation: "Returns {stdout: \"[1] 12\", messages: [], error: null, \
                              eval_env: {kind: \"global\"}}; user sees ℝ(~{ sqrt(144) }) in \
                              their console.",
            },
            ExampleSpec {
                cmd: "rstudio r send 'message(\"hi\"); 1+1'",
                explanation: "Returns {stdout: \"[1] 2\", messages: [\"hi\\n\"], error: null, eval_env: {...}}.",
            },
            ExampleSpec {
                cmd: "rstudio r send 'y'   # at a Browse[n]> prompt inside debug_me()",
                explanation: "Auto-targets the browser frame; reads the local `y`. \
                              eval_env: {kind: \"browser_frame\", function: \"debug_me\", depth: 1}.",
            },
            ExampleSpec {
                cmd: "rstudio r send --no-capture 'print(Sys.time())'",
                explanation: "Fire-and-forget; code runs visibly, nothing returned.",
            },
        ],
        returns: "{stdout: string, messages: string[], error: string|null, \
                   eval_env: {kind: \"global\"|\"attached\"|\"browser_frame\", ...}} \
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
    /// Browser-aware: at a Browse[n]> prompt, auto-targets the debugger frame.
    /// Response includes an `eval_env` field describing where the code ran.
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
    /// Browser-aware: at a Browse[n]> prompt, auto-evaluates in the debugger
    /// frame (mutations persist). Response includes an `eval_env` field.
    /// To navigate the debugger itself, use `rstudio debug step <cmd>`.
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
            exec_aware(rpc, code, eval_timeout)
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

/// Where `r send`'s ℝ helper should resolve its evaluation environment.
///
/// Two RPC-derived inputs drive the choice:
/// - `get_environment_state.context_depth` — `> 0` means R is suspended in
///   a function frame (typically a `browser()` prompt). The CLI then
///   targets that frame so `r send 'y'` reads the debugged function's
///   local `y`, not a global.
/// - `get_environment_state.environment_name` — at depth 0, names the
///   "active" environment the RStudio Environment pane currently shows
///   (`.GlobalEnv` by default, or a search-path entry after `attach()`).
///   `r send` follows the user's gaze so that the visible call evaluates
///   in the same scope they're inspecting.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EvalTarget {
    /// `context_depth > 0`: resolve `parent.frame()` at the time `ℝ` is
    /// invoked. Since `ℝ` is called from the `Browse[n]>` reader,
    /// `parent.frame()` is the frame of the function being debugged.
    BrowserFrame,
    /// Default: evaluate in `.GlobalEnv`.
    Global,
    /// Evaluate in a named search-path entry (post-`attach()` etc.).
    Attached(String),
}

fn current_eval_target(rpc: &RpcClient<'_>) -> EvalTarget {
    let v = match rpc.rpc("get_environment_state", vec![]) {
        Ok(v) => v,
        Err(_) => return EvalTarget::Global,
    };
    let depth = v.get("context_depth").and_then(Value::as_i64).unwrap_or(0);
    if depth > 0 {
        return EvalTarget::BrowserFrame;
    }
    let name = v
        .get("environment_name")
        .and_then(|n| n.as_str())
        .unwrap_or(".GlobalEnv");
    if name == ".GlobalEnv" {
        EvalTarget::Global
    } else {
        EvalTarget::Attached(name.to_string())
    }
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

    // Decide where the ℝ helper should evaluate the user's code:
    //  - depth > 0 (browser/debugger active) → the debugged frame
    //    (`parent.frame()` at call-time);
    //  - depth 0 → the env the Environment pane currently shows
    //    (.GlobalEnv or a search-path entry after `attach()`).
    // The choice is baked into the helper at install time and surfaced
    // to the agent via the `eval_env` field of the response payload.
    let target = current_eval_target(rpc);

    // Install ℝ() with the result path and target environment baked in.
    r_eval::run_silent(rpc, &build_capture_fn(&result_path_for_r, &target))?;

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

fn build_capture_fn(result_path: &str, target: &EvalTarget) -> String {
    let path_r = r_quote(result_path);
    // Each target emits two snippets:
    //   - `env_resolution`: the R lines that set `.eval_env` (and any
    //     bookkeeping needed to describe it).
    //   - `meta_expr`: the R expression that produces the `eval_env`
    //     metadata list embedded in the JSON payload returned to the CLI.
    //
    // For BrowserFrame, both must be evaluated INSIDE ℝ's body — `parent.frame()`
    // and `sys.call(-1L)` only make sense at call-time, after the browser's
    // reader has invoked ℝ. For Global / Attached, install-time resolution
    // would also work but we keep the dispatch uniform.
    let (env_resolution, meta_expr) = match target {
        EvalTarget::BrowserFrame => (
            r#"
    .eval_env <- parent.frame()
    .ee_fn <- tryCatch(deparse(sys.call(-1L)[[1L]])[1L], error = function(e) NA_character_)
    .ee_depth <- tryCatch(as.integer(sys.nframe() - 1L), error = function(e) NA_integer_)"#
                .to_string(),
            r#"list(kind = "browser_frame", "function" = .ee_fn, depth = .ee_depth)"#.to_string(),
        ),
        EvalTarget::Global => (
            r#"
    .eval_env <- globalenv()"#
                .to_string(),
            r#"list(kind = "global")"#.to_string(),
        ),
        EvalTarget::Attached(name) => {
            let name_q = r_quote(name);
            (
                format!(
                    r#"
    .eval_env <- tryCatch(as.environment(match({name_q}, search())),
                          error = function(e) globalenv())"#
                ),
                format!(r#"list(kind = "attached", name = {name_q})"#),
            )
        }
    };
    format!(
        r#"local({{
  assign("ℝ", function(f) {{
    .expr <- f[[2]]{env_resolution}
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
          error = if (.ok) NULL else .err,
          eval_env = {meta_expr}
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

/// Sentinel inserted by `exec_aware`'s wrapper between the captured R
/// output and a one-line JSON describing the evaluation scope. Picked to
/// be implausible in any captured user output (zero-width-space + ascii).
const EXEC_EVAL_ENV_SENTINEL: &str = "\u{200b}__RSCLI_EVAL_ENV__\u{200b}";

/// `r exec` with server-side browser-frame detection.
///
/// Wraps the user code in a `local({…})` that walks `sys.calls()` looking
/// for the first `.rs.*` frame (the boundary between user code and
/// rsession's RPC plumbing). The frame immediately below that boundary,
/// when it exists, is the deepest user frame — the one the browser's
/// reader is sitting on. We eval the user code with `envir = sys.frame(i)`
/// to that frame, mirroring what the user gets when typing at `Browse[n]>`.
///
/// Outside a browser, no user frames sit below an `.rs.*` boundary
/// (or there are no `.rs.*` frames at all in unusual setups), so we
/// fall back to `environment()` — the wrapper's local scope, identical
/// to the pre-0.19 behavior of `r exec` (and to plain `eval(parse(...))`).
///
/// The wrapper emits two sections separated by `EXEC_EVAL_ENV_SENTINEL`:
/// the OK/ER-prefixed captured output (parsed by `r_eval::parse_output`),
/// then a single JSON line with the `eval_env` metadata.
fn exec_aware(
    rpc: &RpcClient<'_>,
    code: &str,
    timeout: EvalTimeout,
) -> Result<Option<Value>, CliError> {
    let r_code = build_exec_wrapper(code, timeout);
    let raw = rpc.rpc("execute_r_code", vec![Value::String(r_code)])?;
    let raw_str = raw
        .as_str()
        .ok_or_else(|| CliError::internal(format!("execute_r_code returned non-string: {raw}")))?;

    // Split the captured-output section from the eval_env metadata line.
    let (main, meta_raw) = match raw_str.rsplit_once(EXEC_EVAL_ENV_SENTINEL) {
        Some((m, j)) => (m.trim_end_matches('\n'), j.trim()),
        None => {
            // Older / partial output (e.g. on timeout): no metadata. We still
            // honour the OK/ER contract by parsing the whole blob.
            let output = r_eval::parse_exec_output(raw_str)?;
            return Ok(Some(json!({
                "output": output,
                "eval_env": { "kind": "top_level" }
            })));
        }
    };

    let output = r_eval::parse_exec_output(main)?;
    let eval_env: Value = serde_json::from_str(meta_raw).unwrap_or_else(|_| {
        // Best-effort fallback if R emitted something we can't parse — we
        // still want the user to see the output.
        json!({ "kind": "top_level" })
    });
    Ok(Some(json!({ "output": output, "eval_env": eval_env })))
}

fn build_exec_wrapper(user_code: &str, timeout: EvalTimeout) -> String {
    let setup = match timeout {
        EvalTimeout::ServerDefault => String::new(),
        EvalTimeout::Limit(secs) => {
            format!("setTimeLimit(elapsed = {secs}, transient = TRUE)")
        }
        EvalTimeout::NoLimit => "setTimeLimit(elapsed = Inf, transient = TRUE)".to_string(),
    };
    let code_q = r_quote(user_code);
    let sentinel = EXEC_EVAL_ENV_SENTINEL;
    format!(
        r#"local({{
  {setup}
  # --- detect browser frame (server-side, no extra RPC) ---
  .calls <- sys.calls()
  .eval_env <- environment()
  .ee_kind <- "top_level"
  .ee_fn <- NA_character_
  .ee_depth <- NA_integer_
  if (length(.calls) > 0L) {{
    .boundary <- NA_integer_
    for (.i in seq_along(.calls)) {{
      .fn <- tryCatch(deparse(.calls[[.i]][[1L]])[1L], error = function(e) "")
      if (startsWith(.fn, ".rs.") || startsWith(.fn, ".rstudio")) {{
        .boundary <- .i
        break
      }}
    }}
    if (!is.na(.boundary) && .boundary > 1L) {{
      .target <- .boundary - 1L
      .eval_env <- sys.frame(.target)
      .ee_kind <- "browser_frame"
      .ee_fn <- tryCatch(deparse(.calls[[.target]][[1L]])[1L], error = function(e) NA_character_)
      .ee_depth <- as.integer(.target)
    }}
  }}
  # --- run user code, OK/ER framing per r_eval contract ---
  .__r <- tryCatch({{
    .__c <- capture.output({{
      .__w <- withVisible(eval(parse(text = {code_q}), envir = .eval_env))
      if (.__w$visible) print(.__w$value)
    }})
    paste0("OK\n", paste(.__c, collapse = "\n"))
  }}, error = function(e) {{
    paste0("ER\n", conditionMessage(e))
  }})
  cat(.__r, sep = "")
  # --- sentinel + eval_env metadata ---
  cat("\n{sentinel}\n")
  cat(jsonlite::toJSON(
    list(kind = .ee_kind, "function" = .ee_fn, depth = .ee_depth),
    auto_unbox = TRUE, null = "null", na = "null"
  ))
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
    let mut parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("r exec --async: invalid JSON: {e}; raw: {raw}"))
    })?;
    if let Value::Object(ref mut map) = parsed {
        map.insert("eval_env".to_string(), json!({ "kind": "background_job" }));
    }
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
    let mut parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("r poll: invalid JSON: {e}; raw: {raw}")))?;
    // `r poll` reports on a background callr process — surface its scope
    // for parity with `r exec --async` and `r exec`.
    if let Value::Object(ref mut map) = parsed {
        map.insert("eval_env".to_string(), json!({ "kind": "background_job" }));
    }
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
        build_capture_fn(path, &EvalTarget::Global)
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
            code.contains("globalenv()") && code.contains(r#"kind = "global""#),
            "EvalTarget::Global must resolve .eval_env to globalenv() and tag meta as 'global'"
        );
    }

    #[test]
    fn capture_fn_browser_frame_uses_parent_frame() {
        let code = build_capture_fn("/tmp/test.json", &EvalTarget::BrowserFrame);
        assert!(
            code.contains("parent.frame()"),
            "BrowserFrame target must resolve .eval_env via parent.frame() at call-time: {code}"
        );
        assert!(
            code.contains(r#"kind = "browser_frame""#),
            "BrowserFrame target must tag meta as 'browser_frame': {code}"
        );
        // The function name and depth are captured at call-time (inside ℝ),
        // not baked in at install-time. Check the call-time helpers appear.
        assert!(
            code.contains("sys.call(-1L)") && code.contains("sys.nframe()"),
            "BrowserFrame meta must be captured at call-time via sys.call(-1L)/sys.nframe()"
        );
    }

    #[test]
    fn capture_fn_attached_uses_search_path_match() {
        let code = build_capture_fn(
            "/tmp/test.json",
            &EvalTarget::Attached("package:foo".into()),
        );
        assert!(
            code.contains(r#""package:foo""#) && code.contains("as.environment(match("),
            "Attached(name) target must use as.environment(match(name, search())): {code}"
        );
        assert!(
            code.contains(r#"kind = "attached""#) && code.contains(r#"name = "package:foo""#),
            "Attached meta must carry kind=attached and the name verbatim"
        );
    }

    #[test]
    fn capture_fn_payload_carries_eval_env() {
        // The JSON written to the result file MUST include `eval_env =
        // <meta_expr>` so the CLI can surface it back to the agent.
        for target in [
            EvalTarget::Global,
            EvalTarget::BrowserFrame,
            EvalTarget::Attached("X".into()),
        ] {
            let code = build_capture_fn("/tmp/t.json", &target);
            assert!(
                code.contains("eval_env ="),
                "capture payload must include `eval_env = ...` for target {target:?}: {code}"
            );
        }
    }

    #[test]
    fn exec_wrapper_carries_browser_detection_and_sentinel() {
        let code = build_exec_wrapper("1+1", EvalTimeout::ServerDefault);
        // Server-side detection walks sys.calls() for the .rs.* boundary.
        assert!(
            code.contains("sys.calls()") && code.contains(".rs.") && code.contains("sys.frame("),
            "exec wrapper must walk sys.calls() and resolve sys.frame() at the boundary: {code}"
        );
        // The sentinel must be present so the CLI knows where to split.
        assert!(
            code.contains(EXEC_EVAL_ENV_SENTINEL),
            "exec wrapper must emit the EXEC_EVAL_ENV_SENTINEL separator"
        );
        // OK/ER framing must be preserved (compatibility with parse_exec_output).
        assert!(
            code.contains("\"OK\\n\"") && code.contains("\"ER\\n\""),
            "exec wrapper must preserve r_eval's OK/ER status framing"
        );
    }

    #[test]
    fn exec_wrapper_emits_eval_env_json_kinds() {
        let code = build_exec_wrapper("ls()", EvalTimeout::ServerDefault);
        // Both kinds appear as literal strings in the wrapper, even if only
        // one is selected at runtime — guards against typos in the meta line.
        assert!(code.contains(r#""top_level""#));
        assert!(code.contains(r#""browser_frame""#));
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
        // Legacy contract: a search-path name (e.g. after `attach()`)
        // resolves via `as.environment(match(name, search()))`. Now
        // routed through EvalTarget::Attached but R-side semantics unchanged.
        let code = build_capture_fn("/tmp/test.json", &EvalTarget::Attached("mydf".into()));
        assert!(
            code.contains(r#""mydf""#) && code.contains("as.environment(match("),
            "non-global env_name must use as.environment(match(...))"
        );
    }
}
