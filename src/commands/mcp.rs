//! MCP server mode — `rstudio mcp serve`.
//!
//! Exposes the entire CLI surface as Model Context Protocol tools over
//! stdio. The MCP protocol is JSON-RPC 2.0; messages are line-delimited
//! on stdin/stdout.
//!
//! Architecture:
//!
//! - `tools/list` returns a small **core set** only: `meta_version`,
//!   `meta_status`, `tools_search`, and the three `tx_*` controls. The
//!   full action registry (~90 tools) is reachable via `tools/call`
//!   regardless of whether they appear in the list — `tools_search`
//!   reveals additional tools (with full `inputSchema`) on demand. This
//!   is the "progressive discovery" pattern: keep the agent's initial
//!   context lean, let it fan out only into the surface it needs.
//!
//! - The full set is auto-derived from `crate::schema::registry()`.
//!   Each `ActionSpec` becomes one MCP tool, name = `{category}_{action}`
//!   with hyphens replaced by underscores. The MCP `inputSchema` is
//!   built from the `ParamSpec` array.
//!
//! - `tools/call` for an action tool dispatches by spawning the same
//!   binary as a subprocess (`rstudio <category> <action> [args]`). We
//!   parse the JSON envelope from stdout and forward it as the MCP
//!   tool result. Subprocess overhead (~10ms per call) is negligible
//!   for an LLM-paced workflow and lets us reuse 100% of the existing
//!   dispatch + per-call lock infrastructure with zero new code paths.
//!
//! - Three special tools — `tx_begin`, `tx_end`, `tx_run` — manage a
//!   server-held `SessionLock`. While in tx, the server sets
//!   `RSTUDIO_TX_HELD=1` on every subprocess; child rstudio invocations
//!   detect this env var and skip their own per-call lock acquisition
//!   (the parent already holds it). Same fork-inherit pattern as
//!   `rstudio tx -- <child>` from the CLI, just with the MCP server in
//!   the role of "tx parent" instead of a shell.
//!
//! Concurrency story: a Claude Code with this MCP server contends with
//! a shell `rstudio editor write` (or another Claude Code's MCP server)
//! exactly the same way two CLI invocations contend — they all share
//! the per-session `flock` at
//! `~/.config/rstudio-cli/locks/session-<id>.lock`. The kernel arbitrates.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::time::Duration;

use clap::Args;
use serde_json::{Map, Value, json};

use crate::error::CliError;
use crate::lock::{SessionLock, TX_ENV};
use crate::schema::{ActionSpec, ParamKind, ParamSpec, registry};
use crate::session::{Session, SessionOverrides};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// MCP-flavored agent guidance, returned in the `instructions` field
/// of the `initialize` response. The CLI skill (`src/skills/rstudio.md`)
/// targets shell-driven agents; this one targets MCP-driven agents.
/// Different vocabulary (tool names not `rstudio` invocations,
/// `tx_begin`/`tx_end`/`tx_run` not `rstudio tx -- bash`, etc.).
const MCP_SKILL_RAW: &str = include_str!("../skills/rstudio-mcp.md");

fn rendered_mcp_skill() -> String {
    MCP_SKILL_RAW.replace("__VERSION__", crate::VERSION)
}

#[derive(Args, Debug)]
pub struct McpCmd {
    /// Lock timeout (seconds) used when `tx_begin` waits to acquire
    /// the per-session writer lock. Default 30s.
    #[arg(long, default_value_t = 30.0)]
    pub lock_timeout: f64,
}

pub fn run(cmd: &McpCmd, overrides: SessionOverrides) -> Result<i32, CliError> {
    let mut server = McpServer::new(overrides, Duration::from_secs_f64(cmd.lock_timeout));
    server.run_loop()
}

struct McpServer {
    overrides: SessionOverrides,
    /// Detected lazily — we don't fail server startup just because
    /// RStudio isn't running yet (the user might launch it after the
    /// MCP server is spawned). First tool that needs a session forces
    /// detection and caches it.
    session: Option<Session>,
    lock_timeout: Duration,
    /// Held when a `tx_begin` is active. Drop releases the flock via
    /// kernel close on the underlying File descriptor — no manual
    /// release path needed.
    current_tx: Option<SessionLock>,
    /// The core set surfaced in `tools/list` — the progressive-discovery
    /// entrypoint. Other tools are reachable via `tools/call` directly
    /// (and discoverable through `tools_search`).
    core_tools: Vec<McpTool>,
    /// Lookup table for `tools/call` dispatch: maps every MCP tool name
    /// in the registry-derived surface to its underlying ActionSpec.
    actions_by_mcp_name: HashMap<String, &'static ActionSpec>,
}

