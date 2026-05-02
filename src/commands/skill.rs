use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::SKILL_VERSION;
use crate::error::CliError;
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

/// The skill template embedded at compile time. Bumping its semantic version
/// requires also bumping `SKILL_VERSION` in `lib.rs` and the `skill_version`
/// frontmatter at the top of the markdown.
pub const SKILL_TEMPLATE: &str = include_str!("../skills/rstudio.md");

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "skill",
        name: "show",
        summary: "Imprime le skill rstudio embarqué dans le binaire (markdown).",
        description: "Le skill est un fichier markdown statique avec un frontmatter \
                      contenant `skill_version`. Sa version est aussi exposée par \
                      `rstudio version` (champ `skill`).",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio skill show",
            explanation: "Imprime le contenu du skill embarqué.",
        }],
        returns: "string (markdown)",
        errors: &[],
    },
    ActionSpec {
        category: "skill",
        name: "install",
        summary: "Installe le skill embarqué dans le projet courant (.claude/skills/rstudio.md).",
        description: "Cherche le dossier `.claude/skills/` le plus proche en remontant \
                      depuis cwd, ou le crée dans cwd. Refuse de surécrire un skill \
                      installé dont la version est >= à celle embarquée, sauf --force.",
        params: &[
            ParamSpec {
                name: "--force",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Surécrit même si la version installée est >= à celle embarquée.",
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
                explanation: "Installe ou met à jour ./.claude/skills/rstudio.md.",
            },
            ExampleSpec {
                cmd: "rstudio skill install --force",
                explanation: "Force la réinstallation même si la version installée est plus récente.",
            },
        ],
        returns: "{path: string, action: 'created'|'updated'|'unchanged', skill_version: int}",
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
    },
];

#[derive(Subcommand, Debug)]
pub enum SkillCmd {
    /// Imprime le skill embarqué (markdown).
    Show,
    /// Installe le skill dans le projet courant.
    Install {
        /// Surécrit même si la version installée est >= à celle embarquée.
        #[arg(long)]
        force: bool,
        /// Dossier explicite de skills.
        #[arg(long)]
        target: Option<PathBuf>,
    },
}

pub fn run(cmd: &SkillCmd) -> Result<Option<Value>, CliError> {
    match cmd {
        SkillCmd::Show => {
            print!("{SKILL_TEMPLATE}");
            Ok(None)
        }
        SkillCmd::Install { force, target } => install(*force, target.as_deref()),
    }
}

fn install(force: bool, target: Option<&Path>) -> Result<Option<Value>, CliError> {
    let skills_dir = match target {
        Some(p) => p.to_path_buf(),
        None => find_or_default_skills_dir()?,
    };
    fs::create_dir_all(&skills_dir).map_err(|e| {
        CliError::internal(format!(
            "create skills dir {}: {e}",
            skills_dir.display()
        ))
    })?;
    let target_file = skills_dir.join("rstudio.md");

    let action = match fs::read_to_string(&target_file) {
        Ok(existing) => {
            let existing_v = parse_skill_version(&existing).unwrap_or(0);
            if existing_v == SKILL_VERSION {
                "unchanged"
            } else if existing_v > SKILL_VERSION && !force {
                return Err(CliError::user(format!(
                    "skill at {} is newer (v{existing_v}) than the embedded one (v{}). \
                     Pass --force to overwrite.",
                    target_file.display(),
                    SKILL_VERSION
                )));
            } else {
                "updated"
            }
        }
        Err(_) => "created",
    };

    if action != "unchanged" {
        fs::write(&target_file, SKILL_TEMPLATE).map_err(|e| {
            CliError::internal(format!("write {}: {e}", target_file.display()))
        })?;
    }

    Ok(Some(json!({
        "path": target_file.to_string_lossy(),
        "action": action,
        "skill_version": SKILL_VERSION,
    })))
}

fn find_or_default_skills_dir() -> Result<PathBuf, CliError> {
    let cwd = env::current_dir()
        .map_err(|e| CliError::internal(format!("getcwd: {e}")))?;
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

fn parse_skill_version(content: &str) -> Option<u32> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_open = &trimmed[3..];
    let end = after_open.find("\n---")?;
    let frontmatter = &after_open[..end];
    for line in frontmatter.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("skill_version:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_version_from_frontmatter() {
        let s = "---\nname: foo\nskill_version: 7\n---\n# Body\n";
        assert_eq!(parse_skill_version(s), Some(7));
    }

    #[test]
    fn returns_none_without_frontmatter() {
        assert_eq!(parse_skill_version("# no frontmatter\n"), None);
    }

    #[test]
    fn embedded_template_has_matching_version() {
        // Must stay in lockstep with SKILL_VERSION in lib.rs.
        let v = parse_skill_version(SKILL_TEMPLATE).expect("frontmatter parses");
        assert_eq!(v, SKILL_VERSION);
    }
}
