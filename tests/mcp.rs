//! End-to-end integration tests for `rstudio mcp` (the MCP server
//! mode, JSON-RPC over stdio).
//!
//! Most tests here run without a live RStudio session: they exercise
//! the MCP protocol layer (initialize, tools/list, ping, malformed
//! input, unknown methods) and tools that don't need a session
//! (observe_events). A handful of tests need RStudio for actual
//! tool-call dispatch (editor_list, tx_begin); they skip cleanly
//! when no session is reachable.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, MutexGuard};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_rstudio");

/// MCP tests that take or observe the per-session lock must serialise,
/// just like tests/locking.rs. The protocol-level tests (initialize,
/// tools/list, malformed input) don't need this.
static LOCK_SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> MutexGuard<'static, ()> {
    LOCK_SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// Helper to drive the MCP server: spawn, send a request, read the
/// matching reply (by id), wait, kill, return the parsed reply.
struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpClient {
    fn spawn() -> Self {
        let mut child = Command::new(BIN)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rstudio mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, request: &Value) {
        let line = format!("{request}\n");
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    /// Read the next JSON line from the server. None on EOF.
    fn next_response(&mut self) -> Option<Value> {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).ok()?;
        if n == 0 {
            return None;
        }
        serde_json::from_str(line.trim()).ok()
    }

    fn shutdown(mut self) {
        // Closing stdin signals EOF; the server's read loop exits.
        drop(self.stdin);
        // Read any remaining responses.
        while self
            .stdout
            .read_line(&mut String::new())
            .is_ok_and(|n| n > 0)
        {}
        let _ = self.child.wait();
    }
}

fn rstudio_available() -> bool {
    Command::new(BIN)
        .arg("status")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn initialize_returns_capabilities_and_server_info() {
    let mut c = McpClient::spawn();
    c.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test"}}
    }));
    let resp = c.next_response().expect("response");
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    assert!(resp["result"]["capabilities"]["tools"].is_object());
    assert_eq!(resp["result"]["serverInfo"]["name"], "rstudio-cli");
    c.shutdown();
}

#[test]
fn initialize_carries_mcp_skill_instructions() {
    let mut c = McpClient::spawn();
    c.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test"}}
    }));
    let resp = c.next_response().unwrap();
    let instructions = resp["result"]["instructions"]
        .as_str()
        .expect("initialize must carry instructions");
    // Spot-check the cross-cutting rules an agent connected via this
    // server needs to know — these are exactly the things they CAN'T
    // deduce from per-tool descriptions alone.
    assert!(
        instructions.contains("tx_begin"),
        "instructions mention tx_begin"
    );
    assert!(
        instructions.contains("Always wrap"),
        "instructions state the defensive multi-call rule"
    );
    assert!(
        instructions.contains("client_init"),
        "instructions warn about client_init blacklist"
    );
    // Version substitution wired correctly.
    assert!(!instructions.contains("__VERSION__"));
    c.shutdown();
}

#[test]
fn tools_list_exposes_core_and_registry_with_alwaysload_on_core() {
    let mut c = McpClient::spawn();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let resp = c.next_response().unwrap();
    let tools = resp["result"]["tools"].as_array().unwrap();
    let names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    // tools/list exposes BOTH the bootstrap core (annotated with
    // _meta["anthropic/alwaysLoad"]=true so Claude Code keeps their full
    // schema in the LLM's catalog) AND every registry-derived action (no
    // alwaysLoad — Claude Code in tst mode defers them and pulls schemas
    // through ToolSearch on demand). Earlier server versions exposed only
    // the 7-tool core and broke under Claude Code's ToolSearchTool, which
    // refused to dispatch tools absent from tools/list.

    // Bootstrap core (must carry alwaysLoad).
    let core = [
        "meta_version",
        "meta_status",
        "tools_search",
        "tx_begin",
        "tx_end",
        "tx_run",
        "r_script",
    ];
    for n in core {
        assert!(names.contains(&n.to_string()), "missing core tool {n}");
        let t = tools.iter().find(|t| t["name"] == n).unwrap();
        assert_eq!(
            t["_meta"]["anthropic/alwaysLoad"], true,
            "core tool {n} missing _meta[\"anthropic/alwaysLoad\"]=true"
        );
    }

    // Registry-derived sample: a few tools from different categories must
    // appear, WITHOUT alwaysLoad (so Claude Code defers them).
    let registry = [
        "editor_open",
        "editor_read_buffer",
        "r_exec",
        "r_send",
        "env_list",
        "term_list",
    ];
    for n in registry {
        assert!(
            names.contains(&n.to_string()),
            "missing registry tool {n} from tools/list"
        );
        let t = tools.iter().find(|t| t["name"] == n).unwrap();
        // alwaysLoad should be absent (or false) — Claude Code's default
        // is to defer MCP tools, which is what we want for the catalog.
        let has_always_load = t["_meta"]
            .get("anthropic/alwaysLoad")
            .and_then(|v| v.as_bool())
            == Some(true);
        assert!(
            !has_always_load,
            "registry tool {n} should NOT carry alwaysLoad (token budget)"
        );
    }

    // meta_tx must NOT appear (it documents the CLI's `rstudio tx --`;
    // the MCP equivalent is the tx_begin/tx_end/tx_run trio).
    assert!(!names.contains(&"meta_tx".to_string()));

    // No duplicates: each name appears exactly once.
    let mut sorted = names.clone();
    sorted.sort();
    let unique_count = {
        let mut s = sorted.clone();
        s.dedup();
        s.len()
    };
    assert_eq!(
        names.len(),
        unique_count,
        "duplicate tool names in tools/list: {sorted:?}"
    );

    c.shutdown();
}