struct McpTool {
    name: String,
    description: String,
    input_schema: Value,
}

impl McpServer {
    fn new(overrides: SessionOverrides, lock_timeout: Duration) -> Self {
        // Build the registry-derived dispatch table. These actions are NOT
        // surfaced in tools/list — they're discovered via `tools_search`
        // (which delegates to `schema::browse` for DRY consistency with
        // the `rstudio schema` CLI command) and invoked directly via
        // tools/call. The table here exists solely to map MCP tool names
        // back to their ActionSpec when a call comes in.
        let mut actions_by_mcp_name = HashMap::new();
        for action in registry() {
            // meta_tx is documentation for the CLI's `rstudio tx --` —
            // not invokable via MCP. The MCP-native equivalent is the
            // tx_begin / tx_end / tx_run trio added below.
            if action.category == "meta" && action.name == "tx" {
                continue;
            }
            actions_by_mcp_name.insert(mcp_name(action.category, action.name), action);
        }

        // The progressive-discovery core surfaced via tools/list. Two
        // registry-derived bootstrap tools (meta_version, meta_status)
        // plus the MCP-specific glue (tools_search, tx_*).
        //
        // Convention: meta_status and meta_version are baked into the
        // core so an agent can confirm bridge health and version without
        // a discovery round-trip. Everything else is "behind" tools_search.
        let mut core_tools = Vec::new();

        // meta_version + meta_status: pull their schemas from the registry
        // so descriptions stay DRY with `rstudio schema meta <action>`.
        for name in ["version", "status"] {
            if let Some(action) = crate::schema::find("meta", name) {
                let tool_name = mcp_name(action.category, action.name);
                core_tools.push(McpTool {
                    name: tool_name,
                    description: format!("{}\n\n{}", action.summary, action.description),
                    input_schema: build_input_schema(action.params),
                });
            }
        }

        // MCP-specific glue tools.
        core_tools.push(McpTool {
            name: "tools_search".into(),
            description: tools_search_description(),
            input_schema: tools_search_input_schema(),
        });
        core_tools.push(McpTool {
            name: "tx_begin".into(),
            description: "Begin a transaction: acquire the per-session writer lock so subsequent \
                 write tool-calls run atomically with respect to other agents (other MCP \
                 clients or shell `rstudio` invocations). Pair with `tx_end`. Auto-releases \
                 if the MCP server exits."
                .into(),
            input_schema: empty_schema(),
        });
        core_tools.push(McpTool {
            name: "tx_end".into(),
            description: "End the current transaction: release the per-session writer lock. \
                 No-op when no tx is active."
                .into(),
            input_schema: empty_schema(),
        });
        core_tools.push(McpTool {
            name: "tx_run".into(),
            description:
                "Execute multiple tool-calls under a single transaction. Equivalent to \
                 `tx_begin` → operations[] → `tx_end` with auto-cleanup on error. Returns \
                 an array of per-operation results. Use this when you can pre-compute the \
                 entire sequence (e.g. read X, set X transformed). For sequences that \
                 depend on intermediate values you've reasoned about, call `tx_begin` / \
                 individual tools / `tx_end` directly."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "operations": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": {"type": "string"},
                                "arguments": {"type": "object"}
                            },
                            "required": ["tool"]
                        },
                        "description": "Ordered list of tool-calls to execute under one transaction."
                    }
                },
                "required": ["operations"],
                "additionalProperties": false
            }),
        });

        Self {
            overrides,
            session: None,
            lock_timeout,
            current_tx: None,
            core_tools,
            actions_by_mcp_name,
        }
    }

    fn run_loop(&mut self) -> Result<i32, CliError> {
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut out = stdout.lock();
        let reader = BufReader::new(stdin.lock());

        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(_) => break, // stdin closed or read error — graceful shutdown
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(resp) = self.handle_message(&line) {
                writeln!(out, "{resp}")
                    .map_err(|e| CliError::internal(format!("mcp: stdout write: {e}")))?;
                out.flush().ok();
            }
        }
        Ok(0)
    }

    /// Handle one JSON-RPC message line. Returns the response line (if
    /// any — notifications produce no response).
    fn handle_message(&mut self, line: &str) -> Option<Value> {
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_envelope(
                    Value::Null,
                    -32700,
                    &format!("parse error: {e}"),
                ));
            }
        };

        let id = req.get("id").cloned();
        let is_notification = id.is_none();
        let id = id.unwrap_or(Value::Null);
        let method = req
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

        // Notifications get no response, even on error.
        if is_notification {
            // We only need to react to `notifications/initialized` (no-op for us)
            // and `notifications/cancelled` (we don't support cancellation).
            return None;
        }

        let handler_result: Result<Value, CliError> = match method.as_str() {
            "initialize" => Ok(self.handle_initialize()),
            "tools/list" => Ok(self.handle_tools_list()),
            "tools/call" => self.handle_tools_call(params),
            "ping" => Ok(json!({})),
            other => {
                return Some(error_envelope(
                    id,
                    -32601,
                    &format!("method not found: {other}"),
                ));
            }
        };

        match handler_result {
            Ok(value) => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": value,
            })),
            Err(e) => Some(error_envelope(id, -32000, &e.to_string())),
        }
    }

    fn handle_initialize(&self) -> Value {
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "rstudio-cli",
                "version": crate::VERSION,
            },
            // The MCP spec (since 2024-11-05) lets the server return
            // `instructions` here — clients (Claude Code etc.) inject
            // this into the LLM's system context as guidance about
            // the server. We use it to deliver the cross-cutting
            // rules an agent can't infer from per-tool descriptions
            // alone: defensive tx pattern, what NOT to put in tx,
            // R FIFO semantics, hard constraints.
            "instructions": rendered_mcp_skill(),
        })
    }

    fn handle_tools_list(&self) -> Value {
        let tools: Vec<Value> = self.core_tools.iter().map(tool_to_json).collect();
        json!({ "tools": tools })
    }

    fn handle_tools_call(&mut self, params: Value) -> Result<Value, CliError> {
        let name = params
            .get("name")
            .and_then(|x| x.as_str())
            .ok_or_else(|| CliError::user("tools/call: missing 'name'"))?
            .to_string();
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let tool_result = match name.as_str() {
            "tx_begin" => self.tx_begin(),
            "tx_end" => self.tx_end(),
            "tx_run" => self.tx_run(arguments),
            "tools_search" => self.tools_search(arguments),
            other => self.invoke_action(other, arguments),
        };

        // MCP convention: tool execution failure is returned IN the
        // result with isError=true, not as a JSON-RPC error. The LLM
        // sees the message and can adapt.
        match tool_result {
            Ok(value) => Ok(json!({
                "content": [{"type": "text", "text": serde_json::to_string(&value).unwrap_or_default()}],
                "isError": false,
            })),
            Err(e) => Ok(json!({
                "content": [{"type": "text", "text": e.to_string()}],
                "isError": true,
            })),
        }
    }

    fn tx_begin(&mut self) -> Result<Value, CliError> {
        if self.current_tx.is_some() {
            return Err(CliError::user(
                "tx_begin: already in a transaction (call tx_end first)",
            ));
        }
        let session = self.session_or_detect()?;
        let id = session.session_id().ok_or_else(|| {
            CliError::session("tx_begin: cannot derive session id (open RStudio first)")
        })?;
        let lock = SessionLock::acquire(&id, self.lock_timeout, "mcp tx_begin")?;
        self.current_tx = Some(lock);
        Ok(json!({ "ok": true, "in_tx": true }))
    }

    fn tx_end(&mut self) -> Result<Value, CliError> {
        let was_in_tx = self.current_tx.take().is_some();
        Ok(json!({ "ok": true, "was_in_tx": was_in_tx }))
    }

    fn tx_run(&mut self, args: Value) -> Result<Value, CliError> {
        let ops = args
            .get("operations")
            .and_then(|x| x.as_array())
            .cloned()
            .ok_or_else(|| CliError::user("tx_run: 'operations' must be an array"))?;

        let auto_release = self.current_tx.is_none();
        if auto_release {
            self.tx_begin()?;
        }

        let mut results = Vec::with_capacity(ops.len());
        for (i, op) in ops.iter().enumerate() {
            let tool = op
                .get("tool")
                .and_then(|x| x.as_str())
                .ok_or_else(|| CliError::user(format!("tx_run: op[{i}] missing 'tool' string")))?;
            let arguments = op.get("arguments").cloned().unwrap_or(json!({}));
            match self.invoke_action(tool, arguments) {
                Ok(r) => results.push(json!({ "ok": true, "result": r })),
                Err(e) => {
                    if auto_release {
                        self.current_tx = None;
                    }
                    return Err(CliError::user(format!(
                        "tx_run: op[{i}] '{tool}' failed: {e}"
                    )));
                }
            }
        }

        if auto_release {
            self.tx_end().ok();
        }
        Ok(json!({ "results": results }))
    }

    /// Progressive-discovery 3-level drill-down across the full tool
    /// catalog. Delegates to `schema::browse` so this stays DRY with
    /// `rstudio schema` — the same logic powers the CLI subcommand and
    /// this MCP tool.
    ///
    /// Arguments mirror `rstudio schema`:
    ///
    /// - no args                       → level 0: categories only (cheapest)
    /// - `{search: "regex"}`           → level 0 filtered: matching actions
    ///   across all categories
    /// - `{category: "editor"}`        → level 1: actions in that category
    /// - `{category, action}`          → level 2: full ActionSpec for one
    ///   action, augmented with the MCP `inputSchema`.
    ///
    /// At level 2 the response also includes `mcp_tool_name` (the underscored
    /// form the agent must use in `tools/call`) so the agent doesn't have to
    /// translate `category` + `action` itself.
    fn tools_search(&self, args: Value) -> Result<Value, CliError> {
        let search = args
            .get("search")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let regex = match search.as_deref() {
            Some(pat) => Some(
                regex_lite::Regex::new(pat)
                    .map_err(|e| CliError::user(format!("invalid 'search' regex: {e}")))?,
            ),
            None => None,
        };
        let matcher = regex.as_ref().map(|r| {
            let r = r.clone();
            move |s: &str| r.is_match(s)
        });
        let matcher_ref: Option<&dyn Fn(&str) -> bool> =
            matcher.as_ref().map(|m| m as &dyn Fn(&str) -> bool);

        let result = crate::schema::browse(category.as_deref(), action.as_deref(), matcher_ref)
            .map_err(|e| CliError::user(e.to_string()))?;

        // For level 2, augment the schema-side ActionSpec with the MCP
        // `inputSchema` and the underscored tool name. This is the bridge
        // between the CLI vocabulary (category/action) and the MCP one
        // (flat `<category>_<action>` tool names).
        let value = match result {
            crate::schema::BrowseResult::Action(spec, mut json_value) => {
                if let Some(obj) = json_value.as_object_mut() {
                    obj.insert(
                        "mcp_tool_name".into(),
                        json!(mcp_name(spec.category, spec.name)),
                    );
                    obj.insert("input_schema".into(), build_input_schema(spec.params));
                }
                json_value
            }
            other => other.into_value(),
        };
        Ok(value)
    }

    /// Spawn `rstudio <category> <action> [args]` as a subprocess and
    /// forward its JSON envelope output. The subprocess inherits
    /// `RSTUDIO_TX_HELD=1` when we're in tx — this is the same
    /// fork-inherit trick `rstudio tx -- <child>` uses, so the child's
    /// dispatch detects the env var and skips its own per-call lock
    /// acquisition (the MCP server already holds the lock).
    fn invoke_action(&mut self, mcp_name: &str, args: Value) -> Result<Value, CliError> {
        let action = *self
            .actions_by_mcp_name
            .get(mcp_name)
            .ok_or_else(|| CliError::user(format!("unknown tool: {mcp_name}")))?;

        let argv = build_argv(action, &args)?;
        let bin = current_exe_path()?;

        let mut cmd = StdCommand::new(&bin);
        cmd.arg("--format").arg("json");
        cmd.args(&argv);
        if self.current_tx.is_some() {
            cmd.env(TX_ENV, "1");
        }

        let output = cmd
            .output()
            .map_err(|e| CliError::internal(format!("mcp: spawn {}: {e}", bin.display())))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

        // The CLI emits exactly one JSON envelope on stdout (the
        // AI-native contract). Parse and forward.
        let mut parsed: Value = serde_json::from_str(stdout.trim()).map_err(|e| {
            CliError::internal(format!(
                "mcp: subprocess output not JSON: {e}; stdout={stdout}"
            ))
        })?;
        // Propagate subprocess errors as Err so handle_tools_call sets isError=true.
        propagate_ok_false(&parsed)?;
        // Inject update notice into every tool response so the agent sees it
        // regardless of which tool it called (not just meta_status).
        if let Some(info) = crate::update_check::check(crate::VERSION)
            && let Some(obj) = parsed.as_object_mut()
        {
            obj.insert("_update_available".to_string(), json!(info.latest));
        }
        Ok(parsed)
    }

    fn session_or_detect(&mut self) -> Result<&Session, CliError> {
        if self.session.is_none() {
            self.session = Some(Session::detect(self.overrides.clone())?);
        }
        Ok(self.session.as_ref().unwrap())
    }
}

