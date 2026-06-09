use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::VERSION;
use std::time::Duration;

use crate::commands::{
    console, debug, editor, env, job, mcp, observe, pane, policy_cmd, pref, project, r, raw,
    schema_cmd, session, skill, status, term, tx, ui,
};
use crate::error::CliError;
use crate::lock::SessionLock;
use crate::output::{Format, Reply, print_err, print_reply};
use crate::policy::Policy;
use crate::rpc::RpcClient;
use crate::session::{Mode, Session, SessionOverrides};

#[derive(Parser, Debug)]
#[command(
    name = "rstudio",
    version = VERSION,
    about = "AI-native CLI bridge to drive an RStudio Server (Linux) or Desktop (macOS) IDE from a terminal",
    long_about = None,
)]
struct Cli {
    /// Force a specific RStudio mode. Default 'auto' picks Server when an
    /// rsession Unix socket is reachable, Desktop when a local rsession
    /// process is running.
    #[arg(long, global = true, default_value = "auto", value_parser = ["auto", "server", "desktop"])]
    mode: String,

    /// (Server) Override socket path (default: $RS_SESSION_TMP_DIR/$RSTUDIO_SESSION_STREAM).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// (Desktop) Override the rsession TCP port. Pair with --secret to skip
    /// process discovery. Default: discover from the running rsession process.
    #[arg(long, global = true)]
    port: Option<u16>,

    /// (Desktop) Override the rsession shared secret (RS_SHARED_SECRET).
    /// Required when --port is passed.
    #[arg(long, global = true)]
    secret: Option<String>,

    /// Override user identity (default: $USER).
    #[arg(long, global = true)]
    user: Option<String>,

    /// Override session id. On Server, used to locate session-persistent-state.
    /// On Desktop, used as the on-disk sources/session-<id>/ folder name
    /// (= the rsession --launcher-token value).
    #[arg(long, global = true)]
    session_id: Option<String>,

    /// (Server) Override the full path to the session-persistent-state file.
    #[arg(long, global = true)]
    state_path: Option<PathBuf>,

    /// Output format. Defaults to `json` for action commands (the AI-native
    /// envelope contract), `text` for meta-CLI commands (`version`, `skill
    /// show`, `skill install`) where plain output is more useful for humans
    /// and Unix pipelines. Pass `--format json|text` to force one.
    #[arg(long, global = true)]
    format: Option<Format>,

    /// Skip the per-call session mutex. The session lock prevents two
    /// agents from interleaving writes on the same RStudio session;
    /// pass --no-lock when you're sure no other writer is active
    /// (debugging, lone scripts). Read commands and meta-CLI never lock.
    #[arg(long, global = true)]
    no_lock: bool,

    /// Timeout in seconds when waiting for the per-session mutex.
    /// On timeout, errors with the holder's PID and command. Default 30s.
    #[arg(long, global = true, default_value_t = 30.0)]
    lock_timeout: f64,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print the CLI version (= embedded skill version — they ship together).
    Version,

    /// Snapshot of the CLI ↔ session wiring (mode, transport, ids, R version, open docs).
    /// Single round-trip; ideal call at the start of an agent session.
    Status,

    /// Editor manipulation (open, navigate, read, context, insert, select, ...).
    #[command(subcommand)]
    Editor(editor::EditorCmd),

    /// Run R code in the active session (silent or visible).
    #[command(subcommand)]
    R(r::RCmd),

    /// R console history and buffer access.
    #[command(subcommand)]
    Console(console::ConsoleCmd),

    /// Interact with R's active debugger (browser, debug, recover):
    /// inspect state, step, list locals, exit. All actions no-op or
    /// error cleanly when no debugger is active.
    #[command(subcommand)]
    Debug(debug::DebugCmd),

    /// RStudio Terminal pane (live shells).
    #[command(subcommand)]
    Term(term::TermCmd),

    /// Inspect the active R environment.
    #[command(subcommand)]
    Env(env::EnvCmd),

    /// Non-editor panes: Viewer (HTML), Files (navigation), Markers (lint feedback).
    #[command(subcommand)]
    Pane(pane::PaneCmd),

    /// Embedded Claude Code skill (show / install).
    #[command(subcommand)]
    Skill(skill::SkillCmd),

    /// Project lifecycle: create / init existing dir / clone from git / open / current.
    #[command(subcommand)]
    Project(project::ProjectCmd),

    /// Whole-session info and lifecycle (info, restart, list).
    #[command(subcommand)]
    Session(session::SessionCmd),

