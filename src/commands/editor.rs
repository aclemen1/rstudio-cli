use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};

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
