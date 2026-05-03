use clap::Subcommand;
use serde_json::Value;

use crate::error::CliError;
use crate::policy::Policy;
use crate::schema::{ActionSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "policy",
        name: "show",
        summary: "Show the current policy (blocked commands).",
        description: "Reads ~/.config/rstudio-cli/policy.json and returns the list of \
                      blocked commands. Does not require a live session.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio policy show",
            explanation: "Returns {path, blocked: [\"session.restart\", ...]}.",
        }],
        returns: "{path: string, blocked: [string]}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: None,
    },
    ActionSpec {
        category: "policy",
        name: "block",
        summary: "Add a command to the blocked list.",
        description: "Appends the command to the blocked list in \
                      ~/.config/rstudio-cli/policy.json (creating the file if absent). \
                      The command is expressed as 'category.action', e.g. \
                      'session.restart' or 'r.exec'. Does not require a live session.",
        params: &[ParamSpec {
            name: "command",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Command to block in 'category.action' form.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio policy block session.restart",
            explanation: "Prevents any subsequent `rstudio session restart` call.",
        }],
        returns: "{path: string, blocked: [string]}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: None,
    },
    ActionSpec {
        category: "policy",
        name: "unblock",
        summary: "Remove a command from the blocked list.",
        description: "Removes the command from ~/.config/rstudio-cli/policy.json. \
                      No-ops if the command was not blocked. Does not require a live session.",
        params: &[ParamSpec {
            name: "command",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Command to unblock in 'category.action' form.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio policy unblock session.restart",
            explanation: "Lifts the block on session restart.",
        }],
        returns: "{path: string, blocked: [string]}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: None,
    },
];

#[derive(Subcommand, Debug)]
pub enum PolicyCmd {
    /// Show the current policy (blocked commands). No session required.
    Show,
    /// Block a command. No session required.
    Block {
        /// Command to block. Use a bare category (e.g. 'session') to block
        /// every action in that category, or a full key 'category.action'
        /// (e.g. 'session.restart') to block one specific action.
        command: String,
    },
    /// Unblock a command. No session required.
    Unblock {
        /// Command to remove from the blocked list. Same form as `block`:
        /// bare category or 'category.action'. No-op if not blocked.
        command: String,
    },
}

pub fn run(cmd: &PolicyCmd) -> Result<Option<Value>, CliError> {
    match cmd {
        PolicyCmd::Show => {
            let policy = Policy::load();
            Ok(Some(policy.to_value()))
        }
        PolicyCmd::Block { command } => {
            let mut policy = Policy::load();
            policy.add_blocked(command);
            policy.save()?;
            Ok(Some(policy.to_value()))
        }
        PolicyCmd::Unblock { command } => {
            let mut policy = Policy::load();
            policy.remove_blocked(command);
            policy.save()?;
            Ok(Some(policy.to_value()))
        }
    }
}
