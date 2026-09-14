//! `rstudio mcp --via` — run the MCP server where the rsession lives.
//!
//! `rstudio mcp` talks to the rsession over a Unix socket, so the binary must
//! run on the host/container that owns the session. When RStudio Server runs
//! in a container or on a remote host and the MCP client runs on the user's
//! machine, `--via "<transport prefix>"` makes the local binary a transparent
//! relay: it execs `<prefix> rstudio mcp` (with a `--no-via` guard when the
//! remote supports it, see below) and lets JSON-RPC frames flow through
//! stdin/stdout unchanged. The prefix is any command that runs
//! its trailing arguments in the session's context without a PTY, e.g.
//! `docker compose exec -T -u ds -e USER=ds ide` or `ssh user@host`.
//!
//! The value can also come from a per-project `.rstudio-cli.toml` (`[mcp] via`)
//! found by walking up from the current directory, or from the user-level
//! `<config-dir>/rstudio-cli/config.toml` (XDG on Linux). Precedence, highest
//! first: `--no-via` (or `--via ""`) forcing local, then `--via <prefix>`,
//! then the project file, then the user file.
//!
//! Re-entrancy: the `rstudio mcp` reached inside the container would read the
//! same config file and tunnel again. The guard is a `--no-via` flag appended
//! to the remote command (an environment variable would not work: `docker
//! compose exec` and `ssh` do not forward environment by default, but arguments
//! always cross). To avoid breaking a remote binary older than 0.21.0 — which
//! rejects the unknown `--no-via` flag — `exec_tunnel` first probes the remote
//! version with `<prefix> rstudio version` and appends `--no-via` only when the
//! remote is >= 0.21.0. An older remote gets no unknown flag (it never reads
//! the config file either, so it cannot loop); a newer remote gets the guard.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::CliError;

const PROJECT_CONFIG: &str = ".rstudio-cli.toml";

/// First rstudio-cli version that understands the `--no-via` guard flag and
/// reads `.rstudio-cli.toml`. The probe compares the remote version to this.
const NO_VIA_MIN_VERSION: (u64, u64, u64) = (0, 21, 0);

