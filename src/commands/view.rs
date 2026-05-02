use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "view",
        name: "html",
        summary: "Affiche un fichier HTML local ou une URL dans le panneau Viewer.",
        description: "Wrap rstudioapi::viewer(url). Pour un fichier local, le chemin \
                      est résolu en absolu via canonicalize.",
        params: &[ParamSpec {
            name: "target",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Chemin local ou URL (http://, https://).",
        }],
        examples: &[
            ExampleSpec {
                cmd: "rstudio view html ~/reports/coverage.html",
                explanation: "Ouvre le fichier dans le panneau Viewer.",
            },
            ExampleSpec {
                cmd: "rstudio view html https://example.com",
                explanation: "Affiche la page distante (selon les permissions du navigateur).",
            },
        ],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Chemin local introuvable.",
        }],
    },
    ActionSpec {
        category: "view",
        name: "files",
        summary: "Navigue le panneau Files vers un dossier.",
        description: "Wrap rstudioapi::filesPaneNavigate(path).",
        params: &[ParamSpec {
            name: "path",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Chemin du dossier cible.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio view files ~/projects/my-project",
            explanation: "Pointe le panneau Files vers ce dossier.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Chemin introuvable.",
        }],
    },
    ActionSpec {
        category: "view",
        name: "mark",
        summary: "Affiche un panneau Markers (style linter) avec une liste de problèmes.",
        description: "Wrap rstudioapi::sourceMarkers(name, markers, autoSelect). \
                      Les markers sont passés en JSON via --markers ou stdin (un array \
                      d'objets {type, file, line, column?, message}). type ∈ \
                      {error,warning,info,style,usage,box}.",
        params: &[
            ParamSpec {
                name: "--name",
                kind: ParamKind::String,
                required: false,
                default: Some("rstudio-cli"),
                allowed: &[],
                description: "Nom de la collection affiché en titre du panneau Markers.",
            },
            ParamSpec {
                name: "--markers",
                kind: ParamKind::Json,
                required: false,
                default: None,
                allowed: &[],
                description: "JSON array inline. Si absent, lu depuis stdin.",
            },
            ParamSpec {
                name: "--auto-select",
                kind: ParamKind::Enum,
                required: false,
                default: Some("none"),
                allowed: &["none", "first", "error"],
                description: "Quel marker activer automatiquement après création.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio view mark --name 'lint' --markers '[{\"type\":\"warning\",\"file\":\"~/p/foo.R\",\"line\":12,\"message\":\"Unused var\"}]'",
                explanation: "Affiche un marker warning sur foo.R ligne 12.",
            },
            ExampleSpec {
                cmd: "cat markers.json | rstudio view mark --name 'lint'",
                explanation: "Lit les markers depuis stdin (un JSON array).",
            },
        ],
        returns: "{count: int}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "JSON invalide ou markers vides.",
            },
            ErrorSpec {
                kind: "r_error",
                when: "Champ obligatoire manquant ou type invalide.",
            },
        ],
    },
];

#[derive(Subcommand, Debug)]
pub enum ViewCmd {
    /// Affiche un fichier HTML local ou une URL dans le panneau Viewer.
    Html { target: String },
    /// Navigue le panneau Files vers un dossier.
    Files { path: PathBuf },
    /// Affiche des markers (style linter) dans le panneau Markers.
    Mark {
        /// Nom de la collection affiché en titre.
        #[arg(long, default_value = "rstudio-cli")]
        name: String,
        /// Markers en JSON inline (sinon lu sur stdin).
        #[arg(long)]
        markers: Option<String>,
        /// Quel marker activer après création.
        #[arg(long, default_value = "none")]
        auto_select: String,
    },
}

pub fn run(cmd: &ViewCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        ViewCmd::Html { target } => html(rpc, target),
        ViewCmd::Files { path } => files(rpc, path),
        ViewCmd::Mark {
            name,
            markers,
            auto_select,
        } => mark(rpc, name, markers.as_deref(), auto_select),
    }
}

fn html(rpc: &RpcClient<'_>, target: &str) -> Result<Option<Value>, CliError> {
    let resolved = if target.starts_with("http://") || target.starts_with("https://") {
        target.to_string()
    } else {
        let p = std::path::Path::new(target).canonicalize().map_err(|e| {
            CliError::user(format!("cannot resolve {target}: {e}"))
        })?;
        p.to_string_lossy().into_owned()
    };
    let r_code = format!("rstudioapi::viewer({})", r_quote(&resolved));
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({ "target": resolved })))
}

fn files(rpc: &RpcClient<'_>, path: &PathBuf) -> Result<Option<Value>, CliError> {
    let abs = path
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {}: {e}", path.display())))?;
    let abs_str = abs.to_string_lossy().into_owned();
    let r_code = format!("rstudioapi::filesPaneNavigate({})", r_quote(&abs_str));
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({ "path": abs_str })))
}

fn mark(
    rpc: &RpcClient<'_>,
    name: &str,
    markers_inline: Option<&str>,
    auto_select: &str,
) -> Result<Option<Value>, CliError> {
    if !["none", "first", "error"].contains(&auto_select) {
        return Err(CliError::user(format!(
            "invalid --auto-select '{auto_select}'. Expected: none, first, error."
        )));
    }
    let json_str = match markers_inline {
        Some(s) => s.to_string(),
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| CliError::user(format!("read stdin: {e}")))?;
            if buf.trim().is_empty() {
                return Err(CliError::user(
                    "no markers given: pass --markers or pipe a JSON array on stdin",
                ));
            }
            buf
        }
    };

    let parsed: Value = serde_json::from_str(&json_str)
        .map_err(|e| CliError::user(format!("invalid markers JSON: {e}")))?;
    let arr = parsed
        .as_array()
        .ok_or_else(|| CliError::user("markers must be a JSON array"))?;
    if arr.is_empty() {
        return Err(CliError::user("markers array is empty"));
    }
    let count = arr.len();

    let r_code = format!(
        r#"local({{
  m <- jsonlite::fromJSON({json_q}, simplifyDataFrame = TRUE)
  if (!is.data.frame(m)) m <- as.data.frame(m, stringsAsFactors = FALSE)
  if (is.null(m$column)) m$column <- 1L
  rstudioapi::sourceMarkers(
    name = {name_q},
    markers = m,
    autoSelect = {auto_q}
  )
}})"#,
        json_q = r_quote(&json_str),
        name_q = r_quote(name),
        auto_q = r_quote(auto_select),
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(Some(json!({ "count": count, "name": name })))
}
