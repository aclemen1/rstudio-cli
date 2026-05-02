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
        description: "Manipulation de l'éditeur de code (ouverture, navigation, sélection, lecture).",
    },
    CategorySpec {
        name: "exec",
        description: "Exécution de code R dans la session active (silencieux ou visible).",
    },
    CategorySpec {
        name: "console",
        description: "Lecture de l'historique des commandes et du buffer console.",
    },
    CategorySpec {
        name: "term",
        description: "Manipulation du panneau Terminal RStudio (shells live, lecture du buffer).",
    },
];

/// Aggregated registry. Each module that owns actions exposes them as a
/// `pub const ACTIONS: &[ActionSpec]` and we just chain the slices here.
pub fn registry() -> Vec<&'static ActionSpec> {
    let mut out: Vec<&'static ActionSpec> = Vec::new();
    for slice in [
        crate::commands::editor::ACTIONS,
        crate::commands::exec::ACTIONS,
        crate::commands::console::ACTIONS,
        crate::commands::term::ACTIONS,
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