/// Resolve the effective `via` prefix from the flags and config files.
///
/// `Ok(None)` means "serve locally" (no tunnel). `Err` means a config file was
/// present but could not be parsed — surfaced so a typo is noticed rather than
/// silently ignored.
pub fn resolve(
    cli_via: Option<&str>,
    no_via: bool,
    project_start: &Path,
    xdg_config_dir: Option<&Path>,
) -> Result<Option<String>, CliError> {
    if no_via {
        return Ok(None);
    }
    match cli_via {
        Some("") => return Ok(None), // explicit force-local
        Some(v) => return Ok(Some(v.to_string())),
        None => {}
    }
    if let Some(path) = find_project_config(project_start)
        && let Some(v) = read_via(&path)?
    {
        return Ok(Some(v));
    }
    if let Some(dir) = xdg_config_dir {
        let path = dir.join("rstudio-cli").join("config.toml");
        if path.is_file()
            && let Some(v) = read_via(&path)?
        {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

/// Walk up from `start` (inclusive) looking for a `.rstudio-cli.toml`.
fn find_project_config(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(PROJECT_CONFIG);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Read `[mcp] via` from a TOML file. `Ok(None)` when the file parses but has
/// no such key; `Err` when the file is not valid TOML.
fn read_via(path: &Path) -> Result<Option<String>, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::user(format!("cannot read {}: {e}", path.display())))?;
    let value: toml::Value = text
        .parse()
        .map_err(|e| CliError::user(format!("invalid TOML in {}: {e}", path.display())))?;
    Ok(value
        .get("mcp")
        .and_then(|m| m.get("via"))
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

/// Split a transport prefix into argv, honouring single and double quotes and
/// backslash escapes outside quotes. No variable expansion.
pub fn split_command(s: &str) -> Result<Vec<String>, CliError> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if in_token {
                    tokens.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            '\'' => {
                in_token = true;
                let mut closed = false;
                for q in chars.by_ref() {
                    if q == '\'' {
                        closed = true;
                        break;
                    }
                    cur.push(q);
                }
                if !closed {
                    return Err(CliError::user(format!(
                        "--via: unterminated single quote in `{s}`"
                    )));
                }
            }
            '"' => {
                in_token = true;
                let mut closed = false;
                while let Some(q) = chars.next() {
                    if q == '"' {
                        closed = true;
                        break;
                    }
                    if q == '\\'
                        && let Some(&n) = chars.peek()
                        && (n == '"' || n == '\\')
                    {
                        cur.push(n);
                        chars.next();
                        continue;
                    }
                    cur.push(q);
                }
                if !closed {
                    return Err(CliError::user(format!(
                        "--via: unterminated double quote in `{s}`"
                    )));
                }
            }
            '\\' => {
                in_token = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            other => {
                in_token = true;
                cur.push(other);
            }
        }
    }
    if in_token {
        tokens.push(cur);
    }
    if tokens.is_empty() {
        return Err(CliError::user(format!(
            "--via: empty transport prefix (`{s}`); nothing to exec"
        )));
    }
    Ok(tokens)
}

/// Build the child argv: the split prefix, then `rstudio mcp`, then the
/// `--no-via` guard when `append_guard` is true (i.e. the remote is >= 0.21.0).
pub fn build_child_argv(via: &str, append_guard: bool) -> Result<Vec<String>, CliError> {
    let mut argv = split_command(via)?;
    argv.push("rstudio".to_string());
    argv.push("mcp".to_string());
    if append_guard {
        argv.push("--no-via".to_string());
    }
    Ok(argv)
}

/// Decide from `<prefix> rstudio version` output whether the remote understands
/// the `--no-via` guard. Unparseable or older-than-0.21.0 output returns false
/// — the safe choice: an older remote must not receive the unknown flag, and it
/// cannot loop because it does not read the config file.
pub fn remote_supports_no_via(version_output: &str) -> bool {
    let Some(token) = version_output.split_whitespace().next() else {
        return false;
    };
    let mut parts = token.split('.');
    let parse = |p: Option<&str>| -> Option<u64> {
        let s = p?;
        let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    };
    let (Some(major), Some(minor), Some(patch)) = (
        parse(parts.next()),
        parse(parts.next()),
        parse(parts.next()),
    ) else {
        return false;
    };
    (major, minor, patch) >= NO_VIA_MIN_VERSION
}

/// Probe the remote binary version by running `<prefix> rstudio version` with
/// stdio not connected to our own. Any failure (spawn error, non-zero exit,
/// unparseable output) yields false — the safe default.
fn probe_remote_supports_no_via(prefix: &[String]) -> bool {
    let (Some(cmd), args) = (prefix.first(), &prefix[1..]) else {
        return false;
    };
    let out = Command::new(cmd)
        .args(args)
        .args(["rstudio", "version"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => remote_supports_no_via(&String::from_utf8_lossy(&o.stdout)),
        _ => false,
    }
}

/// Exec the tunnel, replacing this process. Probes the remote version first to
/// decide whether to append the `--no-via` guard. Returns only on failure to
/// exec the server.
pub fn exec_tunnel(via: &str) -> Result<(), CliError> {
    let prefix = split_command(via)?;
    let append_guard = probe_remote_supports_no_via(&prefix);
    let argv = build_child_argv(via, append_guard)?;
    let err = Command::new(&argv[0]).args(&argv[1..]).exec();
    Err(CliError::internal(format!(
        "mcp --via: failed to exec `{}`: {err}",
        argv[0]
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // --- split_command ---------------------------------------------------

    #[test]
    fn split_plain_whitespace() {
        assert_eq!(
            split_command("docker compose exec -T ide").unwrap(),
            ["docker", "compose", "exec", "-T", "ide"]
        );
    }

    #[test]
    fn split_double_quotes_keep_spaces() {
        assert_eq!(
            split_command(r#"sh -c "echo hi there""#).unwrap(),
            ["sh", "-c", "echo hi there"]
        );
    }

    #[test]
    fn split_single_quotes_keep_spaces() {
        assert_eq!(split_command("a 'b c' d").unwrap(), ["a", "b c", "d"]);
    }

    #[test]
    fn split_unterminated_quote_is_error() {
        assert!(split_command(r#"a "b c"#).is_err());
    }

    #[test]
    fn split_empty_is_error() {
        // A tunnel with no command is meaningless.
        assert!(split_command("   ").is_err());
    }

    // --- build_child_argv ------------------------------------------------

    #[test]
    fn child_argv_appends_guard_when_remote_is_recent() {
        assert_eq!(
            build_child_argv("docker compose exec -T ide", true).unwrap(),
            [
                "docker", "compose", "exec", "-T", "ide", "rstudio", "mcp", "--no-via"
            ]
        );
    }

    #[test]
    fn child_argv_omits_guard_for_old_remote() {
        // An older remote binary rejects the unknown --no-via flag, so it must
        // not be appended. It cannot loop: it does not read the config file.
        assert_eq!(
            build_child_argv("docker compose exec -T ide", false).unwrap(),
            ["docker", "compose", "exec", "-T", "ide", "rstudio", "mcp"]
        );
    }

    // --- remote_supports_no_via (version probe decision) ----------------

    #[test]
    fn version_probe_at_min_is_supported() {
        assert!(remote_supports_no_via("0.21.0"));
        assert!(remote_supports_no_via("0.21.0\n"));
    }

    #[test]
    fn version_probe_newer_is_supported() {
        assert!(remote_supports_no_via("0.22.0"));
        assert!(remote_supports_no_via("1.0.0"));
        assert!(remote_supports_no_via("0.21.5\n"));
    }

    #[test]
    fn version_probe_older_is_not_supported() {
        assert!(!remote_supports_no_via("0.20.3"));
        assert!(!remote_supports_no_via("0.20.99"));
        assert!(!remote_supports_no_via("0.9.0"));
    }

    #[test]
    fn version_probe_unparseable_is_not_supported() {
        assert!(!remote_supports_no_via(""));
        assert!(!remote_supports_no_via("not a version"));
        assert!(!remote_supports_no_via("0.21"));
    }

    #[test]
    fn version_probe_ignores_trailing_and_suffix() {
        // Extra output lines, and a build/pre-release suffix on the patch.
        assert!(remote_supports_no_via("0.21.0+build.7\nextra line"));
    }

    // --- resolve precedence ---------------------------------------------

    #[test]
    fn resolve_no_via_forces_local() {
        let d = tmp();
        assert_eq!(
            resolve(Some("docker ide"), true, d.path(), None).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_empty_via_forces_local() {
        let d = tmp();
        assert_eq!(resolve(Some(""), false, d.path(), None).unwrap(), None);
    }

    #[test]
    fn resolve_explicit_via_wins() {
        let d = tmp();
        assert_eq!(
            resolve(Some("ssh host"), false, d.path(), None).unwrap(),
            Some("ssh host".to_string())
        );
    }

    #[test]
    fn resolve_project_file_when_no_flag() {
        let d = tmp();
        fs::write(
            d.path().join(PROJECT_CONFIG),
            "[mcp]\nvia = \"docker compose exec -T ide\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve(None, false, d.path(), None).unwrap(),
            Some("docker compose exec -T ide".to_string())
        );
    }

    #[test]
    fn resolve_project_file_found_by_walking_up() {
        let d = tmp();
        let deep = d.path().join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        fs::write(d.path().join(PROJECT_CONFIG), "[mcp]\nvia = \"ssh host\"\n").unwrap();
        assert_eq!(
            resolve(None, false, &deep, None).unwrap(),
            Some("ssh host".to_string())
        );
    }

    #[test]
    fn resolve_xdg_user_file_when_no_project() {
        let proj = tmp();
        let xdg = tmp();
        fs::create_dir_all(xdg.path().join("rstudio-cli")).unwrap();
        fs::write(
            xdg.path().join("rstudio-cli").join("config.toml"),
            "[mcp]\nvia = \"ssh dev\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve(None, false, proj.path(), Some(xdg.path())).unwrap(),
            Some("ssh dev".to_string())
        );
    }

    #[test]
    fn resolve_project_wins_over_xdg() {
        let proj = tmp();
        let xdg = tmp();
        fs::write(proj.path().join(PROJECT_CONFIG), "[mcp]\nvia = \"proj\"\n").unwrap();
        fs::create_dir_all(xdg.path().join("rstudio-cli")).unwrap();
        fs::write(
            xdg.path().join("rstudio-cli").join("config.toml"),
            "[mcp]\nvia = \"user\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve(None, false, proj.path(), Some(xdg.path())).unwrap(),
            Some("proj".to_string())
        );
    }

    #[test]
    fn resolve_nothing_is_local() {
        let proj = tmp();
        let xdg = tmp();
        assert_eq!(
            resolve(None, false, proj.path(), Some(xdg.path())).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_malformed_toml_errors() {
        let d = tmp();
        fs::write(d.path().join(PROJECT_CONFIG), "[mcp\nvia = ").unwrap();
        assert!(resolve(None, false, d.path(), None).is_err());
    }

    #[test]
    fn resolve_project_file_without_via_falls_through_to_xdg() {
        let proj = tmp();
        let xdg = tmp();
        fs::write(proj.path().join(PROJECT_CONFIG), "[other]\nkey = \"x\"\n").unwrap();
        fs::create_dir_all(xdg.path().join("rstudio-cli")).unwrap();
        fs::write(
            xdg.path().join("rstudio-cli").join("config.toml"),
            "[mcp]\nvia = \"user\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve(None, false, proj.path(), Some(xdg.path())).unwrap(),
            Some("user".to_string())
        );
    }
}