fn current_exe_path() -> Result<PathBuf, CliError> {
    std::env::current_exe().map_err(|e| CliError::internal(format!("mcp: current_exe: {e}")))
}

/// Convert `(category, action)` to an MCP-safe tool name. MCP names
/// must match the regex `[a-zA-Z0-9_-]{1,64}`; we use `_` separator
/// and replace any `-` in either part with `_` for consistency.
fn mcp_name(category: &str, action: &str) -> String {
    format!(
        "{}_{}",
        category.replace('-', "_"),
        action.replace('-', "_")
    )
}

fn empty_schema() -> Value {
    json!({"type": "object", "properties": {}, "additionalProperties": false})
}

fn tool_to_json(t: &McpTool) -> Value {
    json!({
        "name": t.name,
        "description": t.description,
        "inputSchema": t.input_schema,
    })
}

fn tools_search_description() -> String {
    "Discover the ~90 RStudio tools beyond the core `tools/list`. \
     3-level drill-down, DRY with the `rstudio schema` CLI command:\n\n\
     - No args → catalog: list of categories with `action_count`. Cheap \
     overview (~450 tokens). Pick a category, then call again with \
     `category=...`.\n\
     - `{category: \"editor\"}` → level 1: actions in that category as \
     `[{name, summary, param_count}]`.\n\
     - `{category, action}` → level 2: full ActionSpec (params, examples, \
     errors, return type) plus the MCP `input_schema` and `mcp_tool_name` \
     ready for `tools/call`.\n\
     - `{search: \"regex\"}` (level 0 only) → matching actions across all \
     categories. Regex applies to category|name|summary.\n\n\
     Categories: editor, r, console, term, env, pane, skill, project, \
     session, pref, job, ui, observe, policy, meta.\n\n\
     Tip: if you already know a tool's name from naming convention \
     (`<category>_<action>`, e.g. `editor_read_buffer`), call it directly \
     via `tools/call` — no need to search first."
        .to_string()
}

