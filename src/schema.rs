//! Static command catalog used by the `rstudio schema` drill-down.
//!
//! Each `ActionSpec` documents one CLI action with the shape that an LLM (or a
//! human reading JSON) needs to call it correctly: typed params, defaults,
//! examples, possible error kinds, return type. The registry is kept in this
//! module so it's the single source of truth; the `schema` subcommand just
//! filters / projects it at three levels (catalog, category, action).

use serde::Serialize;
use serde_json::{Value, json};

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
        name: "project",
        description: "Project lifecycle: create / init existing dir / clone from git / open / current.",
    },
    CategorySpec {
        name: "session",
        description: "Whole-session info and lifecycle (version, mode, restart, list).",
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
    CategorySpec {
        name: "meta",
        description: "Meta-CLI commands (version, status, tx) — not RPC-bound but documented here so agents discovering surface via `schema` can find them.",
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
        crate::commands::project::ACTIONS,
        crate::commands::session::ACTIONS,
        crate::commands::pref::ACTIONS,
        crate::commands::job::ACTIONS,
        crate::commands::ui::ACTIONS,
        crate::commands::observe::ACTIONS,
        crate::commands::policy_cmd::ACTIONS,
        crate::commands::meta::ACTIONS,
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

/// Outcome of a `browse` call. The caller knows from the args which
/// variant to expect, but we tag explicitly so a generic JSON serialiser
/// can include the level (useful for agent output and tests).
#[derive(Debug)]
pub enum BrowseResult {
    /// Level 0 with no search: just the categories (with action counts).
    Catalog(Value),
    /// Level 0 with a search regex: matching actions across all categories.
    Search(Value),
    /// Level 1: one category's actions (summary-level).
    Category(Value),
    /// Level 2: full ActionSpec for one action.
    Action(&'static ActionSpec, Value),
}

impl BrowseResult {
    pub fn into_value(self) -> Value {
        match self {
            Self::Catalog(v) | Self::Search(v) | Self::Category(v) | Self::Action(_, v) => v,
        }
    }
}

/// Shared drill-down used by both the CLI (`rstudio schema`) and the MCP
/// server (`tools_search`). Single source of truth for surface discovery.
///
/// Three levels driven by the args:
///
/// - `(None, None, None)` → catalog: just the categories (name, description,
///   action_count). Lean by design — agents call into level 1 after picking
///   a category, instead of paying ~2.5k tokens for the full action list.
/// - `(None, None, Some(re))` → search: every action whose `category`, `name`,
///   or `summary` matches the regex, returned as `{category, name, summary}`.
/// - `(Some(cat), None, _)` → category drilldown: actions in `cat` with
///   `{name, summary, param_count, examples_count}`.
/// - `(Some(cat), Some(act), _)` → full ActionSpec for the action.
///
/// `matcher` is the search predicate — accepts a regex-like closure so the
/// caller chooses its regex engine without dragging the dependency into
/// `schema.rs`. Pass `None` for no filter.
pub fn browse(
    category_filter: Option<&str>,
    action_filter: Option<&str>,
    matcher: Option<&dyn Fn(&str) -> bool>,
) -> Result<BrowseResult, BrowseError> {
    match (category_filter, action_filter) {
        (None, None) => {
            if let Some(m) = matcher {
                let entries: Vec<Value> = registry()
                    .into_iter()
                    .filter(|a| m(a.category) || m(a.name) || m(a.summary))
                    .map(|a| {
                        json!({
                            "category": a.category,
                            "name": a.name,
                            "summary": a.summary,
                        })
                    })
                    .collect();
                Ok(BrowseResult::Search(json!({ "actions": entries })))
            } else {
                use std::collections::HashMap;
                let mut counts: HashMap<&str, usize> = HashMap::new();
                for a in registry() {
                    *counts.entry(a.category).or_insert(0) += 1;
                }
                let cats: Vec<Value> = CATEGORIES
                    .iter()
                    .map(|c| {
                        json!({
                            "name": c.name,
                            "description": c.description,
                            "action_count": counts.get(c.name).copied().unwrap_or(0),
                        })
                    })
                    .collect();
                Ok(BrowseResult::Catalog(json!({ "categories": cats })))
            }
        }
        (Some(cat), None) => {
            let actions = actions_in(cat);
            if actions.is_empty() && category(cat).is_none() {
                return Err(BrowseError::UnknownCategory(cat.to_string()));
            }
            let summarised: Vec<Value> = actions
                .iter()
                .map(|a| {
                    json!({
                        "name": a.name,
                        "summary": a.summary,
                        "param_count": a.params.len(),
                        "examples_count": a.examples.len(),
                    })
                })
                .collect();
            Ok(BrowseResult::Category(json!({
                "category": cat,
                "description": category(cat).map(|c| c.description),
                "actions": summarised,
            })))
        }
        (Some(cat), Some(act)) => {
            let action = find(cat, act)
                .ok_or_else(|| BrowseError::UnknownAction(cat.to_string(), act.to_string()))?;
            Ok(BrowseResult::Action(action, serialize_action(action)))
        }
        (None, Some(_)) => Err(BrowseError::ActionWithoutCategory),
    }
}

#[derive(Debug)]
pub enum BrowseError {
    UnknownCategory(String),
    UnknownAction(String, String),
    ActionWithoutCategory,
}

impl std::fmt::Display for BrowseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCategory(c) => write!(
                f,
                "unknown category '{c}'. Run `rstudio schema` to list categories."
            ),
            Self::UnknownAction(c, a) => write!(
                f,
                "unknown action '{c} {a}'. Run `rstudio schema {c}` to list actions."
            ),
            Self::ActionWithoutCategory => {
                write!(f, "action filter requires a category filter")
            }
        }
    }
}

fn serialize_action(a: &ActionSpec) -> Value {
    #[derive(Serialize)]
    struct Out<'a> {
        category: &'a str,
        name: &'a str,
        summary: &'a str,
        description: &'a str,
        params: &'a [ParamSpec],
        examples: &'a [ExampleSpec],
        returns: &'a str,
        errors: &'a [ErrorSpec],
        rstudioapi_fn: Option<&'a str>,
        rpc_method: Option<&'a str>,
    }
    serde_json::to_value(Out {
        category: a.category,
        name: a.name,
        summary: a.summary,
        description: a.description,
        params: a.params,
        examples: a.examples,
        returns: a.returns,
        errors: a.errors,
        rstudioapi_fn: a.rstudioapi_fn,
        rpc_method: a.rpc_method,
    })
    .unwrap()
}
