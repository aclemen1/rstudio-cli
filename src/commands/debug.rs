//! `rstudio debug …` — first-class support for R's `browser()` debugger.
//!
//! R's debugger (whether entered through `browser()`, `debug()`, `debugonce()`,
//! `traceback() + recover`, or an error with `options(error = recover)`) puts
//! the console at a `Browse[n]>` prompt. The rsession keeps running and all
//! RPCs remain reachable, but the user-facing semantics of "send code to R"
//! change: meta-commands (`n`, `s`, `c`, `f`, `Q`, `where`, `help`, `r`) are
//! intercepted by the browser reader; expressions are evaluated in the frame
//! of the function being debugged, not in `.GlobalEnv`.
//!
//! The `debug` subcommand wraps these affordances behind explicit verbs so
//! agents don't have to guess (and don't accidentally send `n` wrapped in
//! `ℝ(~{...})`, which evaluates the symbol `n` and errors out). It also
//! projects rsession's `get_environment_state` payload into a compact,
//! agent-friendly shape — depth, current frame, full stack with source
//! refs, and the typed local variables of the active frame.
//!
//! All `debug` actions are no-ops when R is NOT at a browser prompt; they
//! return `{ in_browser: false }` or refuse with `kind = not_in_debugger`
//! depending on whether the verb makes sense outside the debugger.

use std::time::{Duration, Instant};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::RpcClient;
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

