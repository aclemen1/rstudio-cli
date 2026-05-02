use clap::Args;
use regex_lite::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::schema::{ActionSpec, CATEGORIES, CategorySpec, actions_in, category, find, registry};

#[derive(Args, Debug)]
pub struct SchemaCmd {
    /// Category (level 1) or action (level 2 when followed by an action arg).
    pub category: Option<String>,
    /// Action name within the category (level 2).
    pub action: Option<String>,
    /// Regex filter (level 0 only) applied to category/name/summary.
    #[arg(long)]
    pub search: Option<String>,
}

pub fn run(cmd: &SchemaCmd) -> Result<Option<Value>, CliError> {
    match (&cmd.category, &cmd.action) {
        (None, _) => level0(cmd.search.as_deref()),
        (Some(cat), None) => level1(cat),
        (Some(cat), Some(act)) => level2(cat, act),
    }
}

fn level0(search: Option<&str>) -> Result<Option<Value>, CliError> {
    let regex = match search {
        Some(pat) => Some(
            Regex::new(pat).map_err(|e| CliError::user(format!("invalid --search regex: {e}")))?,
        ),
        None => None,
    };
    let entries: Vec<Value> = registry()
        .into_iter()
        .filter(|a| match &regex {
            None => true,
            Some(r) => r.is_match(a.category) || r.is_match(a.name) || r.is_match(a.summary),
        })
        .map(|a| {
            json!({
                "category": a.category,
                "name": a.name,
                "summary": a.summary,
            })
        })
        .collect();
    let categories: Vec<&CategorySpec> = CATEGORIES.iter().collect();
    Ok(Some(json!({
        "categories": categories,
        "actions": entries,
    })))
}

fn level1(cat: &str) -> Result<Option<Value>, CliError> {
    let actions = actions_in(cat);
    if actions.is_empty() && category(cat).is_none() {
        return Err(CliError::user(format!(
            "unknown category '{cat}'. Run `rstudio schema` to list categories."
        )));
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
    Ok(Some(json!({
        "category": cat,
        "description": category(cat).map(|c| c.description),
        "actions": summarised,
    })))
}

fn level2(cat: &str, act: &str) -> Result<Option<Value>, CliError> {
    let action = find(cat, act).ok_or_else(|| {
        CliError::user(format!(
            "unknown action '{cat} {act}'. Run `rstudio schema {cat}` to list actions."
        ))
    })?;
    Ok(Some(serialize_action(action)))
}

fn serialize_action(a: &ActionSpec) -> Value {
    #[derive(Serialize)]
    struct Out<'a> {
        category: &'a str,
        name: &'a str,
        summary: &'a str,
        description: &'a str,
        params: &'a [crate::schema::ParamSpec],
        examples: &'a [crate::schema::ExampleSpec],
        returns: &'a str,
        errors: &'a [crate::schema::ErrorSpec],
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
