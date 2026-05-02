use std::fs;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::rpc::RpcClient;
use crate::session::Session;

#[derive(Subcommand, Debug)]
pub enum ConsoleCmd {
    /// Liste les commandes saisies par l'utilisateur dans la console R.
    /// Live (lit l'historique en mémoire de la session R).
    History {
        /// Nombre maximum d'entrées à retourner (les plus récentes).
        #[arg(long, short = 'n', default_value_t = 100)]
        limit: u32,
    },
    /// Lit le snapshot du buffer console écrit par RStudio lors de la dernière
    /// suspend (~/.local/share/rstudio/sessions/active/session-<ID>/suspended-session-data/console_actions).
    /// Pas live — la valeur de `last_modified` indique l'ancienneté du snapshot.
    Actions {
        /// Nombre maximum d'entrées à retourner (les plus récentes).
        #[arg(long, short = 'n')]
        limit: Option<usize>,
        /// Filtre par type d'action (parmi: prompt, input, output, error).
        /// Multi-valué; séparé par virgule. Sans flag, toutes retournées.
        #[arg(long, value_delimiter = ',')]
        types: Vec<ActionType>,
    },
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionType {
    Prompt,
    Input,
    Output,
    Error,
}

impl ActionType {
    fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Prompt),
            1 => Some(Self::Input),
            2 => Some(Self::Output),
            3 => Some(Self::Error),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Input => "input",
            Self::Output => "output",
            Self::Error => "error",
        }
    }
}

pub fn run(
    cmd: &ConsoleCmd,
    rpc: &RpcClient<'_>,
    session: &Session,
) -> Result<Option<Value>, CliError> {
    match cmd {
        ConsoleCmd::History { limit } => history(rpc, *limit),
        ConsoleCmd::Actions { limit, types } => actions(session, *limit, types),
    }
}


fn history(rpc: &RpcClient<'_>, limit: u32) -> Result<Option<Value>, CliError> {
    if limit == 0 {
        return Err(CliError::user("--limit must be > 0"));
    }
    let raw = rpc.rpc("get_recent_history", vec![json!(limit)])?;
    let commands: Vec<String> = raw
        .get("command")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(json!({ "commands": commands })))
}

fn actions(
    session: &Session,
    limit: Option<usize>,
    types: &[ActionType],
) -> Result<Option<Value>, CliError> {
    let dir = session.require_session_dir()?;
    let path = dir.join("suspended-session-data").join("console_actions");
    let metadata = fs::metadata(&path).map_err(|e| {
        CliError::session(format!(
            "console_actions snapshot not found at {} ({e}). \
             A snapshot is only written when the session has been suspended at least once.",
            path.display()
        ))
    })?;
    let last_modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    let content = fs::read_to_string(&path)
        .map_err(|e| CliError::internal(format!("read {}: {e}", path.display())))?;
    let parsed: Value = serde_json::from_str(&content)
        .map_err(|e| CliError::internal(format!("parse {}: {e}", path.display())))?;

    let type_codes = parsed
        .get("type")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CliError::internal(format!("{}: missing 'type' array", path.display())))?;
    let data_arr = parsed
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CliError::internal(format!("{}: missing 'data' array", path.display())))?;

    let allow_all = types.is_empty();
    let mut entries: Vec<Value> = type_codes
        .iter()
        .zip(data_arr.iter())
        .filter_map(|(t, d)| {
            let code = t.as_i64()?;
            let kind = ActionType::from_code(code);
            let kind_str = kind
                .map(ActionType::as_str)
                .unwrap_or("unknown")
                .to_string();
            let text = d.as_str().unwrap_or("").to_string();
            let keep = allow_all
                || kind
                    .map(|k| types.contains(&k))
                    .unwrap_or(false);
            if !keep {
                return None;
            }
            Some(json!({
                "type": kind_str,
                "code": code,
                "text": text,
            }))
        })
        .collect();

    if let Some(n) = limit {
        let drop = entries.len().saturating_sub(n);
        entries.drain(..drop);
    }

    Ok(Some(json!({
        "snapshot_path": path.to_string_lossy(),
        "last_modified_unix": last_modified,
        "is_live": false,
        "entries": entries,
    })))
}
