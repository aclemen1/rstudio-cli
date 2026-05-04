use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::VERSION;
use crate::commands::{
    console, editor, env, job, observe, pane, policy_cmd, pref, r, raw, schema_cmd, session, skill,
    status, term, ui,
};
use crate::error::CliError;
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

    /// Whole-session info and lifecycle (version, project, restart).
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

    match dispatch(cli) {
        Ok(reply) => {
            print_reply(reply, format);
            ExitCode::from(0)
        }
        Err(err) => {
            // Errors default to JSON: the AI-native contract is uniform,
            // and bad-arg errors fire before any command-specific default
            // is known. Explicit --format text still pretty-prints.
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
        Command::Session(session::SessionCmd::Restart { .. }) => Some("session.restart"),
        Command::Session(session::SessionCmd::OpenProject { .. }) => Some("session.open-project"),
        Command::Session(_) => Some("session"),
        // R: differentiate code execution from poll.
        Command::R(r::RCmd::Exec { .. }) => Some("r.exec"),
        Command::R(r::RCmd::Send { .. }) => Some("r.send"),
        Command::R(_) => Some("r"),
        // Everything else: category-level granularity is sufficient.
        Command::Editor(_) => Some("editor"),
        Command::Console(_) => Some("console"),
        Command::Term(_) => Some("term"),
        Command::Env(_) => Some("env"),
        Command::Pane(_) => Some("pane"),
        Command::Pref(_) => Some("pref"),
        Command::Job(_) => Some("job"),
        Command::Ui(_) => Some("ui"),
        Command::Observe(_) => Some("observe"),
        Command::Rpc(_) => Some("rpc"),
        Command::Postback(_) => Some("postback"),
    };
    if let Some(key) = policy_key {
        policy.check(key)?;
    }

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
        Command::Observe(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            observe::run(&cmd, &rpc, &session)
        }
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
    }
}