/// `debug step` post-send confirmation timing. After sending the command we
/// wait `STEP_INITIAL_DELAY` (so console_input is processed and we don't read
/// the pre-step state), then poll every `STEP_POLL_INTERVAL` until the state
/// is stable across two reads, capped at `STEP_SETTLE_TIMEOUT`. The cap is
/// bounded because `c`/`Q` can run arbitrarily long.
const STEP_INITIAL_DELAY: Duration = Duration::from_millis(200);
const STEP_SETTLE_TIMEOUT: Duration = Duration::from_millis(2000);
const STEP_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "debug",
        name: "status",
        summary: "Read the active R debugger state: function, locals, full call stack.",
        description: "Single-RPC projection of `get_environment_state`. \
                      Detects the debugger via context_depth OR a non-empty \
                      call stack, so it correctly reports `in_browser: true` for \
                      ALL browser entries — including a `browser()` at the \
                      top-level prompt or one triggered by `r send 'browser()'`, \
                      which leave rsession's context_depth at 0. When in the \
                      debugger, returns the innermost debugged `function` \
                      (null for a top-level browser), the `src` location, the \
                      typed `locals` of the current frame, and the full \
                      `call_stack` (innermost first). \
                      `browse_level` is the N of Browse[N]> (the count of \
                      active browser contexts). R does not expose it (browser() \
                      is a C primitive; rsession reduces the prompt to a \
                      boolean), so it is recovered via the companion package's \
                      optional native helper that walks R's context stack in C. \
                      `browse_level_source` is \"native\" when that integer is \
                      authoritative, or \"unavailable\" (with browse_level null) \
                      when the helper can't be built — e.g. no C toolchain on \
                      the host. The level is never needed to navigate \
                      (`debug exit`/`Q` leaves all levels at once). \
                      When R is idle, returns {in_browser: false}.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio debug status",
            explanation: "Returns {in_browser, function?, browse_level, browse_level_source, src?, locals?, call_stack?}.",
        }],
        returns: "{in_browser: bool, browse_level: int|null, \
                  browse_level_source: \"native\"|\"unavailable\", \
                  function?: string|null, src?: {file, line}, \
                  locals?: [{name, type, class, value}], \
                  call_stack?: [{depth, function, call, src?}]}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("get_environment_state"),
    },
    ActionSpec {
        category: "debug",
        name: "step",
        summary: "Send a browser meta-command (n, s, f, c, Q, where, help, r) to the active debugger.",
        description: "Pushes the literal command to rsession's console_input \
                      queue, where the browser reader interprets it: \
                      n=next, s=step in, f=finish, c=continue (exit one level), \
                      Q=quit (exit all browsers), where=print call stack, \
                      help=browser help, r=invoke 'resume' restart. \
                      After sending, WAITS (bounded, ~3 s) for the prompt to \
                      settle and returns the POST-step state — so you don't \
                      read a stale snapshot in a race. `settled: false` means \
                      the wait timed out (e.g. `c` is running a long \
                      computation); re-poll `debug status` in that case.",
        params: &[ParamSpec {
            name: "command",
            kind: ParamKind::Enum,
            required: true,
            default: None,
            allowed: &["n", "s", "f", "c", "Q", "where", "help", "r"],
            description: "Browser meta-command. See `?browser` for semantics.",
        }],
        examples: &[
            ExampleSpec {
                cmd: "rstudio debug step n",
                explanation: "Step to the next statement; returns the post-step \
                              {in_browser, function, src, settled: true}.",
            },
            ExampleSpec {
                cmd: "rstudio debug step c",
                explanation: "Continue; typically returns {in_browser: false} once R \
                              leaves the debugger (or the next browser settles).",
            },
        ],
        returns: "{sent: string, settled: bool, in_browser: bool, \
                  captured_at_unix_ms: int, browse_level?: int|null, \
                  browse_level_source?: string, function?: string|null, \
                  src?: {file, line}}",
        errors: &[ErrorSpec {
            kind: "not_in_debugger",
            when: "R is not at a Browse[n]> prompt; sending a meta-command would either error \
                   (e.g. 'where' as a free symbol) or be misinterpreted as user code.",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("console_input"),
    },
    ActionSpec {
        category: "debug",
        name: "where",
        summary: "Print the active call stack — one entry per frame, innermost first.",
        description: "Equivalent semantics to typing `where` at the browser prompt, \
                      but returned as structured JSON instead of free text. Projects \
                      `get_environment_state.call_frames`.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio debug where",
            explanation: "Returns {in_browser, call_stack: [{depth, function, call, src?}]}.",
        }],
        returns: "{in_browser: bool, call_stack?: [{depth, function, call, src?}]}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("get_environment_state"),
    },
    ActionSpec {
        category: "debug",
        name: "locals",
        summary: "List the typed local variables of the current debugger frame.",
        description: "Projects `get_environment_state.environment_list`: one \
                      entry per local with name, class, type, length, and \
                      a textual value preview (already prepared by rsession \
                      for the Environment pane). Returns an empty list when \
                      not in browser.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio debug locals",
            explanation: "Returns {in_browser, locals: [{name, type, class, length, value}]}.",
        }],
        returns: "{in_browser: bool, locals: [{name, type, class, length, value}]}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("get_environment_state"),
    },
    ActionSpec {
        category: "debug",
        name: "src",
        summary: "Source location of the current debugger frame (file + line).",
        description: "Projects the source ref of the innermost call_frame: \
                      `{file, line}` when a real `srcref` is available \
                      (the function was defined in a file or sourced with \
                      `keep.source = TRUE`), otherwise `null`. Use this to \
                      e.g. `editor open` the file at the right line.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio debug src",
            explanation: "Returns {in_browser, src?: {file, line}}.",
        }],
        returns: "{in_browser: bool, src?: {file: string, line: int}}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("get_environment_state"),
    },
    ActionSpec {
        category: "debug",
        name: "exit",
        summary: "Quit the debugger entirely (equivalent to typing Q).",
        description: "Pushes `Q` to the browser reader, terminating ALL active \
                      browser frames in one step. Alias of `debug step Q`: like \
                      `step`, it waits for the prompt to settle and returns the \
                      post-exit state (normally {in_browser: false}).",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio debug exit",
            explanation: "Bails out of the debugger; returns {sent: \"Q\", in_browser: false, settled: true}.",
        }],
        returns: "{sent: \"Q\", settled: bool, in_browser: bool, captured_at_unix_ms: int}",
        errors: &[ErrorSpec {
            kind: "not_in_debugger",
            when: "R is not at a Browse[n]> prompt.",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("console_input"),
    },
];

#[derive(Subcommand, Debug)]
pub enum DebugCmd {
    /// Active debugger state (depth, frame, locals, call stack).
    Status,
    /// Send a browser meta-command (n, s, f, c, Q, where, help, r).
    Step {
        /// Browser meta-command to send. See `?browser` in R.
        #[arg(value_parser = ["n", "s", "f", "c", "Q", "where", "help", "r"])]
        command: String,
    },
    /// Print the active call stack as structured JSON.
    Where,
    /// List the typed locals of the current debugger frame.
    Locals,
    /// Source location of the current debugger frame.
    Src,
    /// Quit the debugger entirely (alias of `step Q`).
    Exit,
}

