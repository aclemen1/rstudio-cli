use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use crate::commands::{console, editor, env, pane, r, raw, schema_cmd, skill, term};
use crate::error::CliError;
use crate::output::{Format, print_err, print_ok};
use crate::rpc::RpcClient;
use crate::session::{Session, SessionOverrides};
use crate::VERSION;

#[derive(Parser, Debug)]
#[command(
    name = "rstudio",
    version = VERSION,
    about = "AI-native CLI bridge to interact with the embedded RStudio Server IDE",
    long_about = None,
)]
struct Cli {
    /// Override socket path (default: $RS_SESSION_TMP_DIR/$RSTUDIO_SESSION_STREAM).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// Override user identity (default: $USER).
    #[arg(long, global = true)]
    user: Option<String>,

    /// Override session id used to locate session-persistent-state
    /// (default: $RSTUDIO_SESSION_ID, else most recent under ~/.local/share/rstudio/sessions/active/).
    #[arg(long, global = true)]
    session_id: Option<String>,

    /// Override the full path to the session-persistent-state file.
    #[arg(long, global = true)]
    state_path: Option<PathBuf>,

    /// Output format.
    #[arg(long, global = true, default_value = "json")]
    format: Format,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print the CLI version (= embedded skill version — they ship together).
    Version,

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

    /// Self-describing command catalog (3-level drill-down).
    Schema(schema_cmd::SchemaCmd),

    /// Raw JSON-RPC call (escape hatch for methods not yet wrapped).
    Rpc(raw::RpcCmd),

    /// Raw postback (escape hatch for postback endpoints not yet wrapped).
    Postback(raw::PostbackCmd),
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let format = cli.format;

    match dispatch(cli) {
        Ok(value) => {
            print_ok(value, format);
            ExitCode::from(0)
        }
        Err(err) => {
            print_err(&err, format);
            ExitCode::from(1)
        }
    }
}

fn dispatch(cli: Cli) -> Result<Option<Value>, CliError> {
    let overrides = SessionOverrides {
        socket: cli.socket,
        user: cli.user,
        session_id: cli.session_id,
        state_path: cli.state_path,
    };
    match cli.command {
        Command::Version => Ok(Some(json!({ "version": VERSION }))),
        Command::Editor(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            editor::run(&cmd, &rpc, &session)
        }
        Command::R(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            r::run(&cmd, &rpc)
        }
        Command::Console(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            console::run(&cmd, &rpc, &session)
        }
        Command::Term(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            term::run(&cmd, &rpc)
        }
        Command::Env(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            env::run(&cmd, &rpc)
        }
        Command::Pane(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            pane::run(&cmd, &rpc)
        }
        Command::Skill(cmd) => skill::run(&cmd),
        Command::Schema(cmd) => schema_cmd::run(&cmd),
        Command::Rpc(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            raw::run_rpc(&cmd, &rpc)
        }
        Command::Postback(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            raw::run_postback(&cmd, &rpc)
        }
    }
}
