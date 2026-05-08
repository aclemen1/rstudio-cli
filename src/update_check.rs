use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const TTL_SECS: u64 = 24 * 3600;
const GITHUB_URL: &str = "https://api.github.com/repos/aclemen1/rstudio-cli/releases/latest";

pub struct UpdateInfo {
    pub latest: String,
}

#[derive(Serialize, Deserialize)]
struct Cache {
    latest: Option<String>,
    checked_at: u64,
}

/// Check if a newer release is available. Returns immediately from the
/// on-disk cache; spawns a background thread to refresh the cache when
/// it has expired (TTL 24 h). Set `RSTUDIO_CLI_NO_UPDATE_CHECK=1` to
/// disable entirely.
pub fn check(current: &str) -> Option<UpdateInfo> {
    if std::env::var_os("RSTUDIO_CLI_NO_UPDATE_CHECK").is_some() {
        return None;
    }
    let path = cache_path()?;
    let cache = load_cache(&path);

    let is_fresh = cache
        .as_ref()
        .is_some_and(|c| now_secs().saturating_sub(c.checked_at) < TTL_SECS);

    if !is_fresh {
        let path_clone = path.clone();
        std::thread::spawn(move || refresh_cache(&path_clone));
    }

    cache?.latest.and_then(|v| {
        if is_newer(&v, current) {
            Some(UpdateInfo { latest: v })
        } else {
            None
        }
    })
}

fn refresh_cache(path: &Path) {
    let cache = Cache {
        latest: fetch_latest(),
        checked_at: now_secs(),
    };
    if let Ok(json) = serde_json::to_string(&cache) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

fn fetch_latest() -> Option<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "-m",
            "5",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: rstudio-cli",
            GITHUB_URL,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let body: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let tag = body.get("tag_name")?.as_str()?;
    Some(tag.trim_start_matches('v').to_string())
}

fn load_cache(path: &Path) -> Option<Cache> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn cache_path() -> Option<PathBuf> {
    let dir = dirs::cache_dir()?.join("rstudio-cli");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("update-check.json"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim_start_matches('v');
    let mut parts = s.splitn(3, '.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch: u32 = parts
        .next()
        .and_then(|p| p.split('-').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    Some((major, minor, patch))
}
