use std::io::{IsTerminal, stderr, stdout};
use std::str::FromStr;

use serde_json::{Value, json};

use crate::error::CliError;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Json,
    Text,
}

impl FromStr for Format {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            other => Err(format!("unknown format: {other} (expected: json|text)")),
        }
    }
}

/// Outcome of a successful command dispatch.
///
/// `Wrapped` is the AI-native default contract: a JSON envelope
/// `{"ok": true, "result": ...}` used by every command that talks to
/// the rsession. Default format = JSON.
///
/// `Adaptive` ships both a structured value AND a custom text rendering.
/// `default_text` controls which is shown when `--format` isn't passed:
///   - `true` for meta-CLI commands (`version`, `skill show`,
///     `skill install`) where humans are the primary audience.
///   - `false` for action commands with a polished text rendering
///     (e.g. `status`) where agents are still the primary audience but
///     a human-friendly text mode is available on demand.
pub enum Reply {
    Wrapped(Option<Value>),
    Adaptive {
        value: Value,
        text: String,
        default_text: bool,
    },
}

pub fn print_reply(reply: Reply, format: Option<Format>) {
    match reply {
        Reply::Wrapped(value) => print_ok(value, format.unwrap_or(Format::Json)),
        Reply::Adaptive {
            value,
            text,
            default_text,
        } => {
            let resolved = format.unwrap_or(if default_text {
                Format::Text
            } else {
                Format::Json
            });
            match resolved {
                Format::Json => print_ok(Some(value), Format::Json),
                Format::Text => print!("{text}"),
            }
        }
    }
}

pub fn print_ok(result: Option<Value>, format: Format) {
    match format {
        Format::Json => {
            let envelope = match result {
                Some(value) => json!({"ok": true, "result": value}),
                None => json!({"ok": true}),
            };
            println!("{}", serde_json::to_string(&envelope).unwrap());
        }
        Format::Text => print_value_as_text(result.as_ref()),
    }
}

pub fn print_err(err: &CliError, format: Format) {
    match format {
        Format::Json => {
            let envelope = json!({
                "ok": false,
                "error": {
                    "code": err.code,
                    "kind": err.kind,
                    "message": err.message,
                }
            });
            println!("{}", serde_json::to_string(&envelope).unwrap());
        }
        Format::Text => {
            let mark = fail_mark(stderr().is_terminal());
            eprintln!("{mark} {}", err.message);
        }
    }
}

fn print_value_as_text(v: Option<&Value>) {
    match v {
        None => {}
        Some(Value::Null) => {}
        Some(Value::String(s)) => println!("{s}"),
        Some(Value::Object(map)) => {
            // Common fields that warrant raw printing.
            if let Some(Value::String(s)) = map.get("output") {
                println!("{s}");
                return;
            }
            for key in ["commands", "lines", "contents"] {
                if let Some(Value::Array(arr)) = map.get(key) {
                    for item in arr {
                        if let Value::String(s) = item {
                            println!("{s}");
                        } else {
                            println!("{}", serde_json::to_string(item).unwrap());
                        }
                    }
                    return;
                }
            }
            // Fallback: pretty JSON.
            println!(
                "{}",
                serde_json::to_string_pretty(&Value::Object(map.clone())).unwrap()
            );
        }
        Some(other) => println!("{}", serde_json::to_string_pretty(other).unwrap()),
    }
}

/// Green check mark in ANSI when the target stream is a TTY,
/// plain ASCII fallback otherwise.
pub fn ok_mark(tty: bool) -> &'static str {
    if tty { "\x1b[32m✓\x1b[0m" } else { "OK" }
}

/// Red cross mark in ANSI when the target stream is a TTY,
/// plain ASCII fallback otherwise.
pub fn fail_mark(tty: bool) -> &'static str {
    if tty { "\x1b[31m✗\x1b[0m" } else { "FAIL" }
}

pub fn stdout_is_tty() -> bool {
    stdout().is_terminal()
}
