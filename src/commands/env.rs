use clap::Subcommand;
use regex_lite::Regex;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "env",
        name: "list",
        summary: "List variables in the active R environment (live).",
        description: "Wraps the get_environment_state RPC. For each variable returns \
                      its name, R type, class, length, size in bytes, and condensed \
                      description. Optional name regex filter applied CLI-side.",
        params: &[ParamSpec {
            name: "--pattern",
            kind: ParamKind::String,
            required: false,
            default: None,
            allowed: &[],
            description: "Regex applied to the variable name (CLI-side filter).",
        }],
        examples: &[
            ExampleSpec {
                cmd: "rstudio env list",
                explanation: "All variables in the active environment.",
            },
            ExampleSpec {
                cmd: "rstudio env list --pattern '^df_'",
                explanation: "Only variables whose name starts with df_.",
            },
        ],
        returns: "{vars: [{name, type, class, length, size_bytes, description, is_data}]}",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "--pattern is not a valid regex.",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("get_environment_state"),
    },
    ActionSpec {
        category: "env",
        name: "contents",
        summary: "Return the formatted contents of an object (live str/head).",
        description: "Wraps the get_object_contents RPC. Returns RStudio's own rendering \
                      (same lines as the Environment pane).",
        params: &[ParamSpec {
            name: "name",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Variable name in the active environment.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio env contents mtcars",
            explanation: "Returns the formatted str() of mtcars.",
        }],
        returns: "{contents: [string]}",
        errors: &[ErrorSpec {
            kind: "rpc_error",
            when: "Variable not present in the active environment.",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("get_object_contents"),
    },
    ActionSpec {
        category: "env",
        name: "info",
        summary: "Concise metadata for a variable (class, typeof, length, dim, size).",
        description: "Wrapped execute_r_code. Returns the key metadata without loading \
                      the value — useful to quickly check the nature of an object.",
        params: &[ParamSpec {
            name: "name",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Variable name in the active environment.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio env info mtcars",
            explanation: "Returns {class: 'data.frame', typeof: 'list', length: 11, dim: [32, 11], size_bytes: ...}.",
        }],
        returns: "{name, class: [string], typeof: string, length: int, dim: [int]|null, size_bytes: int}",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "Name not found (object 'X' not found).",
        }],
        rstudioapi_fn: None,
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum EnvCmd {
    /// List variables in the active environment (live).
    List {
        /// Regex applied to the variable name (CLI-side filter).
        #[arg(long)]
        pattern: Option<String>,
    },
    /// Formatted contents of an object (same lines as the Environment pane).
    Contents { name: String },
    /// Concise metadata (class/typeof/length/dim/size).
    Info { name: String },
}

pub fn run(cmd: &EnvCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        EnvCmd::List { pattern } => list(rpc, pattern.as_deref()),
        EnvCmd::Contents { name } => contents(rpc, name),
        EnvCmd::Info { name } => info(rpc, name),
    }
}

fn list(rpc: &RpcClient<'_>, pattern: Option<&str>) -> Result<Option<Value>, CliError> {
    let regex = match pattern {
        Some(p) => Some(
            Regex::new(p).map_err(|e| CliError::user(format!("invalid --pattern regex: {e}")))?,
        ),
        None => None,
    };

    let raw = rpc.rpc("get_environment_state", vec![])?;
    let env_list = raw
        .get("environment_list")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut vars: Vec<Value> = env_list
        .into_iter()
        .filter_map(|v| {
            let name = v.get("name")?.as_str()?.to_string();
            if let Some(r) = &regex
                && !r.is_match(&name)
            {
                return None;
            }
            let type_ = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
            let length = v.get("length").and_then(|x| x.as_u64());
            let size = v.get("size").and_then(|x| x.as_f64()).map(|s| s as u64);
            let description = v.get("description").and_then(|x| x.as_str()).unwrap_or("");
            let class = v.get("clazz").cloned().unwrap_or(Value::Null);
            let is_data = v.get("is_data").and_then(|x| x.as_bool()).unwrap_or(false);
            Some(json!({
                "name": name,
                "type": type_,
                "class": class,
                "length": length,
                "size_bytes": size,
                "description": description.trim(),
                "is_data": is_data,
            }))
        })
        .collect();

    vars.sort_by(|a, b| {
        let na = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let nb = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        na.cmp(nb)
    });

    Ok(Some(json!({ "vars": vars })))
}

fn contents(rpc: &RpcClient<'_>, name: &str) -> Result<Option<Value>, CliError> {
    let raw = rpc.rpc("get_object_contents", vec![Value::String(name.to_string())])?;
    let lines = raw
        .get("contents")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    Ok(Some(json!({
        "name": name,
        "contents": lines,
    })))
}

fn info(rpc: &RpcClient<'_>, name: &str) -> Result<Option<Value>, CliError> {
    let r_code = format!(
        r#"local({{
  .x <- get({name_q})
  cat(jsonlite::toJSON(list(
    name = {name_q},
    class = as.character(class(.x)),
    typeof = typeof(.x),
    length = length(.x),
    dim = if (is.null(dim(.x))) NA else as.integer(dim(.x)),
    size_bytes = as.numeric(object.size(.x))
  ), auto_unbox = TRUE, na = "null", null = "null"))
}})"#,
        name_q = r_quote(name)
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("env info: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}
