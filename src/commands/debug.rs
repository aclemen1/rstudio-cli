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

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::rpc::RpcClient;
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "debug",
        name: "status",
        summary: "Read the active R debugger state: depth, frame, locals, full call stack.",
        description: "Single-RPC projection of `get_environment_state`. \
                      When R is at a Browse[n]> prompt, returns the depth, \
                      the current frame (function name + source location), \
                      the typed locals of that frame (already class+value \
                      serialized by rsession), and the call stack with one \
                      entry per frame (innermost first). When R is idle, \
                      returns {in_browser: false} and no further fields.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio debug status",
            explanation: "Returns {in_browser, depth?, function?, locals?, call_stack?}.",
        }],
        returns: "{in_browser: bool, depth?: int, function?: string, \
                  src?: {file, line}, locals?: [{name, type, class, value}], \
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
                      Returns immediately — call `debug status` afterwards \
                      if you need to confirm the new state.",
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
                explanation: "Step to the next statement at the current call level.",
            },
            ExampleSpec {
                cmd: "rstudio debug step c",
                explanation: "Continue evaluation; exits one browser level.",
            },
        ],
        returns: "{sent: string}",
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
                      browser frames in one step. Function returns (or errors, \
                      depending on R's contract) immediately. Alias of `debug step Q`.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio debug exit",
            explanation: "Bails out of the debugger; call `debug status` to confirm.",
        }],
        returns: "{sent: \"Q\"}",
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

fn context_depth(state: &Value) -> i64 {
    state
        .get("context_depth")
        .and_then(Value::as_i64)
        .unwrap_or(0)
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

fn status(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let state = fetch_state(rpc)?;
    let depth = context_depth(&state);
    if depth <= 0 {
        return Ok(Some(json!({ "in_browser": false })));
    }
    let function = state
        .get("environment_name")
        .and_then(Value::as_str)
        .map(|s| s.trim_end_matches("()").to_string());
    Ok(Some(json!({
        "in_browser": true,
        "depth": depth,
        "function": function,
        "src": project_current_src(&state),
        "locals": project_locals(&state),
        "call_stack": project_call_stack(&state),
    })))
}

fn where_cmd(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let state = fetch_state(rpc)?;
    let depth = context_depth(&state);
    if depth <= 0 {
        return Ok(Some(json!({ "in_browser": false })));
    }
    Ok(Some(json!({
        "in_browser": true,
        "call_stack": project_call_stack(&state),
    })))
}

fn locals(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let state = fetch_state(rpc)?;
    let depth = context_depth(&state);
    if depth <= 0 {
        return Ok(Some(json!({ "in_browser": false, "locals": [] })));
    }
    Ok(Some(json!({
        "in_browser": true,
        "locals": project_locals(&state),
    })))
}

fn src(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let state = fetch_state(rpc)?;
    let depth = context_depth(&state);
    if depth <= 0 {
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
    // variable. Cheap pre-check via get_environment_state.
    let state = fetch_state(rpc)?;
    if context_depth(&state) <= 0 {
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
    Ok(Some(json!({ "sent": command })))
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
}