#[test]
fn ping_returns_empty_object() {
    let mut c = McpClient::spawn();
    c.send(&json!({"jsonrpc": "2.0", "id": 7, "method": "ping"}));
    let resp = c.next_response().unwrap();
    assert_eq!(resp["id"], 7);
    assert_eq!(resp["result"], json!({}));
    c.shutdown();
}

#[test]
fn unknown_method_returns_method_not_found() {
    let mut c = McpClient::spawn();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "no/such/method"}));
    let resp = c.next_response().unwrap();
    assert_eq!(resp["error"]["code"], -32601);
    c.shutdown();
}

#[test]
fn parse_error_returns_minus_32700() {
    let mut c = McpClient::spawn();
    c.stdin.write_all(b"this is not json\n").unwrap();
    c.stdin.flush().unwrap();
    let resp = c.next_response().unwrap();
    assert_eq!(resp["error"]["code"], -32700);
    c.shutdown();
}

#[test]
fn notification_produces_no_response() {
    let mut c = McpClient::spawn();
    // Send a notification (no id), then a real request. The server
    // should produce only the second response.
    c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    c.send(&json!({"jsonrpc": "2.0", "id": 99, "method": "ping"}));
    let resp = c.next_response().unwrap();
    assert_eq!(resp["id"], 99);
    c.shutdown();
}

#[test]
fn tools_call_observe_events_works_without_rstudio() {
    let mut c = McpClient::spawn();
    c.send(&json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "observe_events", "arguments": {}}
    }));
    let resp = c.next_response().unwrap();
    assert_eq!(resp["result"]["isError"], false);
    let inner: Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(inner["result"]["events"].is_array());
    c.shutdown();
}

#[test]
fn tools_call_unknown_tool_marks_iserror_true() {
    let mut c = McpClient::spawn();
    c.send(&json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "no_such_tool", "arguments": {}}
    }));
    let resp = c.next_response().unwrap();
    // MCP convention: tool execution errors come back IN the result
    // with isError: true (not as JSON-RPC errors).
    assert_eq!(resp["result"]["isError"], true);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("unknown tool"));
    c.shutdown();
}

#[test]
fn tx_begin_then_end_works() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let mut c = McpClient::spawn();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": "tx_begin", "arguments": {}}}));
    let resp = c.next_response().unwrap();
    assert_eq!(resp["result"]["isError"], false);
    let begin_payload: Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(begin_payload["in_tx"], true);

    c.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "tx_end", "arguments": {}}}));
    let resp = c.next_response().unwrap();
    assert_eq!(resp["result"]["isError"], false);
    c.shutdown();
}

#[test]
fn tx_begin_twice_errors() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let mut c = McpClient::spawn();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": "tx_begin", "arguments": {}}}));
    c.next_response();
    c.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "tx_begin", "arguments": {}}}));
    let resp = c.next_response().unwrap();
    assert_eq!(resp["result"]["isError"], true);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("already in a transaction"));
    c.shutdown();
}

#[test]
fn tx_status_visible_inside_tx() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let mut c = McpClient::spawn();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": "tx_begin", "arguments": {}}}));
    c.next_response();

    // While in tx, meta_status should report inside_tx=true and the
    // lock as held by us.
    c.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "meta_status", "arguments": {}}}));
    let resp = c.next_response().unwrap();
    let status: Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(status["result"]["session"]["lock"]["state"], "held");
    assert_eq!(status["result"]["session"]["lock"]["inside_tx"], true);

    c.send(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                   "params": {"name": "tx_end", "arguments": {}}}));
    c.next_response();
    c.shutdown();
}

