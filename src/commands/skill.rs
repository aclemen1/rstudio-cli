use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::json;

use crate::VERSION;
use crate::error::CliError;
use crate::output::{Reply, ok_mark, stdout_is_tty};
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
        summary: "Print the embedded rstudio skill (markdown, with version substituted).",
        description: "The skill is a static markdown file whose frontmatter `version` \
                      tracks the CLI version (they ship together inside the binary).",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio skill show",
            explanation: "Prints the skill content with the real version inlined.",
        }],
        returns: "string (markdown)",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: None,
    },
    ActionSpec {
        category: "skill",
        name: "install",
        summary: "Install the embedded skill at .claude/skills/rstudio/SKILL.md.",
        description: "Looks for the nearest `.claude/skills/` ancestor of cwd, or \
                      creates one in cwd. Then creates the `rstudio/` sub-directory \
                      and writes `SKILL.md`. Refuses to overwrite an installed skill \
                      whose version is strictly newer (semver) unless --force.",
        params: &[
            ParamSpec {
                name: "--force",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Overwrite even if the installed version is newer.",
            },
            ParamSpec {
                name: "--target",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Explicit skills directory (else: nearest .claude/skills/ ancestor of cwd, or cwd/.claude/skills).",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio skill install",
                explanation: "Creates ./.claude/skills/rstudio/SKILL.md.",
            },
            ExampleSpec {
                cmd: "rstudio skill install --force",
                explanation: "Force a reinstall even if the installed version is newer.",
            },
        ],
        returns: "{path: string, action: 'created'|'updated'|'unchanged', version: string}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "Installed skill is newer and --force was not passed.",
            },
            ErrorSpec {
                kind: "internal",
                when: "Insufficient permissions to write the file.",
            },
        ],
        rstudioapi_fn: None,
        rpc_method: None,
    },
];

#[derive(Subcommand, Debug)]
pub enum SkillCmd {
    /// Print the embedded skill (markdown).
    Show,
    /// Install the skill in the current project.
    Install {
        /// Overwrite even if the installed version is strictly newer.
        #[arg(long)]
        force: bool,
        /// Explicit skills directory (parent of `rstudio/`).
        #[arg(long)]
        target: Option<PathBuf>,
    },
}

pub fn run(cmd: &SkillCmd) -> Result<Reply, CliError> {
    match cmd {
        SkillCmd::Show => {
            // Text mode: raw markdown to stdout (pipeable).
            // JSON mode: envelope wrapping the markdown as a string.
            let md = rendered_template();
            Ok(Reply::Adaptive {
                value: json!(md),
                text: md,
            })
        }
        SkillCmd::Install { force, target } => install(*force, target.as_deref()),
    }
}

/// Returns the skill markdown with `__VERSION__` replaced by the CLI version.
pub fn rendered_template() -> String {
    SKILL_TEMPLATE_RAW.replace(VERSION_PLACEHOLDER, VERSION)
}

fn install(force: bool, target: Option<&Path>) -> Result<Reply, CliError> {
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

    let value = json!({
        "path": target_file.to_string_lossy(),
        "action": action,
        "version": VERSION,
    });
    let mark = ok_mark(stdout_is_tty());
    let text = format!(
        "{mark} {action:<9} {} (v{VERSION})\n",
        target_file.display()
    );
    Ok(Reply::Adaptive { value, text })
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