pub fn run(cmd: &DebugCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        DebugCmd::Status => status(rpc),
        DebugCmd::Step { command } => step(rpc, command),
        DebugCmd::Where => where_cmd(rpc),
        DebugCmd::Locals => locals(rpc),
        DebugCmd::Src => src(rpc),
        DebugCmd::Exit => step(rpc, "Q"),
    }
}

/// Fetch and project `get_environment_state` into a debugger-centric shape.
fn fetch_state(rpc: &RpcClient<'_>) -> Result<Value, CliError> {
    rpc.rpc("get_environment_state", vec![])
}

/// Wall-clock capture time (unix epoch ms). Surfaced as `captured_at_unix_ms`
/// on debugger snapshots so an agent can reason about freshness: a debugger
/// snapshot has no rsession-side generation id, and the user may step or
/// continue between calls, so a timestamp is the pragmatic staleness signal.
pub(crate) fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn context_depth(state: &Value) -> i64 {
    state
        .get("context_depth")
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// Number of frames in rsession's `call_frames` array. Non-zero whenever
/// R is suspended in a function (user-debug or otherwise), including when
/// a `browser()` was invoked at the top-level or through a side-channel
/// like `console_input` — cases where rsession's IDE-side `context_depth`
/// counter is NOT incremented because no `debug()` flag / breakpoint
/// triggered the entry. See `is_in_debugger`.
fn call_frames_len(state: &Value) -> usize {
    state
        .get("call_frames")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0)
}

/// True iff R is currently at a Browse[n]> prompt, regardless of how the
/// browser was entered.
///
/// Why we don't just look at `context_depth`: rsession increments that
/// counter only when the user's `browser()` call (or `debug()`-set flag,
/// or breakpoint hit) is observed by the IDE-side debugger hook. A
/// `browser()` invoked at the top-level prompt, or through a side
/// channel like `console_input "ℝ(~{ browser() })"`, leaves
/// `context_depth` at 0 because no IDE hook fired — but the R interpreter
/// IS suspended at a Browse prompt and `call_frames` reflects the
/// active stack. We treat any of these signals as "in debugger".
fn is_in_debugger(state: &Value) -> bool {
    context_depth(state) > 0 || call_frames_len(state) > 0
}

/// The user function currently being debugged, or `None`.
///
/// rsession populates `environment_name` with the innermost debugged
/// function's name (suffixed with `()`) — e.g. `"f()"`. We strip the
/// parens and return `"f"`. When the browser was entered at the top
/// level (`environment_name == ".GlobalEnv"`: a bare `browser()` typed
/// at the console, or `r send 'browser()'` which evaluates inside the
/// CLI's own `ℝ` helper), there is no user function being debugged, so
/// we return `None` rather than leaking an evaluator-wrapper name like
/// `ℝ` or `eval` from the call stack. The full `call_stack` is still
/// available for agents that want the raw frames.
pub(crate) fn debugged_function(state: &Value) -> Option<String> {
    let env_name = state
        .get("environment_name")
        .and_then(Value::as_str)
        .unwrap_or(".GlobalEnv");
    if env_name != ".GlobalEnv" && !env_name.is_empty() {
        return Some(env_name.trim_end_matches("()").to_string());
    }
    // env_name is .GlobalEnv — which happens when the browser was reached
    // through an instrumented `browser()` that scopes the prompt to a
    // non-function env (e.g. `do.call(base::browser, …, envir = wrap)` as
    // modulr / debugme do). rsession then can't name a function, but a user
    // function IS on the stack. Walk call_frames (innermost first), skip the
    // instrumentation frames, and report the first real user function — so
    // `function` is not null when a user call is being debugged.
    first_user_function(state)
}

