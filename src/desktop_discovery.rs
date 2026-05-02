//! Auto-discover an RStudio Desktop `rsession` process and extract its
//! TCP port + launcher token (argv) and shared secret (environment) so the
//! CLI can talk to it without a documented configuration file.
//!
//! This is intentionally best-effort — the values live in a process's argv /
//! environ, not in any official contract — and is meant to be paired with
//! explicit `--port` / `--secret` overrides for cases where discovery fails
//! (more than one rsession running, restricted process inspection, etc.).
//!
//! Linux: reads `/proc/<pid>/cmdline` and `/proc/<pid>/environ`.
//! macOS:  shells out to `ps -axww -o pid=,user=,command=` for the cmdline
//!         and `ps -E -p <pid> -o command=` for the environment.

use std::collections::HashMap;
use std::fs;
use std::process::Command;

use crate::error::CliError;

#[derive(Debug, Clone)]
pub struct DesktopInfo {
    pub pid: u32,
    pub port: u16,
    pub launcher_token: String,
    pub shared_secret: String,
}

/// Find a single Desktop-mode rsession owned by the current user, parse its
/// argv and environment, and return the values the CLI needs.
pub fn discover() -> Result<DesktopInfo, CliError> {
    let pids = find_desktop_rsession_pids()?;
    match pids.len() {
        0 => Err(CliError::session(
            "no RStudio Desktop rsession found. Is RStudio Desktop running, \
             and was the user the one to launch it? Otherwise pass --mode desktop \
             together with --port and --secret.",
        )),
        1 => populate(pids[0]),
        n => Err(CliError::session(format!(
            "found {n} Desktop rsession processes — ambiguous. \
             Pass --port and --secret explicitly to pick one."
        ))),
    }
}

/// Fill in port / launcher_token / shared_secret for a known PID.
pub fn populate(pid: u32) -> Result<DesktopInfo, CliError> {
    let argv = read_argv(pid)?;
    let port_str = lookup_argv(&argv, "--www-port")
        .ok_or_else(|| CliError::session(format!("rsession pid {pid}: --www-port not in argv")))?;
    let port: u16 = port_str.parse().map_err(|e| {
        CliError::internal(format!(
            "rsession pid {pid}: invalid --www-port {port_str:?}: {e}"
        ))
    })?;
    let launcher_token = lookup_argv(&argv, "--launcher-token").ok_or_else(|| {
        CliError::session(format!("rsession pid {pid}: --launcher-token not in argv"))
    })?;
    let env = read_environ(pid)?;
    let shared_secret = env.get("RS_SHARED_SECRET").cloned().ok_or_else(|| {
        CliError::session(format!(
            "rsession pid {pid}: RS_SHARED_SECRET not in environment (process inspection blocked?)"
        ))
    })?;
    Ok(DesktopInfo {
        pid,
        port,
        launcher_token,
        shared_secret,
    })
}

