use clap::Subcommand;
use serde_json::Value;

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "pref",
        name: "read",
        summary: "Read a user preference (project-scoped or user-scoped).",
        description: "Wraps rstudioapi::readPreference(name, default). 'default' is \
                      whatever value the CLI returns when the preference doesn't exist; \
                      pass --default-json to override (parsed as JSON).",
        params: &[
            ParamSpec {
                name: "name",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Preference name.",
            },
            ParamSpec {
                name: "--default-json",
                kind: ParamKind::Json,
                required: false,
                default: Some("null"),
                allowed: &[],
                description: "Default value as JSON (e.g. 'null', '\"foo\"', '42', 'true').",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio pref read my.setting --default-json '\"fallback\"'",
            explanation: "Returns the value, or 'fallback' if unset.",
        }],
        returns: "{name: string, value: any}",
        errors: &[],
        rstudioapi_fn: Some("readPreference"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pref",
        name: "write",
        summary: "Write a user preference.",
        description: "Wraps rstudioapi::writePreference(name, value). The value is \
                      passed as JSON via --value-json (so any JSON-representable type \
                      can be stored: strings, numbers, booleans, lists, objects).",
        params: &[
            ParamSpec {
                name: "name",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Preference name.",
            },
            ParamSpec {
                name: "--value-json",
                kind: ParamKind::Json,
                required: true,
                default: None,
                allowed: &[],
                description: "Value as JSON (e.g. '\"foo\"', '42', '{\"key\":\"val\"}').",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio pref write my.setting --value-json '\"hello\"'",
            explanation: "Stores the string \"hello\" under my.setting.",
        }],
        returns: "{name: string, value: any}",
        errors: &[],
        rstudioapi_fn: Some("writePreference"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pref",
        name: "read-rstudio",
        summary: "Read a built-in RStudio preference.",
        description: "Wraps rstudioapi::readRStudioPreference(name, default). Built-in \
                      preferences are RStudio's own (see RStudio Tools > Global Options); \
                      use `pref read` for arbitrary user-defined settings.",
        params: &[
            ParamSpec {
                name: "name",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Built-in preference name.",
            },
            ParamSpec {
                name: "--default-json",
                kind: ParamKind::Json,
                required: false,
                default: Some("null"),
                allowed: &[],
                description: "Default value as JSON.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio pref read-rstudio rainbow_parentheses",
            explanation: "Returns RStudio's rainbow-parentheses setting.",
        }],
        returns: "{name: string, value: any}",
        errors: &[],
        rstudioapi_fn: Some("readRStudioPreference"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pref",
        name: "write-rstudio",
        summary: "Write a built-in RStudio preference.",
        description: "Wraps rstudioapi::writeRStudioPreference(name, value).",
        params: &[
            ParamSpec {
                name: "name",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Built-in preference name.",
            },
            ParamSpec {
                name: "--value-json",
                kind: ParamKind::Json,
                required: true,
                default: None,
                allowed: &[],
                description: "Value as JSON.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio pref write-rstudio rainbow_parentheses --value-json 'true'",
            explanation: "Enable rainbow parentheses.",
        }],
        returns: "{name: string, value: any}",
        errors: &[],
        rstudioapi_fn: Some("writeRStudioPreference"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pref",
        name: "get-persistent",
        summary: "Read an RStudio persistent value (key-value store).",
        description: "Wraps rstudioapi::getPersistentValue(name). Persistent values \
                      live in the RStudio session storage, separate from preferences.",
        params: &[ParamSpec {
            name: "name",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Key name.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio pref get-persistent my.cached.value",
            explanation: "Returns the stored value or null.",
        }],
        returns: "{name: string, value: any}",
        errors: &[],
        rstudioapi_fn: Some("getPersistentValue"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "pref",
        name: "set-persistent",
        summary: "Write an RStudio persistent value (key-value store).",
        description: "Wraps rstudioapi::setPersistentValue(name, value).",
        params: &[
            ParamSpec {
                name: "name",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Key name.",
            },
            ParamSpec {
                name: "--value-json",
                kind: ParamKind::Json,
                required: true,
                default: None,
                allowed: &[],
                description: "Value as JSON.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio pref set-persistent my.cached.value --value-json '\"abc\"'",
            explanation: "Store \"abc\" under my.cached.value.",
        }],
        returns: "{name: string, value: any}",
        errors: &[],
        rstudioapi_fn: Some("setPersistentValue"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum PrefCmd {
    /// Read a user preference.
    Read {
        name: String,
        /// Default value as JSON if the preference is unset (default: null).
        #[arg(long, default_value = "null")]
        default_json: String,
    },
    /// Write a user preference.
    Write {
        name: String,
        /// Value as JSON.
        #[arg(long)]
        value_json: String,
    },
    /// Read a built-in RStudio preference.
    ReadRstudio {
        name: String,
        #[arg(long, default_value = "null")]
        default_json: String,
    },
    /// Write a built-in RStudio preference.
    WriteRstudio {
        name: String,
        #[arg(long)]
        value_json: String,
    },
    /// Read an RStudio persistent value.
    GetPersistent { name: String },
    /// Write an RStudio persistent value.
    SetPersistent {
        name: String,
        #[arg(long)]
        value_json: String,
    },
}

pub fn run(cmd: &PrefCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        PrefCmd::Read { name, default_json } => read_pref(rpc, "readPreference", name, default_json),
        PrefCmd::Write { name, value_json } => write_pref(rpc, "writePreference", name, value_json),
        PrefCmd::ReadRstudio { name, default_json } => {
            read_pref(rpc, "readRStudioPreference", name, default_json)
        }
        PrefCmd::WriteRstudio { name, value_json } => {
            write_pref(rpc, "writeRStudioPreference", name, value_json)
        }
        PrefCmd::GetPersistent { name } => get_persistent(rpc, name),
        PrefCmd::SetPersistent { name, value_json } => set_persistent(rpc, name, value_json),
    }
}

fn read_pref(
    rpc: &RpcClient<'_>,
    api_fn: &str,
    name: &str,
    default_json: &str,
) -> Result<Option<Value>, CliError> {
    // Validate that --default-json is parseable JSON (CLI-side check).
    let _: Value = serde_json::from_str(default_json)
        .map_err(|e| CliError::user(format!("invalid --default-json: {e}")))?;
    let r_code = format!(
        r#"local({{
  .__d <- jsonlite::fromJSON({default_q}, simplifyVector = FALSE)
  .__v <- rstudioapi::{api_fn}({name_q}, default = .__d)
  cat(jsonlite::toJSON(list(name = {name_q}, value = .__v), auto_unbox = TRUE, null = "null"))
}})"#,
        api_fn = api_fn,
        name_q = r_quote(name),
        default_q = r_quote(default_json),
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("pref read ({api_fn}): invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

fn write_pref(
    rpc: &RpcClient<'_>,
    api_fn: &str,
    name: &str,
    value_json: &str,
) -> Result<Option<Value>, CliError> {
    let _: Value = serde_json::from_str(value_json)
        .map_err(|e| CliError::user(format!("invalid --value-json: {e}")))?;
    let r_code = format!(
        r#"local({{
  .__v <- jsonlite::fromJSON({value_q}, simplifyVector = FALSE)
  rstudioapi::{api_fn}({name_q}, value = .__v)
  cat(jsonlite::toJSON(list(name = {name_q}, value = .__v), auto_unbox = TRUE, null = "null"))
}})"#,
        api_fn = api_fn,
        name_q = r_quote(name),
        value_q = r_quote(value_json),
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("pref write ({api_fn}): invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

fn get_persistent(rpc: &RpcClient<'_>, name: &str) -> Result<Option<Value>, CliError> {
    let r_code = format!(
        r#"local({{
  .__v <- rstudioapi::getPersistentValue({name_q})
  cat(jsonlite::toJSON(list(name = {name_q}, value = .__v), auto_unbox = TRUE, null = "null"))
}})"#,
        name_q = r_quote(name),
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("pref get-persistent: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn set_persistent(
    rpc: &RpcClient<'_>,
    name: &str,
    value_json: &str,
) -> Result<Option<Value>, CliError> {
    let _: Value = serde_json::from_str(value_json)
        .map_err(|e| CliError::user(format!("invalid --value-json: {e}")))?;
    let r_code = format!(
        r#"local({{
  .__v <- jsonlite::fromJSON({value_q}, simplifyVector = FALSE)
  rstudioapi::setPersistentValue({name_q}, .__v)
  cat(jsonlite::toJSON(list(name = {name_q}, value = .__v), auto_unbox = TRUE, null = "null"))
}})"#,
        name_q = r_quote(name),
        value_q = r_quote(value_json),
    );
    let raw = r_eval::run(rpc, &r_code)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("pref set-persistent: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}
