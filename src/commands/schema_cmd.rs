use clap::Args;
use regex_lite::Regex;
use serde_json::Value;

use crate::error::CliError;
use crate::schema::{BrowseError, browse};

#[derive(Args, Debug)]
pub struct SchemaCmd {
    /// Category (level 1) or action (level 2 when followed by an action arg).
    pub category: Option<String>,
    /// Action name within the category (level 2).
    pub action: Option<String>,
    /// Regex filter (level 0 only) applied to category/name/summary.
    /// When present without a category, the CLI returns the matching
    /// actions across all categories — the equivalent of the legacy
    /// flat level-0 listing.
    #[arg(long)]
    pub search: Option<String>,
}

pub fn run(cmd: &SchemaCmd) -> Result<Option<Value>, CliError> {
    let regex = match &cmd.search {
        Some(pat) => Some(
            Regex::new(pat).map_err(|e| CliError::user(format!("invalid --search regex: {e}")))?,
        ),
        None => None,
    };
    let matcher = regex.as_ref().map(|r| {
        let r = r.clone();
        move |s: &str| r.is_match(s)
    });
    // Type-erase the closure through a trait object so it matches the
    // `&dyn Fn(&str) -> bool` parameter of `browse`.
    let matcher_ref: Option<&dyn Fn(&str) -> bool> =
        matcher.as_ref().map(|m| m as &dyn Fn(&str) -> bool);

    let result = browse(cmd.category.as_deref(), cmd.action.as_deref(), matcher_ref).map_err(
        |e| match e {
            BrowseError::UnknownCategory(_) | BrowseError::UnknownAction(_, _) => {
                CliError::user(e.to_string())
            }
            BrowseError::ActionWithoutCategory => CliError::user(e.to_string()),
        },
    )?;
    Ok(Some(result.into_value()))
}
