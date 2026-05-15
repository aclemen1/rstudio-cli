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
fn tools_list_contains_tx_and_registry_actions() {
    let mut c = McpClient::spawn();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let resp = c.next_response().unwrap();
    let tools = resp["result"]["tools"].as_array().unwrap();
    let names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    // The MCP server uses a "progressive discovery" surface: tools/list
    // exposes only a minimal core (meta_*, tools_search, tx_*, r_script).
    // Registry-derived actions (editor_*, env_*, pane_*, term_*, ...) are
    // discovered via tools_search and invoked via tools/call but do NOT
    // appear in tools/list to keep the prompt context small.
    // Tx control tools
    assert!(names.contains(&"tx_begin".to_string()));
    assert!(names.contains(&"tx_end".to_string()));
    assert!(names.contains(&"tx_run".to_string()));
    // Core registry-derived
    assert!(names.contains(&"meta_version".to_string()));
    assert!(names.contains(&"meta_status".to_string()));
    assert!(names.contains(&"r_script".to_string()));
    // tools_search itself
    assert!(names.contains(&"tools_search".to_string()));
    // meta_tx must NOT appear (it documents the CLI's `rstudio tx --`,
    // whose MCP equivalent is the tx_begin/tx_end/tx_run trio above).
    assert!(!names.contains(&"meta_tx".to_string()));
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
