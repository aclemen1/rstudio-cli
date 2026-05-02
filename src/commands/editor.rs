use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "editor",
        name: "open",
        summary: "Ouvre un fichier dans le pane Source (non-modal). Retourne l'id du document.",
        description: "Wrap rstudioapi::documentOpen(path, line, col, moveCursor). \
                      Le fichier apparaît comme un onglet dans l'éditeur principal, \
                      l'utilisateur garde le contrôle. Pas le même comportement que \
                      `editor edit` qui ouvre la modale R `edit()` (Save/Cancel).",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Chemin du fichier (résolu en absolu via canonicalize).",
            },
            ParamSpec {
                name: "--line",
                kind: ParamKind::Integer,
                required: false,
                default: None,
                allowed: &[],
                description: "Ligne (1-based) où placer le curseur après ouverture.",
            },
            ParamSpec {
                name: "--col",
                kind: ParamKind::Integer,
                required: false,
                default: None,
                allowed: &[],
                description: "Colonne (1-based) ; combiner avec --line.",
            },
            ParamSpec {
                name: "--no-cursor",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Ne pas déplacer le curseur (moveCursor=FALSE). Utile pour ouvrir en arrière-plan.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor open ~/code/aclemen1/rstudio-cli/Cargo.toml",
                explanation: "Ouvre Cargo.toml dans le pane Source, position cursor inchangée si déjà ouvert.",
            },
            ExampleSpec {
                cmd: "rstudio editor open src/main.rs --line 42 --col 5",
                explanation: "Ouvre puis place le curseur en (42, 5).",
            },
        ],
        returns: "{path: string, line: int|null, col: int|null, id: string}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "Fichier introuvable (canonicalize fail).",
            },
            ErrorSpec {
                kind: "r_error",
                when: "rstudioapi::documentOpen rejette le chemin.",
            },
        ],
    },
    ActionSpec {
        category: "editor",
        name: "edit",
        summary: "Ouvre la modale R edit() pour le fichier (Save/Cancel). Bloquant.",
        description: "Wrap le postback editfile. Comportement R standard `edit(file = ...)`: \
                      RStudio affiche une fenêtre modale d'édition séparée du pane Source. \
                      L'utilisateur doit cliquer Save ou Cancel pour fermer. Pendant ce temps, \
                      la session R est en attente — les `exec run` qui suivent attendront. \
                      Pour l'édition normale (non-modal), préférer `editor open`.",
        params: &[ParamSpec {
            name: "path",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Chemin du fichier (résolu en absolu via canonicalize).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio editor edit /tmp/scratch.R",
            explanation: "Ouvre une modale d'édition pour /tmp/scratch.R. Bloque jusqu'à Save/Cancel.",
        }],
        returns: "{path: string, exit_code: int}",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Fichier introuvable.",
        }],
    },
    ActionSpec {
        category: "editor",
        name: "read",
        summary: "Lit le contenu d'un fichier (pas le buffer éditeur, le fichier disque).",
        description: "Wrap le RPC get_file_contents [path, encoding=UTF-8]. \
                      Pour le buffer en cours d'édition (avec modifs non sauvegardées), \
                      utiliser `editor context --include-contents`.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Chemin du fichier (canonicalize côté CLI).",
            },
            ParamSpec {
                name: "--encoding",
                kind: ParamKind::String,
                required: false,
                default: Some("UTF-8"),
                allowed: &[],
                description: "Encoding passé à get_file_contents.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio editor read ~/projects/foo/main.R",
            explanation: "Retourne le contenu disque de main.R.",
        }],
        returns: "{path: string, contents: string}",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Fichier introuvable.",
        }],
    },
    ActionSpec {
        category: "editor",
        name: "context",
        summary: "Contexte du document actif dans le panneau Source (path, sélection, etc.).",
        description: "Wrap rstudioapi::getSourceEditorContext(). Sans flag, retourne id, \
                      path, et la liste de sélections (positions start/end + texte sélectionné). \
                      Avec --include-contents, ajoute les lignes du buffer (live, modifs incluses).",
        params: &[ParamSpec {
            name: "--include-contents",
            kind: ParamKind::Bool,
            required: false,
            default: Some("false"),
            allowed: &[],
            description: "Inclure le contenu du buffer (live, peut être grand).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio editor context",
            explanation: "Retourne {id, path, selections: [{start_row, start_col, end_row, end_col, text}]}.",
        }],
        returns: "{id, path, selections, contents?}",
        errors: &[],
    },
    ActionSpec {
        category: "editor",
        name: "insert",
        summary: "Insère du texte dans le document actif.",
        description: "Wrap rstudioapi::insertText(). Sans --at, à la position du curseur. \
                      --at start = (1,1), --at end = fin du fichier, --at L:C = position explicite.",
        params: &[
            ParamSpec {
                name: "text",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Texte à insérer.",
            },
            ParamSpec {
                name: "--at",
                kind: ParamKind::String,
                required: false,
                default: Some("cursor"),
                allowed: &["cursor", "start", "end"],
                description: "Position d'insertion ; valeurs spéciales 'cursor', 'start', 'end' ou format 'L:C'.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor insert 'TODO\\n' --at start",
                explanation: "Préfixe le fichier avec 'TODO\\n'.",
            },
            ExampleSpec {
                cmd: "rstudio editor insert 'x' --at 5:1",
                explanation: "Insère 'x' à la ligne 5, colonne 1.",
            },
        ],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Position invalide ou pas d'éditeur actif.",
        }],
    },
    ActionSpec {
        category: "editor",
        name: "select",
        summary: "Définit la sélection (ou positionne le curseur) dans le document actif.",
        description: "Wrap rstudioapi::setSelectionRanges(). Format range: 'L:C' (curseur sans \
                      sélection) ou 'L1:C1-L2:C2' (sélection range).",
        params: &[ParamSpec {
            name: "range",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Range à sélectionner. 'L:C' ou 'L1:C1-L2:C2'. 1-based.",
        }],
        examples: &[
            ExampleSpec {
                cmd: "rstudio editor select 10:1",
                explanation: "Place le curseur en ligne 10, colonne 1.",
            },
            ExampleSpec {
                cmd: "rstudio editor select 5:1-7:80",
                explanation: "Sélectionne du (5,1) au (7,80).",
            },
        ],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Format de range invalide.",
        }],
    },
];