fn tools_search_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "category": {
                "type": "string",
                "description": "Drill into one category (level 1). Required for level 2."
            },
            "action": {
                "type": "string",
                "description": "Drill into one action of `category` (level 2). Returns the full ActionSpec + MCP input_schema."
            },
            "search": {
                "type": "string",
                "description": "Regex applied to category|name|summary. Level 0 only — combine with `category` for that, or use level 1 directly."
            }
        },
        "additionalProperties": false
    })
}

/// Build a JSON Schema for a tool's `inputSchema` field from our
/// `ParamSpec` array. Positional and `--flag` params both flatten to
/// JSON object properties keyed by the bare name (without leading
/// dashes).
fn build_input_schema(params: &[ParamSpec]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for p in params {
        let key = p.name.trim_start_matches('-').to_string();
        let mut prop = match p.kind {
            ParamKind::String => json!({"type": "string"}),
            ParamKind::Integer => json!({"type": "integer"}),
            ParamKind::Number => json!({"type": "number"}),
            ParamKind::Bool => json!({"type": "boolean"}),
            ParamKind::Json => json!({"anyOf": [{"type": "object"}, {"type": "array"}]}),
            ParamKind::Enum => {
                let allowed: Vec<Value> = p.allowed.iter().map(|s| json!(s)).collect();
                json!({"type": "string", "enum": allowed})
            }
        };
        if let Some(obj) = prop.as_object_mut() {
            obj.insert("description".to_string(), json!(p.description));
            if let Some(default) = p.default {
                obj.insert("default".to_string(), json!(default));
            }
        }
        properties.insert(key.clone(), prop);
        if p.required {
            required.push(json!(key));
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Convert MCP `arguments` (JSON object) into the equivalent CLI
/// argv tail: `[<category>, <action>, ...positionals..., --flag value, ...]`.
/// Positionals are emitted in the order their `ParamSpec` appears.
/// Bool flags are emitted as bare `--flag` only when the value is true.
fn build_argv(action: &ActionSpec, args: &Value) -> Result<Vec<String>, CliError> {
    let args_obj = args.as_object().cloned().unwrap_or_default();

    // The `meta` schema category is documentation-only: its actions
    // (`version`, `status`) map to TOP-LEVEL CLI commands without a
    // category prefix. So `meta_status` invokes `rstudio status`,
    // not `rstudio meta status`.
    let mut argv = if action.category == "meta" {
        vec![action.name.to_string()]
    } else {
        vec![action.category.to_string(), action.name.to_string()]
    };
    for spec in action.params {
        let key = spec.name.trim_start_matches('-');
        let Some(v) = args_obj.get(key) else { continue };

        let is_flag = spec.name.starts_with("--");
        if is_flag && matches!(spec.kind, ParamKind::Bool) {
            if v.as_bool().unwrap_or(false) {
                argv.push(spec.name.to_string());
            }
        } else if is_flag {
            argv.push(spec.name.to_string());
            argv.push(value_to_cli_string(v));
        } else {
            argv.push(value_to_cli_string(v));
        }
    }
    Ok(argv)
}

fn value_to_cli_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        // For objects / arrays / null, serialise the JSON.
        _ => v.to_string(),
    }
}

fn error_envelope(id: Value, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

/// Convert a subprocess `{"ok": false, "error": {"message": ...}}` envelope
/// into a `CliError`. Returns `Ok(())` for any other shape (including `ok: true`
/// or envelopes where the `ok` field is absent or non-bool).
fn propagate_ok_false(parsed: &Value) -> Result<(), CliError> {
    if parsed.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        let message = parsed
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("subprocess error")
            .to_string();
        return Err(CliError::internal(message));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_name_replaces_dashes() {
        assert_eq!(mcp_name("editor", "read-buffer"), "editor_read_buffer");
        assert_eq!(mcp_name("set-marks", "go"), "set_marks_go");
    }

    #[test]
    fn build_argv_positional_and_flag() {
        // Synthetic ActionSpec: `editor.open` with `path` (positional)
        // and `--line` (flag).
        let params: &[ParamSpec] = &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "",
            },
            ParamSpec {
                name: "--line",
                kind: ParamKind::Integer,
                required: false,
                default: None,
                allowed: &[],
                description: "",
            },
        ];
        let action = ActionSpec {
            category: "editor",
            name: "open",
            summary: "",
            description: "",
            params,
            examples: &[],
            returns: "",
            errors: &[],
            rstudioapi_fn: None,
            rpc_method: None,
        };

        let args = json!({"path": "/tmp/foo.R", "line": 42});
        let argv = build_argv(&action, &args).unwrap();
        assert_eq!(argv, vec!["editor", "open", "/tmp/foo.R", "--line", "42"]);
    }

    #[test]
    fn build_argv_bool_flag_true_emits_bare_flag() {
        let params: &[ParamSpec] = &[ParamSpec {
            name: "--once",
            kind: ParamKind::Bool,
            required: false,
            default: None,
            allowed: &[],
            description: "",
        }];
        let action = ActionSpec {
            category: "observe",
            name: "stream",
            summary: "",
            description: "",
            params,
            examples: &[],
            returns: "",
            errors: &[],
            rstudioapi_fn: None,
            rpc_method: None,
        };

        let argv = build_argv(&action, &json!({"once": true})).unwrap();
        assert_eq!(argv, vec!["observe", "stream", "--once"]);
        let argv = build_argv(&action, &json!({"once": false})).unwrap();
        assert_eq!(argv, vec!["observe", "stream"]);
        let argv = build_argv(&action, &json!({})).unwrap();
        assert_eq!(argv, vec!["observe", "stream"]);
    }

    #[test]
    fn build_input_schema_basic() {
        let params: &[ParamSpec] = &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Document id",
            },
            ParamSpec {
                name: "--tier",
                kind: ParamKind::Enum,
                required: false,
                default: Some("2"),
                allowed: &["1", "2", "3"],
                description: "Coverage tier",
            },
        ];
        let schema = build_input_schema(params);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["id"]["type"], "string");
        assert_eq!(schema["properties"]["tier"]["type"], "string");
        assert_eq!(schema["properties"]["tier"]["enum"], json!(["1", "2", "3"]));
        assert_eq!(schema["properties"]["tier"]["default"], "2");
        assert_eq!(schema["required"], json!(["id"]));
    }

    #[test]
    fn handle_initialize_returns_capabilities() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let init = server.handle_initialize();
        assert_eq!(init["protocolVersion"], PROTOCOL_VERSION);
        assert!(init["capabilities"]["tools"].is_object());
        assert_eq!(init["serverInfo"]["name"], "rstudio-cli");
    }

    #[test]
    fn initialize_includes_mcp_skill_instructions() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let init = server.handle_initialize();
        let instructions = init["instructions"]
            .as_str()
            .expect("initialize must carry `instructions` string");
        // Sanity: contains the cross-cutting rules an agent needs.
        assert!(instructions.contains("tx_begin"));
        assert!(instructions.contains("tx_end"));
        assert!(instructions.contains("multi-call sequence"));
        assert!(instructions.contains("client_init"));
        // Version substitution actually happened.
        assert!(instructions.contains(crate::VERSION));
        assert!(!instructions.contains("__VERSION__"));
    }

    #[test]
    fn tools_list_returns_only_core_set() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let tools = server.handle_tools_list();
        let arr = tools["tools"].as_array().unwrap();
        let names: Vec<&str> = arr.iter().map(|t| t["name"].as_str().unwrap()).collect();

        // The progressive-discovery core: small, fixed, predictable.
        let expected = [
            "meta_version",
            "meta_status",
            "tools_search",
            "tx_begin",
            "tx_end",
            "tx_run",
        ];
        for name in expected {
            assert!(names.contains(&name), "core tool missing: {name}");
        }
        assert_eq!(names.len(), expected.len(), "tools/list grew beyond core");

        // Registry-derived tools must NOT appear in tools/list anymore.
        assert!(!names.iter().any(|n| n.starts_with("editor_")));
        assert!(!names.iter().any(|n| n.starts_with("r_") && *n != "tx_run"));
        assert!(!names.iter().any(|n| n.starts_with("observe_")));
    }

    #[test]
    fn dispatch_table_covers_registry() {
        // Every action in the registry (minus meta.tx) must be invocable
        // via tools/call, even though most aren't surfaced in tools/list.
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        assert!(
            server
                .actions_by_mcp_name
                .contains_key("editor_read_buffer")
        );
        assert!(server.actions_by_mcp_name.contains_key("r_exec"));
        assert!(server.actions_by_mcp_name.contains_key("observe_events"));
        // meta_tx is documentation-only — not a callable MCP tool.
        assert!(!server.actions_by_mcp_name.contains_key("meta_tx"));
    }

    #[test]
    fn tools_search_level0_returns_categories_only() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let r = server.tools_search(json!({})).unwrap();
        let cats = r["categories"]
            .as_array()
            .expect("level 0 must list categories");
        assert!(!cats.is_empty());
        // No action-level info leaks into level 0 (that's the whole point).
        assert!(
            r.get("actions").is_none(),
            "level 0 must not include actions array"
        );
        // Each category carries name, description, action_count.
        let editor = cats
            .iter()
            .find(|c| c["name"] == "editor")
            .expect("'editor' category missing");
        assert!(editor["description"].is_string());
        let count = editor["action_count"]
            .as_u64()
            .expect("action_count missing");
        assert!(count > 0, "editor should have at least one action");
    }

    #[test]
    fn tools_search_level1_lists_category_actions() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let r = server.tools_search(json!({"category": "editor"})).unwrap();
        assert_eq!(r["category"], "editor");
        let actions = r["actions"].as_array().expect("level 1 must list actions");
        assert!(!actions.is_empty());
        // Level 1 carries name + summary + counts (not the full ActionSpec).
        let first = &actions[0];
        assert!(first["name"].is_string());
        assert!(first["summary"].is_string());
        assert!(first["param_count"].is_number());
        // No leak of level-2 detail.
        assert!(first.get("description").is_none());
        assert!(first.get("examples").is_none());
    }

    #[test]
    fn tools_search_level1_unknown_category_errors() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let err = server
            .tools_search(json!({"category": "nope-no-such"}))
            .unwrap_err();
        assert!(err.to_string().contains("unknown category"));
    }

    #[test]
    fn tools_search_level2_returns_full_actionspec_with_mcp_schema() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let r = server
            .tools_search(json!({"category": "editor", "action": "read-buffer"}))
            .unwrap();
        // ActionSpec fields are present.
        assert_eq!(r["category"], "editor");
        assert_eq!(r["name"], "read-buffer");
        assert!(r["params"].is_array());
        assert!(r["examples"].is_array());
        assert!(r["returns"].is_string());
        // MCP-side augmentation: the agent gets the underscored tool name
        // and the JSON-Schema-shaped input_schema, ready for tools/call.
        assert_eq!(r["mcp_tool_name"], "editor_read_buffer");
        assert!(r["input_schema"].is_object());
        assert_eq!(r["input_schema"]["type"], "object");
    }

    #[test]
    fn tools_search_search_regex_matches_across_categories() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let r = server.tools_search(json!({"search": "buffer"})).unwrap();
        // search at level 0: returns matching actions as flat list, no categories.
        let actions = r["actions"].as_array().expect("search must return actions");
        assert!(
            !actions.is_empty(),
            "expected at least one buffer-related action"
        );
        // Each entry is the level-0 shape (category, name, summary).
        let first = &actions[0];
        assert!(first["category"].is_string());
        assert!(first["name"].is_string());
        assert!(first["summary"].is_string());
    }

    #[test]
    fn tools_search_invalid_regex_returns_user_error() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let err = server
            .tools_search(json!({"search": "[unclosed"}))
            .unwrap_err();
        assert!(err.to_string().contains("invalid 'search' regex"));
    }

    #[test]
    fn tools_search_action_without_category_errors() {
        let server = McpServer::new(SessionOverrides::default(), Duration::from_secs(30));
        let err = server
            .tools_search(json!({"action": "read-buffer"}))
            .unwrap_err();
        assert!(
            err.to_string().contains("category"),
            "expected error mentioning category requirement, got: {err}"
        );
    }

    // Fix #3: ParamKind::Json must accept both arrays and objects in the schema.
    #[test]
    fn build_input_schema_json_param_uses_anyof() {
        let params: &[ParamSpec] = &[ParamSpec {
            name: "--markers",
            kind: ParamKind::Json,
            required: false,
            default: None,
            allowed: &[],
            description: "JSON array of marker objects.",
        }];
        let schema = build_input_schema(params);
        let prop = &schema["properties"]["markers"];
        let any_of = prop["anyOf"].as_array().expect("Json param must use anyOf");
        let types: Vec<&str> = any_of.iter().map(|v| v["type"].as_str().unwrap()).collect();
        assert!(types.contains(&"array"), "anyOf must include array");
        assert!(types.contains(&"object"), "anyOf must include object");
        // Description must still be propagated alongside anyOf.
        assert_eq!(prop["description"], "JSON array of marker objects.");
    }

    // Fix #1: propagate_ok_false must turn ok:false into Err, and leave ok:true alone.
    #[test]
    fn propagate_ok_false_returns_err_with_message() {
        let v = json!({
            "ok": false,
            "error": {"code": 6, "kind": "rpc_error",
                      "message": "jsonrpc error 6 (Invalid json-rpc request)"}
        });
        let err = propagate_ok_false(&v).unwrap_err();
        assert_eq!(err.message, "jsonrpc error 6 (Invalid json-rpc request)");
    }

    #[test]
    fn propagate_ok_false_falls_back_when_message_absent() {
        let v = json!({"ok": false});
        let err = propagate_ok_false(&v).unwrap_err();
        assert_eq!(err.message, "subprocess error");
    }

    #[test]
    fn propagate_ok_false_is_noop_for_ok_true() {
        let v = json!({"ok": true, "result": {"count": 3}});
        assert!(propagate_ok_false(&v).is_ok());
    }

    #[test]
    fn propagate_ok_false_is_noop_when_ok_absent() {
        // Envelopes that don't carry an `ok` field at all (e.g. partial
        // JSON or legacy format) must pass through without error.
        let v = json!({"result": "foo"});
        assert!(propagate_ok_false(&v).is_ok());
    }
}
