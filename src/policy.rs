use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CliError;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub blocked: Vec<String>,
}

impl Policy {
    pub fn load() -> Self {
        policy_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Check `full_key` (e.g. `"session.restart"`) against the blocked list.
    /// A rule matches the full key OR the bare category (e.g. `"session"` blocks
    /// all session commands).
    pub fn check(&self, full_key: &str) -> Result<(), CliError> {
        let category = full_key.split('.').next().unwrap_or(full_key);
        if let Some(rule) = self
            .blocked
            .iter()
            .find(|b| b.as_str() == full_key || b.as_str() == category)
        {
            return Err(CliError::user(format!(
                "policy: '{}' is blocked by {}",
                rule,
                policy_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.config/rstudio-cli/policy.json".into())
            )));
        }
        Ok(())
    }

    pub fn add_blocked(&mut self, command: &str) {
        let cmd = command.to_string();
        if !self.blocked.contains(&cmd) {
            self.blocked.push(cmd);
        }
    }

    pub fn remove_blocked(&mut self, command: &str) -> bool {
        let before = self.blocked.len();
        self.blocked.retain(|b| b != command);
        self.blocked.len() < before
    }

    pub fn save(&self) -> Result<(), CliError> {
        let path = policy_path().ok_or_else(|| {
            CliError::internal("cannot determine policy file path (HOME not set?)")
        })?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CliError::internal(format!("policy: create dir: {e}")))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| CliError::internal(format!("policy: serialise: {e}")))?;
        fs::write(&path, json)
            .map_err(|e| CliError::internal(format!("policy: write {}: {e}", path.display())))?;
        Ok(())
    }

    pub fn to_value(&self) -> serde_json::Value {
        let path = policy_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        json!({ "path": path, "blocked": self.blocked })
    }
}

fn policy_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("rstudio-cli").join("policy.json"))
}