#[derive(Subcommand, Debug)]
pub enum EditorCmd {
    /// Ouvre un fichier dans le pane Source (non-modal). Retourne l'id du document.
    Open {
        path: PathBuf,
        #[arg(long)]
        line: Option<u32>,
        #[arg(long)]
        col: Option<u32>,
        /// Ne pas déplacer le curseur (moveCursor=FALSE).
        #[arg(long)]
        no_cursor: bool,
    },
    /// Ouvre la modale R `edit()` pour le fichier (Save/Cancel).
    /// Bloque la session R jusqu'à fermeture de la modale.
    Edit { path: PathBuf },
    /// Lit le contenu disque d'un fichier (pas le buffer en cours d'édition).
    Read {
        path: PathBuf,
        /// Encoding (default UTF-8).
        #[arg(long, default_value = "UTF-8")]
        encoding: String,
    },
    /// Contexte du document actif dans le panneau Source.
    Context {
        /// Inclure le contenu live du buffer (peut être grand).
        #[arg(long)]
        include_contents: bool,
    },
    /// Insère du texte dans le document actif.
    Insert {
        text: String,
        /// Position d'insertion : 'cursor' (def), 'start', 'end', ou 'L:C'.
        #[arg(long, default_value = "cursor")]
        at: String,
    },
    /// Définit la sélection (ou le curseur) dans le document actif.
    Select {
        /// Range : 'L:C' ou 'L1:C1-L2:C2'.
        range: String,
    },
}

pub fn run(cmd: &EditorCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        EditorCmd::Open {
            path,
            line,
            col,
            no_cursor,
        } => open(rpc, path, *line, *col, *no_cursor),
        EditorCmd::Edit { path } => edit_modal(rpc, path),
        EditorCmd::Read { path, encoding } => read(rpc, path, encoding),
        EditorCmd::Context { include_contents } => context(rpc, *include_contents),
        EditorCmd::Insert { text, at } => insert(rpc, text, at),
        EditorCmd::Select { range } => select(rpc, range),
    }
}

