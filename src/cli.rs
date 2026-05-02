use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use crate::commands::{console, editor, env, exec, raw, schema_cmd, term};
use crate::error::CliError;
use crate::output::{Format, print_err, print_ok};
use crate::rpc::RpcClient;
use crate::session::{Session, SessionOverrides};
use crate::{CLI_VERSION, SKILL_VERSION};

#[derive(Parser, Debug)]
#[command(
    name = "rstudio",
    version = CLI_VERSION,
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
    /// Imprime les versions du CLI et du skill embarqué.
    Version,

    /// Manipulation de l'éditeur (ouverture, navigation, ...).
    #[command(subcommand)]
    Editor(editor::EditorCmd),

    /// Exécution de code R dans la session active.
    #[command(subcommand)]
    Exec(exec::ExecCmd),

    /// Lecture de l'historique et du buffer console.
    #[command(subcommand)]
    Console(console::ConsoleCmd),

    /// Manipulation du panneau Terminal RStudio (shells live).
    #[command(subcommand)]
    Term(term::TermCmd),

    /// Inspection de l'environnement R actif.
    #[command(subcommand)]
    Env(env::EnvCmd),

    /// Catalogue auto-descriptif des commandes (drill-down 3 niveaux).
    Schema(schema_cmd::SchemaCmd),

    /// Appel JSON-RPC brut (échappatoire pour méthodes non encore wrappées).
    Rpc(raw::RpcCmd),

    /// Postback brut (échappatoire pour endpoints postback non wrappés).
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
        Command::Version => Ok(Some(json!({
            "cli": CLI_VERSION,
            "skill": SKILL_VERSION,
        }))),
        Command::Editor(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            editor::run(&cmd, &rpc)
        }
        Command::Exec(cmd) => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            exec::run(&cmd, &rpc)
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
