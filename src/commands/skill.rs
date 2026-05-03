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
        summary: "Install the embedded skill into the current project, for the chosen agent tool.",
        description: "Writes the SKILL.md (the universal Anthropic-format skill markdown) into \
                      a tool-specific location:\n\
                      \n\
                      - `--for claude-code` (default): <root>/.claude/skills/rstudio/SKILL.md\n\
                      - `--for cursor`              : <root>/.cursor/rules/rstudio.mdc\n\
                      - `--for cline`               : <root>/.clinerules/rstudio.md\n\
                      \n\
                      For other agents that respect the SKILL.md format, use `--target <path>` \
                      to specify the destination directory directly, or pipe `rstudio skill \
                      show` into the desired location. The file content is the same Anthropic \
                      open-format markdown across every tool — only the directory and the \
                      filename extension vary.\n\
                      \n\
                      Refuses to overwrite an installed skill whose version is strictly newer \
                      (semver) unless --force.",
        params: &[
            ParamSpec {
                name: "--for",
                kind: ParamKind::Enum,
                required: false,
                default: Some("claude-code"),
                allowed: &["claude-code", "cursor", "cline"],
                description: "Target agent tool. Picks the right directory and filename.",
            },
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
                description: "Override the auto-resolved directory entirely. Bypasses --for's \
                              default placement.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio skill install",
                explanation: "Creates ./.claude/skills/rstudio/SKILL.md (Claude Code default).",
            },
            ExampleSpec {
                cmd: "rstudio skill install --for cursor",
                explanation: "Creates ./.cursor/rules/rstudio.mdc.",
            },
            ExampleSpec {
                cmd: "rstudio skill install --for cline",
                explanation: "Creates ./.clinerules/rstudio.md.",
            },
            ExampleSpec {
                cmd: "rstudio skill show > /path/to/agent/skills/rstudio.md",
                explanation: "Universal fallback for any agent whose convention isn't built in.",
            },
        ],
        returns: "{path: string, tool: string, action: 'created'|'updated'|'unchanged', version: string}",
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
    /// Install the skill in the current project, for the chosen agent tool.
    Install {
        /// Target agent tool. Picks the right directory and filename.
        #[arg(long = "for", value_parser = ["claude-code", "cursor", "cline"], default_value = "claude-code")]
        target_tool: String,
        /// Overwrite even if the installed version is strictly newer.
        #[arg(long)]
        force: bool,
        /// Override the auto-resolved directory entirely.
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
                default_text: true,
            })
        }
        SkillCmd::Install {
            target_tool,
            force,
            target,
        } => install(target_tool, *force, target.as_deref()),
    }
}

/// Per-tool install layout: relative directory under the project root, plus
/// the filename used inside that directory. The content (SKILL.md markdown)
/// is the same across every tool — Anthropic's SKILL.md is the universal
/// open format, adopted by Claude Code, Codex CLI, Gemini CLI, Copilot,
/// Cursor (with manual placement), Cline, etc. Only the destination differs.
struct InstallLayout {
    /// Tool name as displayed in the response (`tool` field).
    tool: &'static str,
    /// Directory relative to project root, e.g. ".claude/skills".
    rel_dir: &'static str,
    /// Whether a `rstudio/` subfolder is created beneath rel_dir.
    /// Claude Code expects `<rel_dir>/<name>/SKILL.md`; flatter conventions
    /// (cursor, cline) put a single file at `<rel_dir>/<name>.<ext>`.
    use_subdir: bool,
    /// Final filename written inside the resolved directory.
    file_name: &'static str,
}

fn layout_for(tool: &str) -> Result<InstallLayout, CliError> {
    match tool {
        "claude-code" => Ok(InstallLayout {
            tool: "claude-code",
            rel_dir: ".claude/skills",
            use_subdir: true,
            file_name: "SKILL.md",
        }),
        "cursor" => Ok(InstallLayout {
            tool: "cursor",
            rel_dir: ".cursor/rules",
            use_subdir: false,
            file_name: "rstudio.mdc",
        }),
        "cline" => Ok(InstallLayout {
            tool: "cline",
            rel_dir: ".clinerules",
            use_subdir: false,
            file_name: "rstudio.md",
        }),
        other => Err(CliError::user(format!(
            "unknown --for value '{other}'. Expected one of: claude-code, cursor, cline."
        ))),
    }
}

/// Returns the skill markdown with `__VERSION__` replaced by the CLI version.
pub fn rendered_template() -> String {
    SKILL_TEMPLATE_RAW.replace(VERSION_PLACEHOLDER, VERSION)
}

fn install(tool: &str, force: bool, target: Option<&Path>) -> Result<Reply, CliError> {
    let layout = layout_for(tool)?;

    // Directory resolution:
    // - --target overrides everything (used as the parent dir, layout's
    //   subdir/filename still apply within it)
    // - else: nearest <rel_dir> ancestor of cwd, or cwd/<rel_dir>
    let base_dir = match target {
        Some(p) => p.to_path_buf(),
        None => find_or_default_dir(layout.rel_dir)?,
    };

    let target_file = if layout.use_subdir {
        let skill_dir = base_dir.join(SKILL_NAME);
        fs::create_dir_all(&skill_dir).map_err(|e| {
            CliError::internal(format!("create skill dir {}: {e}", skill_dir.display()))
        })?;
        skill_dir.join(layout.file_name)
    } else {
        fs::create_dir_all(&base_dir)
            .map_err(|e| CliError::internal(format!("create dir {}: {e}", base_dir.display())))?;
        base_dir.join(layout.file_name)
    };

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
        "tool": layout.tool,
        "action": action,
        "version": VERSION,
    });
    let mark = ok_mark(stdout_is_tty());
    let text = format!(
        "{mark} {action:<9} {} (v{VERSION}, for {})\n",
        target_file.display(),
        layout.tool,
    );
    Ok(Reply::Adaptive {
        value,
        text,
        default_text: true,
    })
}

/// Find the nearest ancestor of cwd containing `rel_dir` (e.g.
/// ".claude/skills"), or default to `<cwd>/<rel_dir>`. Mirrors how each
/// agent tool typically expects to find its rules/skills root, regardless
/// of where in the project tree the command is invoked.
fn find_or_default_dir(rel_dir: &str) -> Result<PathBuf, CliError> {
    let cwd = env::current_dir().map_err(|e| CliError::internal(format!("getcwd: {e}")))?;
    let rel = PathBuf::from(rel_dir);
    let mut probe = cwd.clone();
    loop {
        let candidate = probe.join(&rel);
        if candidate.is_dir() {
            return Ok(candidate);
        }
        if !probe.pop() {
            break;
        }
    }
    Ok(cwd.join(rel))
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
