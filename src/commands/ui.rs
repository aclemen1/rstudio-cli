use clap::Subcommand;
use serde_json::Value;

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

// All actions in this category open a MODAL dialog in the user's RStudio
// browser. They block the R session until the user dismisses the dialog,
// which means subsequent `r exec` calls will wait. Use carefully — these are
// not appropriate for an autonomous agent that has no human in front of the
// browser to interact with the dialog.

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "ui",
        name: "dialog",
        summary: "Show a modal information dialog (BLOCKING).",
        description: "Wraps rstudioapi::showDialog(title, message, url). Blocks the R \
                      session until the user dismisses the dialog. Don't invoke this \
                      from an autonomous flow with no human in front of the screen.",
        params: &[
            ParamSpec {
                name: "title",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Dialog title.",
            },
            ParamSpec {
                name: "message",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Dialog body (HTML allowed).",
            },
            ParamSpec {
                name: "--url",
                kind: ParamKind::String,
                required: false,
                default: Some(""),
                allowed: &[],
                description: "Optional URL link shown in the dialog.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio ui dialog 'Heads up' 'Long task started.'",
            explanation: "Show an info dialog. Blocks until the user clicks OK.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("showDialog"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "ui",
        name: "update-dialog",
        summary: "Update an already-open dialog (BLOCKING context).",
        description: "Wraps rstudioapi::updateDialog(...). Pass arbitrary fields via \
                      --fields-json (object). Useful from inside a callback to mutate the \
                      currently-displayed dialog content without re-opening.",
        params: &[ParamSpec {
            name: "--fields-json",
            kind: ParamKind::Json,
            required: true,
            default: None,
            allowed: &[],
            description: "JSON object whose keys/values are forwarded as named arguments.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio ui update-dialog --fields-json '{\"message\":\"Almost done...\"}'",
            explanation: "Update the open dialog's message.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Invalid JSON.",
        }],
        rstudioapi_fn: Some("updateDialog"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "ui",
        name: "prompt",
        summary: "Show a modal text prompt and return what the user typed (BLOCKING).",
        description: "Wraps rstudioapi::showPrompt(title, message, default). Blocks until \
                      the user submits or cancels.",
        params: &[
            ParamSpec {
                name: "title",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Prompt title.",
            },
            ParamSpec {
                name: "message",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Prompt label.",
            },
            ParamSpec {
                name: "--default",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Pre-filled value.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio ui prompt 'Enter name' 'What is your name?'",
            explanation: "Show a prompt. Returns {value: '<typed>'} or {value: null} on cancel.",
        }],
        returns: "{value: string|null}",
        errors: &[],
        rstudioapi_fn: Some("showPrompt"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "ui",
        name: "question",
        summary: "Show a modal yes/no question (BLOCKING). Returns {answer: bool}.",
        description: "Wraps rstudioapi::showQuestion(title, message, ok, cancel). \
                      The 'ok' button corresponds to true, 'cancel' to false.",
        params: &[
            ParamSpec {
                name: "title",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Title.",
            },
            ParamSpec {
                name: "message",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Message.",
            },
            ParamSpec {
                name: "--ok",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Label for the OK button.",
            },
            ParamSpec {
                name: "--cancel",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Label for the Cancel button.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio ui question 'Confirm' 'Delete file?' --ok 'Delete' --cancel 'Keep'",
            explanation: "Returns {answer: true} if the user clicks Delete.",
        }],
        returns: "{answer: bool}",
        errors: &[],
        rstudioapi_fn: Some("showQuestion"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "ui",
        name: "select-file",
        summary: "Show a modal file picker (BLOCKING). Returns the selected path.",
        description: "Wraps rstudioapi::selectFile(caption, label, path, filter, existing).",
        params: &[
            ParamSpec {
                name: "--caption",
                kind: ParamKind::String,
                required: false,
                default: Some("Select File"),
                allowed: &[],
                description: "Dialog caption.",
            },
            ParamSpec {
                name: "--label",
                kind: ParamKind::String,
                required: false,
                default: Some("Select"),
                allowed: &[],
                description: "Confirm-button label.",
            },
            ParamSpec {
                name: "--path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Initial directory (defaults to the active project, else home).",
            },
            ParamSpec {
                name: "--filter",
                kind: ParamKind::String,
                required: false,
                default: Some("All Files (*)"),
                allowed: &[],
                description: "Filename filter expression.",
            },
            ParamSpec {
                name: "--new-file",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Allow non-existent files (existing=FALSE).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio ui select-file --caption 'Pick a CSV' --filter 'CSV (*.csv)'",
            explanation: "Show a file picker. Returns {path: '...'} or {path: null} on cancel.",
        }],
        returns: "{path: string|null}",
        errors: &[],
        rstudioapi_fn: Some("selectFile"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "ui",
        name: "select-dir",
        summary: "Show a modal directory picker (BLOCKING).",
        description: "Wraps rstudioapi::selectDirectory(caption, label, path).",
        params: &[
            ParamSpec {
                name: "--caption",
                kind: ParamKind::String,
                required: false,
                default: Some("Select Directory"),
                allowed: &[],
                description: "Dialog caption.",
            },
            ParamSpec {
                name: "--label",
                kind: ParamKind::String,
                required: false,
                default: Some("Select"),
                allowed: &[],
                description: "Confirm-button label.",
            },
            ParamSpec {
                name: "--path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Initial directory (defaults to the active project, else home).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio ui select-dir --caption 'Pick output folder'",
            explanation: "Returns {path: '...'} or {path: null}.",
        }],
        returns: "{path: string|null}",
        errors: &[],
        rstudioapi_fn: Some("selectDirectory"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "ui",
        name: "ask-password",
        summary: "Show a modal password prompt (BLOCKING).",
        description: "Wraps rstudioapi::askForPassword(prompt). The returned value is \
                      the typed password (use with care; appears in JSON output).",
        params: &[ParamSpec {
            name: "--prompt",
            kind: ParamKind::String,
            required: false,
            default: Some("Please enter your password"),
            allowed: &[],
            description: "Prompt text.",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio ui ask-password --prompt 'DB password'",
            explanation: "Returns {value: '<typed>'} or {value: null} on cancel.",
        }],
        returns: "{value: string|null}",
        errors: &[],
        rstudioapi_fn: Some("askForPassword"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "ui",
        name: "ask-secret",
        summary: "Show a modal secret prompt managed by RStudio's keyring (BLOCKING).",
        description: "Wraps rstudioapi::askForSecret(name, message, title). RStudio caches \
                      the secret in keyring after the first prompt; subsequent calls return \
                      the cached value silently.",
        params: &[
            ParamSpec {
                name: "name",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Secret name (key in the keyring).",
            },
            ParamSpec {
                name: "--message",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Prompt body.",
            },
            ParamSpec {
                name: "--title",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Dialog title.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio ui ask-secret api_key --message 'Your API key'",
            explanation: "Prompt once, then read from keyring. Returns {value: '...'}.",
        }],
        returns: "{value: string|null}",
        errors: &[],
        rstudioapi_fn: Some("askForSecret"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum UiCmd {
    /// Show a modal information dialog (BLOCKING).
    Dialog {
        title: String,
        message: String,
        #[arg(long, default_value = "")]
        url: String,
    },
    /// Update an already-open dialog.
    UpdateDialog {
        #[arg(long)]
        fields_json: String,
    },
    /// Show a modal text prompt (BLOCKING).
    Prompt {
        title: String,
        message: String,
        #[arg(long)]
        default: Option<String>,
    },
    /// Show a modal yes/no question (BLOCKING).
    Question {
        title: String,
        message: String,
        #[arg(long)]
        ok: Option<String>,
        #[arg(long)]
        cancel: Option<String>,
    },
    /// Show a modal file picker (BLOCKING).
    SelectFile {
        #[arg(long, default_value = "Select File")]
        caption: String,
        #[arg(long, default_value = "Select")]
        label: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value = "All Files (*)")]
        filter: String,
        /// Allow non-existent files (existing=FALSE).
        #[arg(long)]
        new_file: bool,
    },
    /// Show a modal directory picker (BLOCKING).
    SelectDir {
        #[arg(long, default_value = "Select Directory")]
        caption: String,
        #[arg(long, default_value = "Select")]
        label: String,
        #[arg(long)]
        path: Option<String>,
    },
    /// Show a modal password prompt (BLOCKING).
    AskPassword {
        #[arg(long, default_value = "Please enter your password")]
        prompt: String,
    },
    /// Show a modal secret prompt (BLOCKING; keyring-cached).
    AskSecret {
        name: String,
        #[arg(long)]
        message: Option<String>,
        #[arg(long)]
        title: Option<String>,
    },
}

pub fn run(cmd: &UiCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        UiCmd::Dialog { title, message, url } => dialog(rpc, title, message, url),
        UiCmd::UpdateDialog { fields_json } => update_dialog(rpc, fields_json),
        UiCmd::Prompt { title, message, default } => prompt(rpc, title, message, default.as_deref()),
        UiCmd::Question { title, message, ok, cancel } => {
            question(rpc, title, message, ok.as_deref(), cancel.as_deref())
        }
        UiCmd::SelectFile {
            caption,
            label,
            path,
            filter,
            new_file,
        } => select_file(rpc, caption, label, path.as_deref(), filter, *new_file),
        UiCmd::SelectDir { caption, label, path } => {
            select_dir(rpc, caption, label, path.as_deref())
        }
        UiCmd::AskPassword { prompt: prompt_text } => ask_password(rpc, prompt_text),
        UiCmd::AskSecret { name, message, title } => {
            ask_secret(rpc, name, message.as_deref(), title.as_deref())
        }
    }
}

fn dialog(
    rpc: &RpcClient<'_>,
    title: &str,
    message: &str,
    url: &str,
) -> Result<Option<Value>, CliError> {
    let r = format!(
        "rstudioapi::showDialog(title = {}, message = {}, url = {})",
        r_quote(title),
        r_quote(message),
        r_quote(url)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn update_dialog(rpc: &RpcClient<'_>, fields_json: &str) -> Result<Option<Value>, CliError> {
    let parsed: Value = serde_json::from_str(fields_json)
        .map_err(|e| CliError::user(format!("invalid --fields-json: {e}")))?;
    let obj = parsed
        .as_object()
        .ok_or_else(|| CliError::user("--fields-json must be a JSON object"))?;
    let mut args: Vec<String> = Vec::new();
    for (k, v) in obj {
        let v_str = serde_json::to_string(v).unwrap();
        args.push(format!(
            "{name} = jsonlite::fromJSON({json}, simplifyVector = FALSE)",
            name = k,
            json = r_quote(&v_str),
        ));
    }
    let r = format!("rstudioapi::updateDialog({})", args.join(", "));
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn prompt(
    rpc: &RpcClient<'_>,
    title: &str,
    message: &str,
    default: Option<&str>,
) -> Result<Option<Value>, CliError> {
    let default_arg = match default {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    let r = format!(
        r#"local({{
  .__v <- rstudioapi::showPrompt(title = {title_q}, message = {message_q}, default = {default_arg})
  if (is.null(.__v)) cat("{{\"value\":null}}")
  else cat(jsonlite::toJSON(list(value = .__v), auto_unbox = TRUE))
}})"#,
        title_q = r_quote(title),
        message_q = r_quote(message),
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("ui prompt: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn question(
    rpc: &RpcClient<'_>,
    title: &str,
    message: &str,
    ok: Option<&str>,
    cancel: Option<&str>,
) -> Result<Option<Value>, CliError> {
    let ok_arg = match ok {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    let cancel_arg = match cancel {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    let r = format!(
        r#"local({{
  .__a <- rstudioapi::showQuestion(title = {title_q}, message = {message_q}, ok = {ok_arg}, cancel = {cancel_arg})
  cat(jsonlite::toJSON(list(answer = .__a), auto_unbox = TRUE))
}})"#,
        title_q = r_quote(title),
        message_q = r_quote(message),
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("ui question: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn select_file(
    rpc: &RpcClient<'_>,
    caption: &str,
    label: &str,
    path: Option<&str>,
    filter: &str,
    new_file: bool,
) -> Result<Option<Value>, CliError> {
    let path_arg = match path {
        Some(s) => r_quote(s),
        None => "rstudioapi::getActiveProject()".into(),
    };
    let existing_arg = if new_file { "FALSE" } else { "TRUE" };
    let r = format!(
        r#"local({{
  .__p <- rstudioapi::selectFile(caption = {caption_q}, label = {label_q}, path = {path_arg}, filter = {filter_q}, existing = {existing_arg})
  if (is.null(.__p)) cat("{{\"path\":null}}")
  else cat(jsonlite::toJSON(list(path = .__p), auto_unbox = TRUE))
}})"#,
        caption_q = r_quote(caption),
        label_q = r_quote(label),
        filter_q = r_quote(filter),
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("ui select-file: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn select_dir(
    rpc: &RpcClient<'_>,
    caption: &str,
    label: &str,
    path: Option<&str>,
) -> Result<Option<Value>, CliError> {
    let path_arg = match path {
        Some(s) => r_quote(s),
        None => "rstudioapi::getActiveProject()".into(),
    };
    let r = format!(
        r#"local({{
  .__p <- rstudioapi::selectDirectory(caption = {caption_q}, label = {label_q}, path = {path_arg})
  if (is.null(.__p)) cat("{{\"path\":null}}")
  else cat(jsonlite::toJSON(list(path = .__p), auto_unbox = TRUE))
}})"#,
        caption_q = r_quote(caption),
        label_q = r_quote(label),
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("ui select-dir: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn ask_password(rpc: &RpcClient<'_>, prompt: &str) -> Result<Option<Value>, CliError> {
    let r = format!(
        r#"local({{
  .__v <- rstudioapi::askForPassword(prompt = {prompt_q})
  if (is.null(.__v)) cat("{{\"value\":null}}")
  else cat(jsonlite::toJSON(list(value = .__v), auto_unbox = TRUE))
}})"#,
        prompt_q = r_quote(prompt),
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("ui ask-password: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn ask_secret(
    rpc: &RpcClient<'_>,
    name: &str,
    message: Option<&str>,
    title: Option<&str>,
) -> Result<Option<Value>, CliError> {
    let message_arg = match message {
        Some(s) => r_quote(s),
        None => format!("paste({}, ':', sep = '')", r_quote(name)),
    };
    let title_arg = match title {
        Some(s) => r_quote(s),
        None => format!("paste({}, 'Secret')", r_quote(name)),
    };
    let r = format!(
        r#"local({{
  .__v <- rstudioapi::askForSecret(name = {name_q}, message = {message_arg}, title = {title_arg})
  if (is.null(.__v)) cat("{{\"value\":null}}")
  else cat(jsonlite::toJSON(list(value = .__v), auto_unbox = TRUE))
}})"#,
        name_q = r_quote(name),
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("ui ask-secret: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}