    /// User and built-in RStudio preferences + persistent key/value store.
    #[command(subcommand)]
    Pref(pref::PrefCmd),

    /// Background jobs in the Jobs pane.
    #[command(subcommand)]
    Job(job::JobCmd),

    /// Modal UI prompts (BLOCKING).
    #[command(subcommand)]
    Ui(ui::UiCmd),

    /// Stream session-state changes as JSON Lines on stdout (live tail).
    Observe(observe::ObserveCmd),

    /// Hold the session lock across a child process for atomic
    /// multi-call sequences (style `flock(1)`). Examples:
    ///   rstudio tx -- bash -c 'editor read X | jq ... | editor write X'
    ///   rstudio tx -- bash    # interactive REPL inside the transaction
    ///   rstudio tx            # same, defaults to $SHELL
    Tx(tx::TxCmd),

    /// Run as an MCP (Model Context Protocol) server over stdio.
    /// Exposes the entire CLI surface as MCP tools so Claude Code,
    /// Cline, Cursor, Continue, etc. can invoke them natively. Includes
    /// `tx_begin` / `tx_end` / `tx_run` tools that map the per-session
    /// writer lock onto the LLM's tool-call sequence. Configure the
    /// client with `claude mcp add rstudio --scope user -- rstudio mcp`.
    Mcp(mcp::McpCmd),

    /// Self-describing command catalog (3-level drill-down).
    Schema(schema_cmd::SchemaCmd),

    /// Raw JSON-RPC call (escape hatch for methods not yet wrapped).
    Rpc(raw::RpcCmd),

    /// Raw postback (escape hatch for postback endpoints not yet wrapped).
    Postback(raw::PostbackCmd),

    /// Security policy: block / unblock commands (no session required).
    #[command(subcommand)]
    Policy(policy_cmd::PolicyCmd),
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let format = cli.format;

    // Read update notice from cache before dispatch (zero latency — cache
    // read only; background thread refreshes when TTL has expired).
    let update = crate::update_check::check(VERSION);

    match dispatch(cli) {
        Ok(reply) => {
            print_reply(reply, format);
            if let Some(info) = update {
                eprintln!(
                    "rstudio-cli {} is available (installed: {}).",
                    info.latest, VERSION
                );
            }
            ExitCode::from(0)
        }
        Err(err) => {
            print_err(&err, format.unwrap_or(Format::Json));
            ExitCode::from(1)
        }
    }
}