fn open(
    rpc: &RpcClient<'_>,
    path: &PathBuf,
    line: Option<u32>,
    col: Option<u32>,
    no_cursor: bool,
) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();

    let line_arg = line.map(|l| format!("{l}L")).unwrap_or_else(|| "-1L".into());
    let col_arg = col.map(|c| format!("{c}L")).unwrap_or_else(|| "-1L".into());
    let move_cursor = if no_cursor { "FALSE" } else { "TRUE" };

    let r_code = format!(
        "cat(rstudioapi::documentOpen({path}, line = {line_arg}, col = {col_arg}, moveCursor = {move_cursor}))",
        path = r_quote(&abs_str),
    );
    let id = r_eval::run(rpc, &r_code)?;

    Ok(Some(json!({
        "path": abs_str,
        "line": line,
        "col": col,
        "id": id.trim(),
    })))
}

fn edit_modal(rpc: &RpcClient<'_>, path: &PathBuf) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();
    let pb = rpc.postback("editfile", &abs_str)?;
    Ok(Some(json!({
        "path": abs_str,
        "exit_code": pb.exit_code,
    })))
}

fn read(rpc: &RpcClient<'_>, path: &PathBuf, encoding: &str) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();
    let raw = rpc.rpc(
        "get_file_contents",
        vec![
            Value::String(abs_str.clone()),
            Value::String(encoding.to_string()),
        ],
    )?;
    let contents = raw.as_str().unwrap_or("").to_string();
    Ok(Some(json!({
        "path": abs_str,
        "contents": contents,
    })))
}

fn context(rpc: &RpcClient<'_>, include_contents: bool) -> Result<Option<Value>, CliError> {
    let contents_field = if include_contents {
        "contents = paste(ctx$contents, collapse = \"\\n\"),"
    } else {
        ""
    };
    let r_code = format!(
        r#"local({{
  ctx <- rstudioapi::getSourceEditorContext()
  if (is.null(ctx)) {{
    cat("null")
    return(invisible())
  }}
  selections <- lapply(ctx$selection, function(s) {{
    list(
      start_row = as.integer(s$range$start[[1]]),
      start_col = as.integer(s$range$start[[2]]),
      end_row = as.integer(s$range$end[[1]]),
      end_col = as.integer(s$range$end[[2]]),
      text = s$text
    )
  }})
  out <- list(
    id = ctx$id,
    path = ctx$path,
    {contents_field}
    selections = selections
  )
  cat(jsonlite::toJSON(out, auto_unbox = TRUE, null = "null"))
}})"#
    );
    let raw = r_eval::run(rpc, &r_code)?;
    if raw.trim() == "null" {
        return Ok(Some(Value::Null));
    }
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("editor context: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn insert(rpc: &RpcClient<'_>, text: &str, at: &str) -> Result<Option<Value>, CliError> {
    let location = match at {
        "cursor" => "NULL".to_string(),
        "start" => "rstudioapi::document_position(1L, 1L)".to_string(),
        "end" => {
            // End = (lines, last_col + 1). Compute on the R side.
            "{ ctx <- rstudioapi::getSourceEditorContext(); \
               n <- length(ctx$contents); \
               last <- if (n > 0) nchar(ctx$contents[n]) + 1L else 1L; \
               rstudioapi::document_position(if (n > 0) n else 1L, last) }"
                .to_string()
        }
        custom => {
            let (line, col) = parse_line_col(custom)
                .ok_or_else(|| CliError::user(format!("invalid --at value: {custom}")))?;
            format!("rstudioapi::document_position({line}L, {col}L)")
        }
    };
    let r_code = format!(
        "rstudioapi::insertText(location = {location}, text = {})",
        r_quote(text)
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn select(rpc: &RpcClient<'_>, range: &str) -> Result<Option<Value>, CliError> {
    let r_range = match parse_range(range) {
        Some(((l1, c1), (l2, c2))) => format!(
            "rstudioapi::document_range(rstudioapi::document_position({l1}L, {c1}L), \
                                         rstudioapi::document_position({l2}L, {c2}L))"
        ),
        None => {
            return Err(CliError::user(format!(
                "invalid range '{range}'. Expected 'L:C' or 'L1:C1-L2:C2'."
            )));
        }
    };
    let r_code = format!("rstudioapi::setSelectionRanges(list({r_range}))");
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn parse_line_col(s: &str) -> Option<(u32, u32)> {
    let (l, c) = s.split_once(':')?;
    Some((l.parse().ok()?, c.parse().ok()?))
}

fn parse_range(s: &str) -> Option<((u32, u32), (u32, u32))> {
    if let Some((a, b)) = s.split_once('-') {
        Some((parse_line_col(a)?, parse_line_col(b)?))
    } else {
        let pos = parse_line_col(s)?;
        Some((pos, pos))
    }
}
