use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::VERSION;
use crate::error::CliError;
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

const SKILL_NAME: &str = "rstudio";
const SKILL_FILE: &str = "SKILL.md";
const VERSION_PLACEHOLDER: &str = "__VERSION__";

/// Raw skill template embedded at compile time. Contains `__VERSION__`,
/// substituted to `VERSION` (= `CARGO_PKG_VERSION`) at install time.
pub const SKILL_TEMPLATE_RAW: &str = include_str!("../skills/rstudio.md");

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "skill",
        name: "show",
        summary: "Imprime le skill rstudio embarqué (markdown, version substituée).",
        description: "Le skill est un fichier markdown statique avec un frontmatter \
                      contenant `version`, alignée sur la version du CLI (les deux \
                      sont distribués ensemble dans le binaire).",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio skill show",
            explanation: "Imprime le contenu du skill (avec version réelle).",
        }],
        returns: "string (markdown)",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: None,
    },
    ActionSpec {
        category: "skill",
        name: "install",
        summary: "Installe le skill embarqué dans .claude/skills/rstudio/SKILL.md.",
        description: "Cherche le dossier `.claude/skills/` le plus proche en remontant \
                      depuis cwd, ou le crée dans cwd. Crée ensuite le sous-dossier \
                      `rstudio/` et y écrit `SKILL.md`. Refuse de surécrire un skill \
                      installé dont la version est strictement supérieure (semver), \
                      sauf --force.",
        params: &[
            ParamSpec {
                name: "--force",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Surécrit même si la version installée est plus récente.",
            },
            ParamSpec {
                name: "--target",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Dossier explicite de skills (sinon: .claude/skills/ ancestor de cwd, ou cwd/.claude/skills).",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio skill install",
                explanation: "Crée ./.claude/skills/rstudio/SKILL.md.",
            },
            ExampleSpec {
                cmd: "rstudio skill install --force",
                explanation: "Force la réinstallation même si la version installée est plus récente.",
            },
        ],
        returns: "{path: string, action: 'created'|'updated'|'unchanged', version: string}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "Skill installé plus récent et --force absent.",
            },
            ErrorSpec {
                kind: "internal",
                when: "Permissions insuffisantes pour écrire le fichier.",
            },
        ],
        rstudioapi_fn: None,
        rpc_method: None,
    },
];

#[derive(Subcommand, Debug)]
pub enum SkillCmd {
    /// Imprime le skill embarqué (markdown).
    Show,
    /// Installe le skill dans le projet courant.
    Install {
        /// Surécrit même si la version installée est strictement plus récente.
        #[arg(long)]
        force: bool,
        /// Dossier explicite de skills (parent de `rstudio/`).
        #[arg(long)]
        target: Option<PathBuf>,
    },
}

pub fn run(cmd: &SkillCmd) -> Result<Option<Value>, CliError> {
    match cmd {
        SkillCmd::Show => {
            print!("{}", rendered_template());
            Ok(None)
        }
        SkillCmd::Install { force, target } => install(*force, target.as_deref()),
    }
}

/// Returns the skill markdown with `__VERSION__` replaced by the CLI version.
pub fn rendered_template() -> String {
    SKILL_TEMPLATE_RAW.replace(VERSION_PLACEHOLDER, VERSION)
}

fn install(force: bool, target: Option<&Path>) -> Result<Option<Value>, CliError> {
    let skills_dir = match target {
        Some(p) => p.to_path_buf(),
        None => find_or_default_skills_dir()?,
    };
    let skill_dir = skills_dir.join(SKILL_NAME);
    fs::create_dir_all(&skill_dir).map_err(|e| {
        CliError::internal(format!("create skill dir {}: {e}", skill_dir.display()))
    })?;
    let target_file = skill_dir.join(SKILL_FILE);

    let action = match fs::read_to_string(&target_file) {
        Ok(existing) => {
            let existing_v = parse_skill_version(&existing).unwrap_or_default();
            let embedded_v = parse_semver(VERSION).unwrap_or_default();
            if existing_v == embedded_v {
                "unchanged"
            } else if existing_v > embedded_v && !force {
                return Err(CliError::user(format!(
                    "skill at {} is newer (v{}) than the embedded one (v{}). \
                     Pass --force to overwrite.",
                    target_file.display(),
                    fmt_semver(&existing_v),
                    VERSION
                )));
            } else {
                "updated"
            }
        }
        Err(_) => "created",
    };

    if action != "unchanged" {
        fs::write(&target_file, rendered_template())
            .map_err(|e| CliError::internal(format!("write {}: {e}", target_file.display())))?;
    }

    Ok(Some(json!({
        "path": target_file.to_string_lossy(),
        "action": action,
        "version": VERSION,
    })))
}

fn find_or_default_skills_dir() -> Result<PathBuf, CliError> {
    let cwd = env::current_dir().map_err(|e| CliError::internal(format!("getcwd: {e}")))?;
    let mut probe = cwd.clone();
    loop {
        let candidate = probe.join(".claude").join("skills");
        if candidate.is_dir() {
            return Ok(candidate);
        }
        if !probe.pop() {
            break;
        }
    }
    Ok(cwd.join(".claude").join("skills"))
}

type SemVer = (u32, u32, u32);

fn parse_skill_version(content: &str) -> Option<SemVer> {
    let trimmed = content.trim_start();
    let after_open = trimmed.strip_prefix("---")?;
    let end = after_open.find("\n---")?;
    let frontmatter = &after_open[..end];
    for line in frontmatter.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("version:") {
            return parse_semver(rest.trim());
        }
    }
    None
}

fn parse_semver(s: &str) -> Option<SemVer> {
    let mut parts = s.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn fmt_semver(v: &SemVer) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_from_frontmatter() {
        let s = "---\nname: foo\nversion: 0.2.5\n---\n# Body\n";
        assert_eq!(parse_skill_version(s), Some((0, 2, 5)));
    }

    #[test]
    fn returns_none_without_frontmatter() {
        assert_eq!(parse_skill_version("# no frontmatter\n"), None);
    }

    #[test]
    fn parse_semver_basic() {
        assert_eq!(parse_semver("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_semver("1.10.3"), Some((1, 10, 3)));
        assert_eq!(parse_semver("0.2"), None);
        assert_eq!(parse_semver("0.2.0.1"), None);
    }

    #[test]
    fn semver_ordering_is_numeric() {
        // Sanity: tuple ordering avoids the "0.10 < 0.2" string-compare trap.
        assert!(parse_semver("0.10.0").unwrap() > parse_semver("0.2.0").unwrap());
    }

    #[test]
    fn template_substitutes_version() {
        let rendered = rendered_template();
        assert!(rendered.contains(&format!("version: {}", VERSION)));
        assert!(!rendered.contains(VERSION_PLACEHOLDER));
    }
}