fn dispatch(cli: Cli) -> Result<Reply, CliError> {
    let mode = match cli.mode.as_str() {
        "auto" => None,
        "server" => Some(Mode::Server),
        "desktop" => Some(Mode::Desktop),
        // clap value_parser already restricts the set, so this is unreachable
        // in practice — kept for explicit error reporting.
        other => {
            return Err(CliError::user(format!(
                "invalid --mode '{other}'. Expected: auto, server, desktop."
            )));
        }
    };
    let overrides = SessionOverrides {
        mode,
        socket: cli.socket,
        user: cli.user,
        session_id: cli.session_id,
        state_path: cli.state_path,
        port: cli.port,
        secret: cli.secret,
    };
    // Policy commands and meta-CLI commands are exempt from policy checks.
    // Everything else is checked before session detection.
    let policy = Policy::load();
    let policy_key: Option<&str> = match &cli.command {
        Command::Version
        | Command::Status
        | Command::Schema(_)
        | Command::Skill(_)
        | Command::Policy(_) => None,
        // Session: differentiate the two destructive actions for fine-grained blocking.
        // Project: differentiate the destructive / disk-mutating actions.
        Command::Project(project::ProjectCmd::Open { .. }) => Some("project.open"),
        Command::Project(project::ProjectCmd::New { .. }) => Some("project.new"),
        Command::Project(project::ProjectCmd::Init { .. }) => Some("project.init"),
        Command::Project(project::ProjectCmd::Clone { .. }) => Some("project.clone"),
        Command::Project(_) => Some("project"),
        Command::Session(session::SessionCmd::Restart { .. }) => Some("session.restart"),
        Command::Session(_) => Some("session"),
        // R: differentiate code execution from poll.
        Command::R(r::RCmd::Exec { .. }) => Some("r.exec"),
        Command::R(r::RCmd::Send { .. }) => Some("r.send"),
        Command::R(_) => Some("r"),
        // Everything else: category-level granularity is sufficient.
        Command::Editor(_) => Some("editor"),
        Command::Console(_) => Some("console"),
        // Debug: status / where / locals / src are reads; step / exit
        // are writes (they push input to the console queue). Use
        // category-level granularity for policy filtering.
        Command::Debug(_) => Some("debug"),
        Command::Term(_) => Some("term"),
        Command::Env(_) => Some("env"),
        Command::Pane(_) => Some("pane"),
        Command::Pref(_) => Some("pref"),
        Command::Job(_) => Some("job"),
        Command::Ui(_) => Some("ui"),
        Command::Observe(_) => Some("observe"),
        Command::Tx(_) => Some("tx"),
        Command::Mcp(_) => Some("mcp"),
        Command::Rpc(_) => Some("rpc"),
        Command::Postback(_) => Some("postback"),
    };
    if let Some(key) = policy_key {
        policy.check(key)?;
    }

    // Tx: holds its own lock and execs a child. Never returns through
    // the standard reply path — std::process::exit propagates the
    // child's status code.
    // MCP: long-running stdio server. No top-level lock; the server
    // itself manages locks via tx_begin / tx_end and per-tool-call
    // subprocess spawns (which acquire their own per-call lock).
    if matches!(&cli.command, Command::Mcp(_)) {
        let Command::Mcp(mcp_cmd) = cli.command else {
            unreachable!()
        };
        let code = mcp::run(&mcp_cmd, overrides)?;
        std::process::exit(code);
    }

    if matches!(&cli.command, Command::Tx(_)) {
        let Command::Tx(tx_cmd) = cli.command else {
            unreachable!()
        };
        let session = Session::detect(overrides)?;
        let timeout = Duration::from_secs_f64(cli.lock_timeout);
        let acquire = !cli.no_lock && !SessionLock::inside_tx();
        let code = tx::run(&tx_cmd, &session, timeout, acquire)?;
        std::process::exit(code);
    }

    // Phase 1 mutex: acquire a per-session exclusive lock for write
    // commands. Skipped when --no-lock is set, when we're already
    // inside an outer `tx -- ...` (RSTUDIO_TX_HELD), or when the
    // command is read-only / meta-CLI. Held for the lifetime of this
    // process — kernel cleanup on exit.
    let _session_lock =
        if needs_write_lock(&cli.command) && !cli.no_lock && !SessionLock::inside_tx() {
            let session = Session::detect(overrides.clone())?;
            let id = session.session_id().ok_or_else(|| {
                CliError::session(
                    "lock: cannot derive session id; pass --session-id, or --no-lock to bypass.",
                )
            })?;
            let label = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
            let timeout = Duration::from_secs_f64(cli.lock_timeout);
            Some(SessionLock::acquire(&id, timeout, &label)?)
        } else {
            None
        };

    match cli.command {
        // Meta-CLI carve-out: text mode prints "0.5.0\n" raw, no envelope.
        Command::Version => Ok(Reply::Adaptive {
            value: json!({ "version": VERSION }),
            text: format!("{VERSION}\n"),
            default_text: true,
        }),
        Command::Status => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            status::run(&rpc, &session)
        }
        Command::Editor(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            editor::run(&cmd, &rpc, &session).map(Reply::Wrapped)
        }
        Command::R(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            r::run(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Console(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            console::run(&cmd, &rpc, &session).map(Reply::Wrapped)
        }
        Command::Debug(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            debug::run(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Term(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            term::run(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Env(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            env::run(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Pane(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            pane::run(&cmd, &rpc).map(Reply::Wrapped)
        }
        // Skill returns Reply directly: 'show' is text-raw markdown,
        // 'install' is human-friendly with ✓/✗ marks.
        Command::Skill(cmd) => skill::run(&cmd),
        Command::Project(cmd) => project::run(&cmd, overrides).map(Reply::Wrapped),
        // `session list` does not need a live session — dispatch before detect.
        Command::Session(session::SessionCmd::List) => session::list_sessions().map(Reply::Wrapped),
        Command::Session(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            session::run(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Pref(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            pref::run(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Job(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            job::run(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Ui(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            ui::run(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Observe(cmd) => observe::run(&cmd, overrides),
        Command::Schema(cmd) => schema_cmd::run(&cmd).map(Reply::Wrapped),
        Command::Rpc(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            raw::run_rpc(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Postback(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            raw::run_postback(&cmd, &rpc).map(Reply::Wrapped)
        }
        Command::Policy(cmd) => policy_cmd::run(&cmd).map(Reply::Wrapped),
        // Tx is handled by an earlier early-return branch. This arm
        // satisfies the exhaustiveness check.
        Command::Tx(_) => unreachable!("Tx handled before this match"),
        Command::Mcp(_) => unreachable!("Mcp handled before this match"),
    }
}

/// Whether this command should acquire the per-session write lock
/// before running. The principle: any command that mutates rsession
/// state (R execution, document editing, project switch, modal UI,
/// etc.) takes the lock. Pure reads (and meta-CLI like version /
/// schema / policy) do not — rsession serializes its own RPCs, so
/// reader/writer races can't happen at the protocol level.
fn needs_write_lock(cmd: &Command) -> bool {
    match cmd {
        // Meta-CLI: no session, no lock.
        Command::Version
        | Command::Status
        | Command::Schema(_)
        | Command::Skill(_)
        | Command::Policy(_) => false,
        // Tx and Mcp: each handles its own locking.
        Command::Tx(_) | Command::Mcp(_) => false,

        // Editor: read-only subcommands.
        Command::Editor(editor::EditorCmd::Read { .. })
        | Command::Editor(editor::EditorCmd::ReadBuffer { .. })
        | Command::Editor(editor::EditorCmd::Context { .. })
        | Command::Editor(editor::EditorCmd::ActiveId { .. })
        | Command::Editor(editor::EditorCmd::Path { .. })
        | Command::Editor(editor::EditorCmd::List) => false,
        Command::Editor(_) => true,

        // R: poll is read-only; kill mutates the async-job registry; exec / send
        // are writes. Interrupt is a write conceptually but MUST skip the lock —
        // its raison d'être is to unblock whoever currently holds it (typically
        // an `r send` waiting for a capture). Otherwise: classic deadlock.
        Command::R(r::RCmd::Poll { .. }) => false,
        Command::R(r::RCmd::Interrupt) => false,
        Command::R(_) => true,

        // Console: history / actions / context are reads, activate is a write.
        Command::Console(console::ConsoleCmd::History { .. })
        | Command::Console(console::ConsoleCmd::Actions { .. })
        | Command::Console(console::ConsoleCmd::Context) => false,
        Command::Console(_) => true,

        // Debug: introspection (status/where/locals/src) is read-only;
        // navigation (step/exit) pushes to console_input and counts as a write.
        Command::Debug(debug::DebugCmd::Status)
        | Command::Debug(debug::DebugCmd::Where)
        | Command::Debug(debug::DebugCmd::Locals)
        | Command::Debug(debug::DebugCmd::Src) => false,
        Command::Debug(_) => true,

        // Term: list / buffer / context / busy / running / exit-code / visible are reads.
        Command::Term(term::TermCmd::List)
        | Command::Term(term::TermCmd::Buffer { .. })
        | Command::Term(term::TermCmd::Context { .. })
        | Command::Term(term::TermCmd::Busy { .. })
        | Command::Term(term::TermCmd::Running { .. })
        | Command::Term(term::TermCmd::ExitCode { .. })
        | Command::Term(term::TermCmd::Visible) => false,
        Command::Term(_) => true,

        // Env: all reads.
        Command::Env(_) => false,

        // Pane: all writes (open viewer / save plot / preview / markers / highlight-ui all mutate).
        Command::Pane(_) => true,

        // Project: `current` is a read; everything else mutates IDE state.
        Command::Project(project::ProjectCmd::Current) => false,
        Command::Project(_) => true,

        // Session: info / list are reads; restart is a write.
        Command::Session(session::SessionCmd::Info)
        | Command::Session(session::SessionCmd::List) => false,
        Command::Session(_) => true,

        // Pref: read* and get-persistent are reads; write* and set-persistent are writes.
        Command::Pref(pref::PrefCmd::Read { .. })
        | Command::Pref(pref::PrefCmd::ReadRstudio { .. })
        | Command::Pref(pref::PrefCmd::GetPersistent { .. }) => false,
        Command::Pref(_) => true,

        // Job: list / is-active are reads, everything else mutates.
        Command::Job(job::JobCmd::List) | Command::Job(job::JobCmd::IsActive) => false,
        Command::Job(_) => true,

        // UI: every modal mutates IDE state and blocks until dismissed.
        Command::Ui(_) => true,

        // Observe: pure read-only — file watching + R-free RPCs (or one
        // execute_r_code per tick that doesn't mutate). No lock.
        Command::Observe(_) => false,

        // Escape hatches: conservatively treat as writes since the
        // method / postback name is arbitrary.
        Command::Rpc(_) | Command::Postback(_) => true,
    }
}
