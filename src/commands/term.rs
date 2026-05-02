use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};

#[derive(Subcommand, Debug)]
pub enum TermCmd {
    /// Liste les terminaux ouverts dans le panneau Terminal avec leur contexte.
    List,
    /// Affiche le buffer (lignes) d'un terminal. Live.
    Buffer {
        /// Identifiant du terminal (8 caractères hex, comme retourné par `term list`).
        id: String,
        /// Nombre maximum de lignes (les plus récentes).
        #[arg(long, short = 'n')]
        limit: Option<usize>,
        /// Préserver les codes ANSI (par défaut: strippés).
        #[arg(long)]
        ansi: bool,
    },
    /// Crée un nouveau terminal. Retourne son id.
    Create {
        /// Caption visible dans le panneau Terminal.
        #[arg(long)]
        name: Option<String>,
        /// Type de shell (ex: "bash", "zsh", "default").
        #[arg(long)]
        shell_type: Option<String>,
        /// Donner le focus au panneau Terminal (par défaut: FALSE).
        #[arg(long)]
        show: bool,
    },
    /// Envoie du texte au terminal sans newline final (pas d'Enter).
    /// Le texte est poké au prompt courant. Un `term exec` qui suit s'appendra
    /// à la ligne courante au lieu de démarrer une nouvelle commande —
    /// préférer plusieurs `term exec` pour exécuter des commandes distinctes.
    Send {
        id: String,
        text: String,
    },
    /// Envoie du texte au terminal avec newline final (équivalent d'un Enter).
    /// Comportement fire-and-forget : ne bloque pas, n'attend pas la fin
    /// d'exécution. Lire `term buffer <id>` après pour voir le résultat.
    Exec {
        id: String,
        text: String,
    },
    /// Tue un terminal (le supprime du panneau).
    Kill {
        id: String,
    },
    /// Vide le buffer d'un terminal.
    Clear {
        id: String,
    },
    /// Retourne le contexte complet d'un terminal (caption, working_dir, shell, pid, etc.).
    Context {
        id: String,
    },
    /// Donne le focus au panneau Terminal et active ce terminal.
    Activate {
        id: String,
    },
}

pub fn run(cmd: &TermCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        TermCmd::List => list(rpc),
        TermCmd::Buffer { id, limit, ansi } => buffer(rpc, id, *limit, *ansi),
        TermCmd::Create { name, shell_type, show } => create(rpc, name.as_deref(), shell_type.as_deref(), *show),
        TermCmd::Send { id, text } => send(rpc, id, text),
        TermCmd::Exec { id, text } => exec(rpc, id, text),
        TermCmd::Kill { id } => kill(rpc, id),
        TermCmd::Clear { id } => clear(rpc, id),
        TermCmd::Context { id } => context(rpc, id),
        TermCmd::Activate { id } => activate(rpc, id),
    }
}

fn list(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    let r = r#"local({
  ids <- rstudioapi::terminalList()
  if (length(ids) == 0) {
    cat("[]")
  } else {
    items <- lapply(ids, function(id) {
      ctx <- rstudioapi::terminalContext(id)
      list(
        id = ctx$handle,
        caption = ctx$caption,
        title = ctx$title,
        working_dir = ctx$working_dir,
        shell = ctx$shell,
        running = ctx$running,
        busy = ctx$busy,
        exit_code = ctx$exit_code,
        pid = ctx$pid,
        cols = ctx$cols,
        rows = ctx$rows,
        lines = ctx$lines,
        connection = ctx$connection
      )
    })
    cat(jsonlite::toJSON(items, auto_unbox = TRUE, null = "null"))
  }
})"#;
    let raw = r_eval::run(rpc, r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("term list: invalid JSON from R: {e}; raw: {raw}")))?;
    Ok(Some(json!({ "terminals": parsed })))
}

fn context(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    let r = format!(
        r#"local({{
  ctx <- rstudioapi::terminalContext({id_q})
  cat(jsonlite::toJSON(ctx, auto_unbox = TRUE, null = "null"))
}})"#,
        id_q = r_quote(id)
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("term context: invalid JSON from R: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn buffer(
    rpc: &RpcClient<'_>,
    id: &str,
    limit: Option<usize>,
    ansi: bool,
) -> Result<Option<Value>, CliError> {
    let strip = if ansi { "FALSE" } else { "TRUE" };
    // We want a JSON array of lines so the CLI can return structured output
    // and the `--limit N` knob works on the server side too (saves transport).
    let n_clause = match limit {
        Some(n) => format!("buf <- tail(buf, {n}); "),
        None => String::new(),
    };
    let r = format!(
        r#"local({{
  buf <- rstudioapi::terminalBuffer({id_q}, stripAnsi = {strip})
  {n_clause}cat(jsonlite::toJSON(buf, auto_unbox = FALSE))
}})"#,
        id_q = r_quote(id),
    );
    let raw = r_eval::run(rpc, &r)?;
    let lines: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("term buffer: invalid JSON from R: {e}; raw: {raw}")))?;
    Ok(Some(json!({ "id": id, "lines": lines })))
}

fn create(
    rpc: &RpcClient<'_>,
    name: Option<&str>,
    shell_type: Option<&str>,
    show: bool,
) -> Result<Option<Value>, CliError> {
    let caption_arg = name.map(r_quote).unwrap_or_else(|| "NULL".into());
    let shell_arg = shell_type.map(r_quote).unwrap_or_else(|| "NULL".into());
    let show_r = if show { "TRUE" } else { "FALSE" };
    let r = format!(
        r#"cat(rstudioapi::terminalCreate(caption = {caption_arg}, show = {show_r}, shellType = {shell_arg}))"#
    );
    let id = r_eval::run(rpc, &r)?;
    Ok(Some(json!({ "id": id.trim() })))
}

fn send(rpc: &RpcClient<'_>, id: &str, text: &str) -> Result<Option<Value>, CliError> {
    let r = format!(
        "rstudioapi::terminalSend({}, {})",
        r_quote(id),
        r_quote(text)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn exec(rpc: &RpcClient<'_>, id: &str, text: &str) -> Result<Option<Value>, CliError> {
    let with_newline = if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    };
    let r = format!(
        "rstudioapi::terminalSend({}, {})",
        r_quote(id),
        r_quote(&with_newline)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn kill(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    let r = format!("rstudioapi::terminalKill({})", r_quote(id));
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn clear(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    let r = format!("rstudioapi::terminalClear({})", r_quote(id));
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn activate(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    let r = format!("rstudioapi::terminalActivate({})", r_quote(id));
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}
