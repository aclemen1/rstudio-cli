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
const UPDATE_SECTION_PLACEHOLDER: &str = "__UPDATE_SECTION__";

/// Raw skill template embedded at compile time. Contains `__VERSION__`,
/// substituted to `VERSION` (= `CARGO_PKG_VERSION`) at install time.
pub const SKILL_TEMPLATE_RAW: &str = include_str!("../skills/rstudio.md");

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "skill",
        name: "show",
        summary: "Print the embedded rstudio skill (markdown, with version + install path baked in).",
        description: "The skill is a static markdown file whose frontmatter `version` \
                      tracks the CLI version (they ship together inside the binary).\n\
                      \n\
                      Accepts the same `--for` and `--target` as `install`: the self-update \
                      section embedded in the output names the exact path the skill is meant \
                      to live at and the precise `rstudio skill install` invocation needed to \
                      overwrite it. This means `rstudio skill show [--for X] [--target Y] > \
                      <path>` produces a file byte-identical to what `install` would have \
                      written there.",
        params: &[
            ParamSpec {
                name: "--for",
                kind: ParamKind::Enum,
                required: false,
                default: Some("claude-code"),
                allowed: &["claude-code", "cursor", "cline"],
                description: "Target agent tool whose path/filename is baked into the \
                              self-update section.",
            },
            ParamSpec {
                name: "--target",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Override the auto-resolved directory baked into the self-update \
                              section. `show` never writes — this only affects content.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio skill show",
                explanation: "Prints the skill with the default install path baked into its \
                              self-update section.",
            },
            ExampleSpec {
                cmd: "rstudio skill show --target /opt/skills > /opt/skills/rstudio/SKILL.md",
                explanation: "Universal fallback for any agent whose convention isn't built \
                              in: the file written contains the exact reinstall command for \
                              that location.",
            },
        ],
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
    /// Print the embedded skill (markdown). Accepts the same `--for` and
    /// `--target` as `install` so that piping the output to a file
    /// (`rstudio skill show --target X > X/rstudio/SKILL.md`) yields a
    /// document whose self-update section is baked for that very location
    /// — identical to what `install` would have written there.
    Show {
        /// Target agent tool. Picks the directory and filename whose path
        /// gets baked into the self-update section.
        #[arg(long = "for", value_parser = ["claude-code", "cursor", "cline"], default_value = "claude-code")]
        target_tool: String,
        /// Override the auto-resolved directory entirely. Same semantics
        /// as `install --target`. Used purely to compute the path baked
        /// into the markdown — `show` never writes anything itself.
        #[arg(long)]
        target: Option<PathBuf>,
    },
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
        SkillCmd::Show {
            target_tool,
            target,
        } => {
            let (_, _, md) = render_for(target_tool, target.as_deref())?;
            // Text mode: raw markdown to stdout (pipeable).
            // JSON mode: envelope wrapping the markdown as a string.
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

/// Returns the skill markdown rendered for `(tool, target)`. Both `show`
/// and `install` go through this so their outputs agree byte-for-byte —
/// the user can pipe `show` to a file and get exactly what `install` would
/// have written there. Pure: no filesystem side effects (the path is
/// resolved by probing ancestors read-only via `find_or_default_dir`).
fn render_for(
    tool: &str,
    target: Option<&Path>,
) -> Result<(InstallLayout, PathBuf, String), CliError> {
    let (layout, target_file) = compute_target_file(tool, target)?;
    let update_cmd = build_update_command(tool, target);
    let section = build_update_section(&target_file, &update_cmd);
    let md = SKILL_TEMPLATE_RAW
        .replace(VERSION_PLACEHOLDER, VERSION)
        .replace(UPDATE_SECTION_PLACEHOLDER, &section);
    Ok((layout, target_file, md))
}

/// Resolves the destination file path without touching the filesystem
/// (no `create_dir_all`). `install` creates dirs separately before writing;
/// `show` only needs the path to bake into the rendered markdown.
fn compute_target_file(
    tool: &str,
    target: Option<&Path>,
) -> Result<(InstallLayout, PathBuf), CliError> {
    let layout = layout_for(tool)?;
    let base_dir = match target {
        Some(p) => p.to_path_buf(),
        None => find_or_default_dir(layout.rel_dir)?,
    };
    let target_file = if layout.use_subdir {
        base_dir.join(SKILL_NAME).join(layout.file_name)
    } else {
        base_dir.join(layout.file_name)
    };
    Ok((layout, target_file))
}

/// Builds the update section embedded in the skill. Worded neutrally so
/// it reads correctly whether the file ended up on disk via `install`
/// (which wrote it) or via `show > path` (which the user piped). Both
/// produce the same path resolution and the same baked command.
fn build_update_section(target_file: &Path, update_cmd: &str) -> String {
    format!(
        "This skill is meant to live at:\n\
         \n    {}\n\
         \n\
         To overwrite it with the embedded skill from a newer CLI binary, run the\n\
         exact command below — it has been baked at render time to point at that\n\
         very location:\n\
         \n    {update_cmd}",
        target_file.display(),
    )
}

/// Builds the exact `rstudio skill install` invocation that, when re-run,
/// will overwrite the skill file at the same location it was just written
/// to. `--target` is baked in only when the user explicitly passed one
/// (otherwise the auto-resolved default — nearest ancestor or cwd — would
/// be brittle to bake as an absolute path). `--for` is included only when
/// non-default, to keep the line minimal in the common case.
fn build_update_command(tool: &str, target: Option<&Path>) -> String {
    let mut cmd = String::from("rstudio skill install --force");
    if tool != "claude-code" {
        cmd.push_str(&format!(" --for {tool}"));
    }
    if let Some(p) = target {
        cmd.push_str(&format!(
            " --target {}",
            shell_quote(&p.display().to_string())
        ));
    }
    cmd
}

/// Minimal POSIX single-quote escaping for paths embedded in the skill's
/// update command. Safe for any path; only adds quotes when needed.
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'/' | b'.' | b'_' | b'-' | b'+' | b'=' | b':' | b',')
        })
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn install(tool: &str, force: bool, target: Option<&Path>) -> Result<Reply, CliError> {
    // Compute path & rendered content first (pure); only touch the
    // filesystem after we know we're going to write.
    let (layout, target_file, rendered) = render_for(tool, target)?;

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
        // Create parent dirs only now (right before write).
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CliError::internal(format!("create dir {}: {e}", parent.display())))?;
        }
        fs::write(&target_file, rendered)
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
    fn render_substitutes_version_and_section() {
        let (_, _, md) = render_for("claude-code", Some(Path::new("/tmp/x"))).unwrap();
        assert!(md.contains(&format!("version: {}", VERSION)));
        assert!(!md.contains(VERSION_PLACEHOLDER));
        assert!(!md.contains(UPDATE_SECTION_PLACEHOLDER));
    }

    #[test]
    fn render_bakes_target_path_into_section() {
        let (_, file, md) = render_for("claude-code", Some(Path::new("/tmp/my skills"))).unwrap();
        // Path with a space gets single-quoted in the command.
        assert!(md.contains("--target '/tmp/my skills'"));
        // The skill's "lives at" block names the actual file we'd write to.
        assert!(md.contains(&format!("    {}\n", file.display())));
        assert_eq!(file, Path::new("/tmp/my skills/rstudio/SKILL.md"));
    }

    #[test]
    fn show_and_install_render_identically() {
        // The whole point of giving `show` the same params as `install`:
        // piping `show` to a file must yield byte-identical content.
        let (_, _, a) = render_for("cursor", Some(Path::new("/opt/skills"))).unwrap();
        let (_, _, b) = render_for("cursor", Some(Path::new("/opt/skills"))).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn install_rendering_omits_target_when_default() {
        let cmd = build_update_command("claude-code", None);
        assert_eq!(cmd, "rstudio skill install --force");
    }

    #[test]
    fn install_rendering_includes_for_when_non_default() {
        let cmd = build_update_command("cursor", None);
        assert_eq!(cmd, "rstudio skill install --force --for cursor");
        let cmd = build_update_command("cline", Some(Path::new("/opt/skills")));
        assert_eq!(
            cmd,
            "rstudio skill install --force --for cline --target /opt/skills"
        );
    }

    #[test]
    fn shell_quote_safe_chars() {
        assert_eq!(shell_quote("/usr/local/bin"), "/usr/local/bin");
        assert_eq!(shell_quote("a.b-c_d+e=f:g,h"), "a.b-c_d+e=f:g,h");
        assert_eq!(shell_quote("with space"), "'with space'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
