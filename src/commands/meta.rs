//! Schema documentation for the meta-CLI commands (`version`,
//! `status`, `tx`). These don't dispatch through the ActionSpec
//! infrastructure — their code lives directly in `cli.rs`,
//! `commands/status.rs`, and `commands/tx.rs` respectively. We
//! register their docs here so that `rstudio schema meta <action>`
//! drill-down works the same as for any RPC-bound action, giving
//! agents a uniform discovery surface.
//!
//! In particular, `meta tx` is the home of the multi-agent
//! transactional contract — the rule that an agent can't deduce
//! from a single `--help`: when to use tx, what `RSTUDIO_TX_HELD`
//! means, what NOT to put inside it.

use crate::schema::{ActionSpec, ExampleSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "meta",
        name: "version",
        summary: "Print the CLI version (= embedded skill version — they ship together).",
        description: "Returns the string `X.Y.Z` (text format, default for this command) or \
             `{version: X.Y.Z}` (JSON envelope when --format json). Used by agents to \
             check that a feature they need is available, and by the embedded skill's \
             self-update check.",
        params: &[],
        examples: &[
            ExampleSpec {
                cmd: "rstudio version",
                explanation: "Prints `0.8.0` (or whatever the binary version is).",
            },
            ExampleSpec {
                cmd: "rstudio version --format json",
                explanation: "Returns `{ok: true, result: {version: \"0.8.0\"}}`.",
            },
        ],
        returns: "string (text mode) or {version: string} (json mode)",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: None,
    },
    ActionSpec {
        category: "meta",
        name: "status",
        summary: "Snapshot of the CLI ↔ session wiring (mode, transport, ids, R version, open docs, lock).",
        description: "Single round-trip for the agent at the start of a session — verifies that a \
             session is reachable, returns transport details (Server vs Desktop, socket vs \
             TCP), session id and client id, R / RStudio version, count and identity of \
             open documents, active project, and the per-session lock state \
             (`session.lock.state` = `free` | `held`, `holder` = {pid, command, started_ms} \
             when held, `inside_tx` = whether we're called from inside a `rstudio tx --`). \
             \
             The lock state is informational. An agent should NOT gate behaviour on it — \
             the holder can release between the read and the next call. For atomicity, \
             use `rstudio tx --`. The field exists for debugging timeouts, auditing, and \
             situational awareness.",
        params: &[],
        examples: &[
            ExampleSpec {
                cmd: "rstudio status",
                explanation: "Full JSON envelope (default for status).",
            },
            ExampleSpec {
                cmd: "rstudio status --format text",
                explanation: "Polished one-screen rendering for humans.",
            },
            ExampleSpec {
                cmd: "rstudio status | jq '.result.session.lock'",
                explanation: "Inspect just the lock state.",
            },
        ],
        returns: "{cli, transport, user, session: {id, client_id, sources_dir, state_path, active_project, lock}, rsession, documents}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: None,
    },
    ActionSpec {
        category: "meta",
        name: "tx",
        summary: "Hold the per-session writer lock across a child process (multi-call atomicity).",
        description: "Acquire the per-session writer lock, set `RSTUDIO_TX_HELD=1` in the child \
             environment, and exec the child. Every nested `rstudio` invocation inside \
             the child detects the env var and skips its own per-call lock — the parent \
             already holds it. Patterned after `flock(1)` from util-linux. Kernel \
             cleanup on parent exit handles every failure mode (clean exit, SIGKILL, \
             crash); no daemon, no PID files, no stale locks. \
             \
             === When to use === \
             Single writes are protected automatically by the per-call mutex (Phase 1); \
             you don't need tx for them. Use tx for SEQUENCES of `rstudio` invocations \
             that must be atomic with respect to other agents: \
             \
             - Read-modify-write (`editor read-buffer X` → transform → `editor set-contents X`): \
               another agent could write to X between your read and your write — wrap the \
               whole sequence. \
             - Multi-step edit (`editor select` then `editor insert`): another agent could \
               move the selection between calls. \
             - State-dependent R execution (`r exec setup` then `r exec execute`): another \
               agent could clobber globals between calls. \
             \
             === Defensive default === \
             You CANNOT reliably know whether another agent is connected. The check would \
             race against any new connection. Default to defensive: ALWAYS wrap multi-call \
             sequences in tx, regardless of perceived solitude. Cost when alone: ~10ms \
             (one fork). Cost when not alone without tx: silent data loss. \
             \
             === What NOT to put inside a tx === \
             - `rstudio observe stream` — it never returns; would hold the lock forever. \
               (Read-only; doesn't need a tx anyway.) \
             - `rstudio ui dialog` and other `ui` modals — they block the rsession until \
               the user dismisses them, freezing every other agent's RPC during that time. \
             - Any command in a child process that you can't ensure terminates promptly. \
             \
             === Serialisation, not full ACID === \
             tx provides serialisation (no other agent interleaves), not transactionality. \
             If your 3rd command fails, the first two are already applied. Rollback is \
             your responsibility (snapshot before, restore on error). \
             \
             === Bypass === \
             Global `--no-lock` skips lock acquisition (useful for debugging or solo \
             scripts). Inside a tx, `--no-lock` makes tx a pure env-wrapper: it still \
             sets `RSTUDIO_TX_HELD` for child consistency but doesn't take the flock.",
        params: &[],
        examples: &[
            ExampleSpec {
                cmd: "rstudio tx -- bash -c 'buf=$(rstudio editor read-buffer X | jq -r .result.contents); rstudio editor set-contents X \"$(echo \"$buf\" | sed s/foo/bar/g)\"'",
                explanation: "Atomic read-modify-write — no other agent can interleave between read and write.",
            },
            ExampleSpec {
                cmd: "rstudio tx -- bash",
                explanation: "Interactive REPL with the lock held; `exit` releases.",
            },
            ExampleSpec {
                cmd: "rstudio tx",
                explanation: "Same as above; defaults to $SHELL.",
            },
            ExampleSpec {
                cmd: "rstudio tx -- python3 my_agent.py",
                explanation: "Run any agent script under the lock; nested rstudio calls inside skip locking automatically.",
            },
            ExampleSpec {
                cmd: "rstudio --lock-timeout 5 tx -- echo done",
                explanation: "Override default 30s timeout (errors with holder PID + command if exceeded).",
            },
        ],
        returns: "Child process exit code, propagated. No JSON envelope (passes child stdout/stderr through).",
        errors: &[
            crate::schema::ErrorSpec {
                kind: "user_error",
                when: "Lock timeout — another agent is holding the lock; error includes the holder's PID, command, and start timestamp.",
            },
            crate::schema::ErrorSpec {
                kind: "session_unavailable",
                when: "No RStudio session reachable (Desktop not running, no Server socket).",
            },
        ],
        rstudioapi_fn: None,
        rpc_method: None,
    },
];
