use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[ActionSpec {
    category: "editor",
    name: "open",
    summary: "Ouvre un fichier dans l'éditeur RStudio.",
    description: "Sans --line, utilise un postback editfile (non bloquant côté R). \
                  Avec --line, route via rstudioapi::navigateToFile pour ouvrir ET \
                  positionner le curseur en un seul appel.",
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
            default: Some("1"),
            allowed: &[],
            description: "Colonne (1-based) ; nécessite --line.",
        },
    ],
    examples: &[
        ExampleSpec {
            cmd: "rstudio editor open ~/code/aclemen1/rstudio-cli/Cargo.toml",
            explanation: "Ouvre Cargo.toml sans changer la position du curseur.",
        },
        ExampleSpec {
            cmd: "rstudio editor open src/main.rs --line 42",
            explanation: "Ouvre src/main.rs et place le curseur en ligne 42, colonne 1.",
        },
    ],
    returns: "{path: string, line: int|null, col: int|null}",
    errors: &[
        ErrorSpec {
            kind: "user_error",
            when: "Fichier introuvable (canonicalize fail).",
        },
        ErrorSpec {
            kind: "r_error",
            when: "rstudioapi::navigateToFile rejette le chemin avec --line.",
        },
    ],
}];

#[derive(Subcommand, Debug)]
pub enum EditorCmd {
    /// Ouvre un fichier dans l'éditeur RStudio.
    Open {
        /// Chemin du fichier à ouvrir.
        path: PathBuf,
        /// Saute à cette ligne après ouverture.
        #[arg(long)]
        line: Option<u32>,
        /// Saute à cette colonne après ouverture (nécessite --line).
        #[arg(long)]
        col: Option<u32>,
    },
}

pub fn run(cmd: &EditorCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        EditorCmd::Open { path, line, col } => {
            let abs = path
                .canonicalize()
                .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
            let abs_str = abs.to_string_lossy().into_owned();

            match line {
                None => {
                    let pb = rpc.postback("editfile", &abs_str)?;
                    if pb.exit_code != 0 {
                        return Err(CliError::rpc(
                            pb.exit_code,
                            format!("editfile postback failed (exit_code={})", pb.exit_code),
                        ));
                    }
                }
                Some(l) => {
                    let c = col.unwrap_or(1);
                    let r_code = format!(
                        "rstudioapi::navigateToFile({}, line = {}L, column = {}L)",
                        r_quote(&abs_str),
                        l,
                        c
                    );
                    r_eval::run_silent(rpc, &r_code)?;
                }
            }

            Ok(Some(json!({
                "path": abs_str,
                "line": line,
                "col": col,
            })))
        }
    }
}
