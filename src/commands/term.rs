use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "term",
        name: "list",
        summary: "Liste les terminaux ouverts avec leur contexte complet.",
        description: "Wrap rstudioapi::terminalList() + terminalContext() pour chaque id.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio term list",
            explanation: "Retourne un tableau de terminaux, chacun avec id, caption, working_dir, shell, pid, busy, ...",
        }],
        returns: "{terminals: [{id, caption, title, working_dir, shell, running, busy, exit_code, pid, cols, rows, lines, connection}]}",
        errors: &[],
        rstudioapi_fn: Some("terminalList"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "buffer",
        summary: "Lit le buffer (lignes) d'un terminal. Live.",
        description: "rstudioapi::terminalBuffer(id, stripAnsi). \
                      Strip les codes ANSI SGR (couleurs) par défaut, \
                      garde les codes OSC (titres window etc.).",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Identifiant du terminal (8 hex chars, depuis term list).",
            },
            ParamSpec {
                name: "--limit",
                kind: ParamKind::Integer,
                required: false,
                default: None,
                allowed: &[],
                description: "Nombre maximum de lignes (les plus récentes).",
            },
            ParamSpec {
                name: "--ansi",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Préserver les codes ANSI SGR (default strippés).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio term buffer 93555F0A --limit 20",
            explanation: "Les 20 dernières lignes du terminal 93555F0A.",
        }],
        returns: "{id: string, lines: [string]}",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Identifiant inconnu.",
        }],
        rstudioapi_fn: Some("terminalBuffer"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "context",
        summary: "Retourne le contexte complet d'un terminal (metadata).",
        description: "rstudioapi::terminalContext(id), tout le détail (handle, caption, pid, ...).",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Identifiant du terminal.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio term context 93555F0A",
            explanation: "Retourne handle, caption, working_dir, shell, pid, busy, exit_code, ...",
        }],
        returns: "{handle, caption, title, working_dir, shell, running, busy, exit_code, connection, sequence, lines, cols, rows, pid, full_screen, restarted}",
        errors: &[],
        rstudioapi_fn: Some("terminalContext"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "create",
        summary: "Crée un nouveau terminal et retourne son id.",
        description: "rstudioapi::terminalCreate(caption, show, shellType). \
                      show=FALSE par défaut pour ne pas perturber le focus.",
        params: &[
            ParamSpec {
                name: "--name",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Caption visible dans le panneau Terminal.",
            },
            ParamSpec {
                name: "--shell-type",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Type de shell (ex: bash, zsh, default). NULL = default.",
            },
            ParamSpec {
                name: "--show",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Donner le focus au panneau Terminal après création.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio term create --name \"my-task\"",
            explanation: "Crée un terminal nommé my-task, retourne {id: \"...\"}.",
        }],
        returns: "{id: string}",
        errors: &[],
        rstudioapi_fn: Some("terminalCreate"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "send",
        summary: "Envoie du texte au terminal SANS Enter final (poked au prompt courant).",
        description: "Piège : un term exec qui suit s'ajoute à la ligne courante. \
                      Pour exécuter plusieurs commandes distinctes, préférer plusieurs term exec.",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Id du terminal.",
            },
            ParamSpec {
                name: "text",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Texte à insérer.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio term send 93555F0A \"git status\"",
            explanation: "Tape git status dans le terminal mais n'exécute pas.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalSend"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "exec",
        summary: "Envoie du texte au terminal AVEC Enter final (exécute). Fire-and-forget.",
        description: "N'attend pas la fin d'exécution. Lire term buffer <id> après pour le résultat.",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Id du terminal.",
            },
            ParamSpec {
                name: "text",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Code à exécuter (un newline final est ajouté si absent).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio term exec 93555F0A 'ls -la /tmp'",
            explanation: "Tape et exécute ls -la /tmp dans le terminal.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalSend"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "kill",
        summary: "Tue un terminal (le supprime du panneau).",
        description: "rstudioapi::terminalKill(id).",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Id du terminal.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio term kill 0ACC78A5",
            explanation: "Termine et supprime le terminal 0ACC78A5.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalKill"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "clear",
        summary: "Vide le buffer d'un terminal.",
        description: "rstudioapi::terminalClear(id).",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Id du terminal.",
        }],
        examples: &[],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalClear"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "term",
        name: "activate",
        summary: "Donne le focus au panneau Terminal et active ce terminal.",
        description: "rstudioapi::terminalActivate(id). Visible côté user.",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Id du terminal.",
        }],
        examples: &[],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("terminalActivate"),
        rpc_method: Some("execute_r_code"),
    },
];

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
