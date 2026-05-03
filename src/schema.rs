//! Static command catalog used by the `rstudio schema` drill-down.
//!
//! Each `ActionSpec` documents one CLI action with the shape that an LLM (or a
//! human reading JSON) needs to call it correctly: typed params, defaults,
//! examples, possible error kinds, return type. The registry is kept in this
//! module so it's the single source of truth; the `schema` subcommand just
//! filters / projects it at three levels (catalog, category, action).

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    String,
    Integer,
    Number,
    Bool,
    /// JSON value (object or array) passed as a string in the CLI.
    Json,
    /// Constrained string with a fixed set of allowed values.
    Enum,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ParamSpec {
    /// Either a positional name (`path`) or a long flag (`--line`).
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    /// Default value as it would appear in the CLI (or `None` when there's no default).
    pub default: Option<&'static str>,
    /// Allowed values for `ParamKind::Enum`; ignored otherwise.
    pub allowed: &'static [&'static str],
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ExampleSpec {
    pub cmd: &'static str,
    pub explanation: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ErrorSpec {
    pub kind: &'static str,
    pub when: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ActionSpec {
    pub category: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamSpec],
    pub examples: &'static [ExampleSpec],
    pub returns: &'static str,
    pub errors: &'static [ErrorSpec],
    /// Name of the corresponding `rstudioapi` function if the action wraps
    /// one (e.g. `documentOpen`, `terminalBuffer`). `None` for actions that
    /// don't have a direct rstudioapi analog (CLI-internal helpers, raw RPC
    /// calls without a public wrapper, etc.).
    pub rstudioapi_fn: Option<&'static str>,
    /// JSON-RPC method invoked on the rsession Unix socket. Postbacks are
    /// noted as `"postback:<cmd>"`. `None` for actions that don't hit the
    /// socket at all (CLI-internal: `skill show`, `skill install`,
    /// `console actions` which just reads a disk file, etc.).
    pub rpc_method: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CategorySpec {
    pub name: &'static str,
    pub description: &'static str,
}

/// Categories declared in display order. A category not listed here is still
/// usable (its actions show up under the category name from the action), but
/// without a description.
pub const CATEGORIES: &[CategorySpec] = &[
    CategorySpec {
        name: "editor",
        description: "Code editor manipulation (open, navigate, read, context, insert, select).",
    },
    CategorySpec {
        name: "r",
        description: "Run R code in the active session (silent via execute_r_code, or visible via console_input).",
    },
    CategorySpec {
        name: "console",
        description: "R console history, on-disk buffer snapshot, and live console editor context.",
    },
    CategorySpec {
        name: "term",
        description: "RStudio Terminal pane (live shells, buffer reads, send/exec, lifecycle).",
    },
    CategorySpec {
        name: "env",
        description: "Inspect the active R environment (variables, contents, metadata).",
    },
    CategorySpec {
        name: "pane",
        description: "Non-editor panes: Viewer (HTML), Files (navigation), Markers (lint-style feedback).",
    },
    CategorySpec {
        name: "skill",
        description: "Manage the embedded Claude Code skill (show / install).",
    },
    CategorySpec {
        name: "session",
        description: "Whole-session info and lifecycle (version, mode, project, restart).",
    },
    CategorySpec {
        name: "pref",
        description: "User and built-in RStudio preferences + persistent key/value store.",
    },
    CategorySpec {
        name: "job",
        description: "Background jobs in the Jobs pane (create, drive, run R scripts).",
    },
    CategorySpec {
        name: "ui",
        description: "Modal UI prompts (dialog, prompt, question, file/dir picker, secret). All BLOCKING.",
    },
    CategorySpec {
        name: "observe",
        description: "Stream session-state changes as JSON Lines on stdout (polling-based, R-free).",
    },
    CategorySpec {
        name: "policy",
        description: "Security policy: block / unblock commands by category or action. No session required.",
    },
];

/// Aggregated registry. Each module that owns actions exposes them as a
/// `pub const ACTIONS: &[ActionSpec]` and we just chain the slices here.
pub fn registry() -> Vec<&'static ActionSpec> {
    let mut out: Vec<&'static ActionSpec> = Vec::new();
    for slice in [
        crate::commands::editor::ACTIONS,
        crate::commands::r::ACTIONS,
        crate::commands::console::ACTIONS,
        crate::commands::term::ACTIONS,
        crate::commands::env::ACTIONS,
        crate::commands::pane::ACTIONS,
        crate::commands::skill::ACTIONS,
        crate::commands::session::ACTIONS,
        crate::commands::pref::ACTIONS,
        crate::commands::job::ACTIONS,
        crate::commands::ui::ACTIONS,
        crate::commands::observe::ACTIONS,
        crate::commands::policy_cmd::ACTIONS,
    ] {
        out.extend(slice.iter());
    }
    out
}

pub fn category(name: &str) -> Option<&'static CategorySpec> {
    CATEGORIES.iter().find(|c| c.name == name)
}

pub fn actions_in(category: &str) -> Vec<&'static ActionSpec> {
    registry()
        .into_iter()
        .filter(|a| a.category == category)
        .collect()
}

pub fn find(category: &str, name: &str) -> Option<&'static ActionSpec> {
    registry()
        .into_iter()
        .find(|a| a.category == category && a.name == name)
}