fn find_desktop_rsession_pids() -> Result<Vec<u32>, CliError> {
    let me = current_uid_string();
    // Use ps so we work the same on Linux and macOS. Filter for our uid +
    // rsession + --program-mode desktop.
    let output = Command::new("ps")
        .args(["-axww", "-o", "pid=,user=,command="])
        .output()
        .map_err(|e| CliError::session(format!("ps failed: {e}")))?;
    if !output.status.success() {
        return Err(CliError::session(format!(
            "ps exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut pids = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        let mut parts = trimmed.splitn(3, char::is_whitespace);
        let Some(pid_str) = parts.next() else {
            continue;
        };
        let Some(user) = parts.next() else { continue };
        let Some(command) = parts.next() else {
            continue;
        };
        if user != me {
            continue;
        }
        if !command.contains("rsession") {
            continue;
        }
        if !command.contains("--program-mode desktop") {
            continue;
        }
        if let Ok(pid) = pid_str.parse::<u32>() {
            pids.push(pid);
        }
    }
    Ok(pids)
}

fn current_uid_string() -> String {
    if let Ok(out) = Command::new("id").arg("-un").output()
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    std::env::var("USER").unwrap_or_default()
}

fn read_argv(pid: u32) -> Result<Vec<String>, CliError> {
    // Linux: /proc/<pid>/cmdline is NUL-separated argv. Try that first.
    let proc_path = format!("/proc/{pid}/cmdline");
    if let Ok(bytes) = fs::read(&proc_path) {
        return Ok(split_nul(&bytes));
    }
    // macOS fallback: `ps -p <pid> -o command=` returns a single string.
    // We split on whitespace, which is good enough for the flags we look at
    // (`--www-port`, `--launcher-token`, both followed by a single token).
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .map_err(|e| CliError::session(format!("ps -p {pid}: {e}")))?;
    if !output.status.success() {
        return Err(CliError::session(format!(
            "ps -p {pid} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if line.is_empty() {
        return Err(CliError::session(format!(
            "rsession pid {pid} has no command line"
        )));
    }
    Ok(line.split_whitespace().map(str::to_owned).collect())
}

fn read_environ(pid: u32) -> Result<HashMap<String, String>, CliError> {
    let proc_path = format!("/proc/{pid}/environ");
    if let Ok(bytes) = fs::read(&proc_path) {
        return Ok(parse_environ(&split_nul(&bytes)));
    }
    // macOS: `ps -E -p <pid> -o command=` appends env vars after the command.
    // Each is KEY=VALUE separated by whitespace; values themselves don't
    // typically contain whitespace for the variables we care about
    // (RS_SHARED_SECRET is a UUID-like token).
    let output = Command::new("ps")
        .args(["-E", "-p", &pid.to_string(), "-o", "command="])
        .output()
        .map_err(|e| CliError::session(format!("ps -E -p {pid}: {e}")))?;
    if !output.status.success() {
        return Err(CliError::session(format!(
            "ps -E -p {pid} failed (process inspection blocked?): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let line = String::from_utf8_lossy(&output.stdout);
    let tokens: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
    Ok(parse_environ(&tokens))
}

fn split_nul(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

fn parse_environ(tokens: &[String]) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for tok in tokens {
        if let Some((k, v)) = tok.split_once('=') {
            env.insert(k.to_string(), v.to_string());
        }
    }
    env
}

/// `--flag value` style lookup. Returns the token after the flag, if any.
fn lookup_argv(argv: &[String], flag: &str) -> Option<String> {
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_argv_space_separated() {
        let argv = vec![
            "rsession".to_string(),
            "--program-mode".to_string(),
            "desktop".to_string(),
            "--www-port".to_string(),
            "26258".to_string(),
        ];
        assert_eq!(lookup_argv(&argv, "--www-port"), Some("26258".to_string()));
        assert_eq!(
            lookup_argv(&argv, "--program-mode"),
            Some("desktop".to_string())
        );
        assert_eq!(lookup_argv(&argv, "--missing"), None);
    }

    #[test]
    fn lookup_argv_equals_form() {
        let argv = vec!["rsession".to_string(), "--www-port=26258".to_string()];
        assert_eq!(lookup_argv(&argv, "--www-port"), Some("26258".to_string()));
    }

    #[test]
    fn parse_environ_basic() {
        let toks = vec![
            "USER=foo".to_string(),
            "RS_SHARED_SECRET=2def-uuid".to_string(),
            "PATH=/usr/bin:/bin".to_string(),
        ];
        let env = parse_environ(&toks);
        assert_eq!(env.get("USER"), Some(&"foo".to_string()));
        assert_eq!(env.get("RS_SHARED_SECRET"), Some(&"2def-uuid".to_string()));
    }

    #[test]
    fn split_nul_handles_trailing_nul() {
        let bytes = b"a\0b\0c\0";
        let parts = split_nul(bytes);
        assert_eq!(parts, vec!["a", "b", "c"]);
    }
}