#[test]
fn tx_run_executes_multiple_ops() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let mut c = McpClient::spawn();
    c.send(&json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tx_run",
            "arguments": {
                "operations": [
                    {"tool": "editor_list", "arguments": {}},
                    {"tool": "meta_status", "arguments": {}},
                    {"tool": "observe_events", "arguments": {}}
                ]
            }
        }
    }));
    let resp = c.next_response().unwrap();
    assert_eq!(resp["result"]["isError"], false);
    let payload: Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let results = payload["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    for r in results {
        assert_eq!(r["ok"], true);
    }
    c.shutdown();
}

// Regression test for an observed agent failure: registry-derived tools
// (e.g. `editor_new`, `r_send`) MUST be invocable directly via tools/call
// — no detour through tx_run, no failed dispatch from Claude Code's
// ToolSearchTool. Earlier versions hid them from tools/list, which broke
// dispatch under tst mode. This test now asserts the full path:
//
//   1. tools/list contains the registry tool (even if without alwaysLoad).
//   2. tools_search returns its mcp_tool_name for agents that prefer the
//      DRY discovery path.
//   3. tools/call invokes it successfully without tx_run.
//
// No live RStudio session needed: we use the offline `observe_events` tool
// which deterministically returns a catalog without touching rsession.
#[test]
fn registry_tool_callable_directly_after_tools_search() {
    let mut c = McpClient::spawn();

    // Step 1: confirm observe_events IS in tools/list now that progressive
    // discovery has been replaced by deferred-tools with alwaysLoad on the
    // core only.
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let resp = c.next_response().unwrap();
    let names: Vec<String> = resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        names.contains(&"observe_events".to_string()),
        "observe_events missing from tools/list — clients can no longer dispatch it: {names:?}"
    );

    // Step 2: discover via tools_search. The response must carry an
    // mcp_tool_name field that the agent can plug straight into tools/call.
    c.send(&json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "tools/call",
        "params": {"name": "tools_search",
                   "arguments": {"category": "observe", "action": "events"}}
    }));
    let resp = c.next_response().unwrap();
    let spec: Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        spec["mcp_tool_name"], "observe_events",
        "tools_search must surface mcp_tool_name at the top level so the \
         agent can plug it straight into tools/call without transcribing \
         hyphens to underscores"
    );

    // Step 3: invoke the tool by name via tools/call DIRECT. No tx_run,
    // no tools_search round-trip beyond the discovery one above. This is
    // the canonical agent workflow documented in the MCP skill.
    c.send(&json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "tools/call",
        "params": {"name": "observe_events", "arguments": {}}
    }));
    let resp = c.next_response().unwrap();
    assert_eq!(
        resp["result"]["isError"], false,
        "direct tools/call on a registry tool must succeed without tx_run"
    );

    c.shutdown();
}

// Companion to the above: documents that putting a single tool-call inside
// tx_run is *valid* but unnecessary — and asserts both shapes return the
// same payload. If the skill docs ever start telling agents to route
// everything through tx_run, this test will keep both paths working at
// least, even while we educate.
//
// Requires a live RStudio session: tx_run acquires the per-session writer
// lock, which fails fast on CI runners with no rsession reachable.
#[test]
fn tx_run_with_one_op_is_equivalent_to_direct_tools_call() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let mut c = McpClient::spawn();

    // Direct call.
    c.send(&json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "observe_events", "arguments": {}}
    }));
    let direct = c.next_response().unwrap();
    let direct_payload: Value =
        serde_json::from_str(direct["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let direct_events = direct_payload["result"]["events"].as_array().unwrap();

    // Same thing inside tx_run.
    c.send(&json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "tools/call",
        "params": {
            "name": "tx_run",
            "arguments": {
                "operations": [{"tool": "observe_events", "arguments": {}}]
            }
        }
    }));
    let tx = c.next_response().unwrap();
    let tx_payload: Value =
        serde_json::from_str(tx["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    // tx_run wraps each operation's full envelope (the wrapped tool's own
    // {ok, result} response) under results[N].result. So the path to the
    // events array is one level deeper than for a direct call.
    let tx_first = &tx_payload["results"][0];
    assert_eq!(tx_first["ok"], true);
    let tx_events = tx_first["result"]["result"]["events"].as_array().unwrap();

    assert_eq!(
        direct_events.len(),
        tx_events.len(),
        "direct call and tx_run wrapper return different event counts"
    );

    c.shutdown();
}