/// True for call-stack frames that are debugger / harness instrumentation
/// rather than the user's code. Skipping these lets `debugged_function`
/// report the innermost *user* function even when the browser was entered
/// through such a wrapper. Covered: the `browser()` shim and the
/// `do.call(base::browser, …)` some wrappers (modulr, debugme) use to enter
/// the browser with an explicit env; rsession's own `.rs.*` / `.rstudio*`
/// internals; and the rstudio-cli `ℝ` capture helper plus the R eval
/// machinery it (and `r send` / `r exec`) wrap user code in (`eval`,
/// `withVisible`, `withCallingHandlers`, the `tryCatch` family,
/// `capture.output`, `local`, `eval.parent`, `suppressWarnings`).
fn is_instrumentation_frame(function_name: &str) -> bool {
    matches!(
        function_name,
        "do.call"
            | "browser"
            | "base::browser"
            | "ℝ"
            | "eval"
            | "eval.parent"
            | "evalq"
            | "withVisible"
            | "withCallingHandlers"
            | "tryCatch"
            | "tryCatchList"
            | "tryCatchOne"
            | "doTryCatch"
            | "capture.output"
            | "suppressWarnings"
            | "suppressMessages"
            | "local"
    ) || function_name.starts_with(".rs")
        || function_name.starts_with(".rstudio")
}

/// First user (non-instrumentation) function name walking call_frames from
/// the innermost outward, or `None`.
fn first_user_function(state: &Value) -> Option<String> {
    state
        .get("call_frames")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|f| f.get("function_name").and_then(Value::as_str))
        .find(|name| !name.is_empty() && !is_instrumentation_frame(name))
        .map(str::to_string)
}

fn project_call_stack(state: &Value) -> Vec<Value> {
    state
        .get("call_frames")
        .and_then(Value::as_array)
        .map(|frames| {
            frames
                .iter()
                .map(|f| {
                    let file = f
                        .get("file_name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty());
                    let line = f.get("line_number").and_then(Value::as_i64);
                    let src = match (file, line) {
                        (Some(f), Some(l)) => Some(json!({ "file": f, "line": l })),
                        _ => None,
                    };
                    json!({
                        "depth": f.get("context_depth").and_then(Value::as_i64),
                        "function": f.get("function_name").and_then(Value::as_str),
                        "call": f.get("call_summary").and_then(Value::as_str),
                        "src": src,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn project_locals(state: &Value) -> Vec<Value> {
    state
        .get("environment_list")
        .and_then(Value::as_array)
        .map(|vars| {
            vars.iter()
                .map(|v| {
                    json!({
                        "name": v.get("name").and_then(Value::as_str),
                        "type": v.get("type").and_then(Value::as_str),
                        "class": v.get("clazz"),
                        "length": v.get("length").and_then(Value::as_i64),
                        "value": v.get("value").and_then(Value::as_str),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn project_current_src(state: &Value) -> Option<Value> {
    let frames = state.get("call_frames").and_then(Value::as_array)?;
    let first = frames.first()?;
    let file = first
        .get("file_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let line = first.get("line_number").and_then(Value::as_i64)?;
    Some(json!({ "file": file, "line": line }))
}

/// Best-effort retrieval of the `Browse[N]>` nesting level via the
/// companion package's optional native helper (`rscli_browse_level`,
/// see `r-package/inst/native/browse_level.c`). Returns:
/// - `(Some(n), "native")` when the helper is built/loaded and returns N;
/// - `(None, "unavailable")` when the helper can't be built (no toolchain),
///   failed, or the eval errored.
///
/// The helper walks R's context stack in C — the only way to recover N,
/// which R does not otherwise expose. It compiles lazily on first use and
/// caches the shared object, so the cost is paid at most once per machine /
/// R version. Everything degrades to `unavailable` rather than failing.
///
/// Shared with `status::collect_debugger` so the ambient `rsession.debugger`
/// block and the rich `debug status` report agree on the level.
pub(crate) fn native_browse_level(rpc: &RpcClient<'_>) -> (Value, &'static str) {
    // `cat("null")` for an unavailable/NULL result, the integer otherwise.
    let code = "local({ v <- tryCatch(rstudiocli::rscli_browse_level(), \
                error = function(e) NULL); \
                if (is.null(v)) cat(\"null\") else cat(as.integer(v)) })";
    match r_eval::run(rpc, code) {
        Ok(out) => {
            let t = out.trim();
            if t == "null" || t.is_empty() {
                (Value::Null, "unavailable")
            } else if let Ok(n) = t.parse::<i64>() {
                (json!(n), "native")
            } else {
                (Value::Null, "unavailable")
            }
        }
        Err(_) => (Value::Null, "unavailable"),
    }
}

fn status(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let state = fetch_state(rpc)?;
    if !is_in_debugger(&state) {
        return Ok(Some(json!({ "in_browser": false })));
    }
    // `browse_level` is the N of the `Browse[N]>` prompt — the count of
    // active browser contexts on R's interpreter stack. R does not expose
    // it to the language (browser() is a C primitive; sys.calls()/
    // sys.nframe() don't reflect it; rsession regex-matches the prompt to a
    // boolean and discards the digits), so we recover it via the companion
    // package's optional native helper, which walks R's context stack in C.
    // When the helper is unavailable (no C toolchain to build it, build/load
    // failure), `browse_level` is null and `browse_level_source` is
    // "unavailable". `browse_level_source` == "native" means the integer is
    // authoritative.
    //
    // (Note: rsession's `context_depth` is NOT the browse level — it is the
    // selected-frame index, 1 at both Browse[1]> and Browse[2]> — which is
    // why we don't derive the level from it.)
    //
    // Navigation note: R's `Q` exits ALL nested browsers at once (see
    // `?browser`), so the level is never needed to escape — a single
    // `debug exit` / `debug step Q` suffices regardless of N.
    let (browse_level, browse_level_source) = native_browse_level(rpc);
    Ok(Some(json!({
        "in_browser": true,
        "browse_level": browse_level,
        "browse_level_source": browse_level_source,
        "function": debugged_function(&state),
        "src": project_current_src(&state),
        "locals": project_locals(&state),
        "call_stack": project_call_stack(&state),
        "captured_at_unix_ms": now_unix_ms(),
    })))
}

fn where_cmd(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let state = fetch_state(rpc)?;
    if !is_in_debugger(&state) {
        return Ok(Some(json!({ "in_browser": false })));
    }
    Ok(Some(json!({
        "in_browser": true,
        "call_stack": project_call_stack(&state),
    })))
}

fn locals(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let state = fetch_state(rpc)?;
    if !is_in_debugger(&state) {
        return Ok(Some(json!({ "in_browser": false, "locals": [] })));
    }
    Ok(Some(json!({
        "in_browser": true,
        "locals": project_locals(&state),
    })))
}

fn src(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let state = fetch_state(rpc)?;
    if !is_in_debugger(&state) {
        return Ok(Some(json!({ "in_browser": false })));
    }
    Ok(Some(json!({
        "in_browser": true,
        "src": project_current_src(&state),
    })))
}

fn step(rpc: &RpcClient<'_>, command: &str) -> Result<Option<Value>, CliError> {
    // Refuse to send a meta-command when no debugger is active: at the
    // regular `>` prompt, `n` / `s` / `c` / `where` are either bare symbols
    // (likely undefined and noisy) or, worse, accidentally rebind a user
    // variable. Cheap pre-check via get_environment_state. We use
    // `is_in_debugger` (not just `context_depth > 0`) so we correctly
    // recognise top-level / side-channel `browser()` calls — the kind
    // that an agent may trigger via `r send 'browser()'`, where the
    // IDE-side context counter stays at 0 but the R interpreter is
    // genuinely suspended at a Browse[]> prompt.
    let state = fetch_state(rpc)?;
    if !is_in_debugger(&state) {
        return Err(CliError::user(format!(
            "debug step '{command}': R is not at a Browse[n]> prompt. \
             Use `rstudio debug status` to confirm, or trigger a browser() \
             first. Meta-commands like 'n', 'c', 'where' are only meaningful \
             inside the debugger."
        )));
    }
    // console_input expects [text, "", 0] (see commands/r.rs::console_input).
    // No trailing newline — rsession adds one.
    rpc.rpc(
        "console_input",
        vec![
            Value::String(command.to_string()),
            Value::String(String::new()),
            json!(0),
        ],
    )?;

    // Confirm the post-step state instead of returning the stale pre-step
    // snapshot (the race the old fire-and-forget behaviour exposed). We:
    //   1. wait a short initial delay so console_input is processed and the
    //      new prompt has begun to settle (avoids reading the pre-step state);
    //   2. poll until the state signature is STABLE across two consecutive
    //      reads (avoids transient mid-step snapshots), capped by a deadline.
    // We intentionally do NOT require the signature to *change*: a step that
    // stays on the same source line (e.g. a one-line function, or stepping
    // within a line) would otherwise never "change" and we'd block to the
    // cap for nothing. Stability after the initial delay is the post-step
    // state. `settled: false` means it never stabilized within the cap
    // (e.g. `c` is running a long computation) — re-poll `debug status`.
    std::thread::sleep(STEP_INITIAL_DELAY);
    let deadline = Instant::now() + STEP_SETTLE_TIMEOUT;
    let mut final_state = fetch_state(rpc).unwrap_or(state);
    let mut last_sig = debug_signature(&final_state);
    let mut settled = false;
    while Instant::now() < deadline {
        std::thread::sleep(STEP_POLL_INTERVAL);
        let s = match fetch_state(rpc) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let sig = debug_signature(&s);
        let stable = sig == last_sig;
        last_sig = sig;
        final_state = s;
        if stable {
            settled = true;
            break;
        }
    }

    let in_browser = is_in_debugger(&final_state);
    let mut out = serde_json::Map::new();
    out.insert("sent".into(), json!(command));
    out.insert("settled".into(), json!(settled));
    out.insert("in_browser".into(), json!(in_browser));
    out.insert("captured_at_unix_ms".into(), json!(now_unix_ms()));
    if in_browser {
        let (browse_level, browse_level_source) = native_browse_level(rpc);
        out.insert("browse_level".into(), browse_level);
        out.insert("browse_level_source".into(), json!(browse_level_source));
        out.insert("function".into(), json!(debugged_function(&final_state)));
        out.insert(
            "src".into(),
            project_current_src(&final_state).unwrap_or(Value::Null),
        );
    }
    Ok(Some(Value::Object(out)))
}

/// Compact comparable signature of a debugger snapshot: whether we're in a
/// browser, the current function, and the current source line. Two snapshots
/// with the same signature represent the "same place" for step-confirmation.
fn debug_signature(state: &Value) -> (bool, Option<String>, Option<i64>) {
    let in_browser = is_in_debugger(state);
    let func = debugged_function(state);
    let line = state
        .get("call_frames")
        .and_then(Value::as_array)
        .and_then(|f| f.first())
        .and_then(|f| f.get("line_number"))
        .and_then(Value::as_i64);
    (in_browser, func, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_expected_actions() {
        let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
        for expected in ["status", "step", "where", "locals", "src", "exit"] {
            assert!(
                names.contains(&expected),
                "debug.{expected} missing from ACTIONS: {names:?}"
            );
        }
    }

    #[test]
    fn step_allowed_values_match_browser_reference() {
        let step = ACTIONS.iter().find(|a| a.name == "step").unwrap();
        let cmd_param = step
            .params
            .iter()
            .find(|p| p.name == "command")
            .expect("step.command param");
        // Pinned: any drift between the clap `value_parser` (in DebugCmd::Step)
        // and the schema's `allowed` list would let users send unsupported
        // tokens through one surface but not the other.
        for expected in ["n", "s", "f", "c", "Q", "where", "help", "r"] {
            assert!(
                cmd_param.allowed.contains(&expected),
                "step.command schema must list '{expected}': {:?}",
                cmd_param.allowed
            );
        }
    }

    fn state(depth: i64) -> Value {
        json!({
            "context_depth": depth,
            "environment_name": if depth > 0 { "debug_me()" } else { ".GlobalEnv" },
            "call_frames": [
                {
                    "context_depth": 1,
                    "function_name": "debug_me",
                    "call_summary": "debug_me(21)",
                    "file_name": "/tmp/foo.R",
                    "line_number": 42,
                }
            ],
            "environment_list": [
                { "name": "x", "type": "numeric", "clazz": ["numeric","double"],
                  "length": 1, "value": "21" }
            ],
        })
    }

    #[test]
    fn project_call_stack_extracts_src_when_file_present() {
        let s = state(1);
        let stack = project_call_stack(&s);
        assert_eq!(stack.len(), 1);
        let first = &stack[0];
        assert_eq!(first["function"], "debug_me");
        assert_eq!(first["src"]["file"], "/tmp/foo.R");
        assert_eq!(first["src"]["line"], 42);
    }

    #[test]
    fn project_call_stack_drops_src_when_file_empty() {
        let mut s = state(1);
        s["call_frames"][0]["file_name"] = json!("");
        let stack = project_call_stack(&s);
        assert_eq!(stack[0]["src"], Value::Null);
    }

    #[test]
    fn project_locals_projects_to_compact_shape() {
        let s = state(1);
        let l = project_locals(&s);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0]["name"], "x");
        assert_eq!(l[0]["value"], "21");
        assert_eq!(l[0]["length"], 1);
    }

    /// A top-level / side-channel `browser()` (the exact bug-report case):
    /// rsession leaves `context_depth == 0` but the call stack is non-empty.
    /// We MUST still detect the debugger — relying on context_depth alone
    /// (the pre-0.19.3 behaviour) reported `in_browser: false` here and made
    /// `debug step Q` refuse to act.
    fn top_level_browser_state() -> Value {
        json!({
            "context_depth": 0,
            "environment_name": ".GlobalEnv",
            "call_frames": [
                { "context_depth": 1, "function_name": "ℝ",
                  "call_summary": "ℝ(~{ base::browser() })",
                  "file_name": "", "line_number": 0 }
            ],
            "environment_list": [],
        })
    }

    #[test]
    fn is_in_debugger_true_when_context_depth_zero_but_frames_present() {
        let s = top_level_browser_state();
        assert_eq!(context_depth(&s), 0, "precondition: context_depth is 0");
        assert!(
            is_in_debugger(&s),
            "must detect the debugger via non-empty call_frames even when \
             context_depth is 0 (top-level / side-channel browser)"
        );
    }

    #[test]
    fn is_in_debugger_true_for_function_debug() {
        assert!(is_in_debugger(&state(1)));
    }

    #[test]
    fn is_in_debugger_false_when_idle() {
        let idle = json!({
            "context_depth": 0,
            "environment_name": ".GlobalEnv",
            "call_frames": [],
            "environment_list": [],
        });
        assert!(!is_in_debugger(&idle));
    }

    #[test]
    fn debugged_function_is_null_for_top_level_browser() {
        // Must NOT leak the `ℝ` evaluator-wrapper name as "the function".
        let s = top_level_browser_state();
        assert_eq!(
            debugged_function(&s),
            None,
            "top-level browser has no user function being debugged"
        );
    }

    #[test]
    fn debugged_function_strips_parens_for_function_debug() {
        assert_eq!(debugged_function(&state(1)), Some("debug_me".to_string()));
    }

    /// The modulr / debugme case: browser() is overridden and enters via
    /// `do.call(base::browser, …, envir = wrap)`, so rsession reports
    /// environment_name = .GlobalEnv (function can't be named from the env).
    /// We must skip the do.call/browser instrumentation frames and report
    /// the innermost USER function (variance_pop) — never null when a user
    /// call is on the stack.
    fn modulr_sandwich_state() -> Value {
        json!({
            "context_depth": 0,
            "environment_name": ".GlobalEnv",
            "call_frames": [
                { "function_name": "do.call",
                  "call_summary": "do.call(base::browser, args = list(...), envir = wrap)" },
                { "function_name": "browser", "call_summary": "browser()" },
                { "function_name": "variance_pop", "call_summary": "variance_pop(x)" },
                { "function_name": "ecart_type", "call_summary": "ecart_type(d)" },
                { "function_name": "resume_stats", "call_summary": "resume_stats(...)" },
                { "function_name": "ℝ", "call_summary": "ℝ(~{ ... })" },
            ],
            "environment_list": [],
        })
    }

    #[test]
    fn debugged_function_skips_instrumentation_and_finds_user_fn() {
        assert_eq!(
            debugged_function(&modulr_sandwich_state()),
            Some("variance_pop".to_string()),
            "must skip do.call/browser and report the innermost user function"
        );
    }

    #[test]
    fn first_user_function_is_none_when_only_instrumentation() {
        // Top-level `r send 'base::browser()'`: the only frame is the ℝ
        // wrapper — no user function — so we must NOT leak `ℝ`.
        let s = json!({
            "context_depth": 0,
            "environment_name": ".GlobalEnv",
            "call_frames": [{ "function_name": "ℝ", "call_summary": "ℝ(~{ base::browser() })" }],
        });
        assert_eq!(first_user_function(&s), None);
        assert_eq!(debugged_function(&s), None);
    }

    #[test]
    fn is_instrumentation_frame_classifies_known_wrappers() {
        for f in [
            "do.call",
            "browser",
            "ℝ",
            "eval",
            "tryCatch",
            ".rs.foo",
            ".rstudioBar",
        ] {
            assert!(is_instrumentation_frame(f), "{f} should be instrumentation");
        }
        for f in ["variance_pop", "myfun", "ecart_type"] {
            assert!(!is_instrumentation_frame(f), "{f} should be user code");
        }
    }
}
