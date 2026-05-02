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

pub fn print_ok(result: Option<Value>, format: Format) {
    match format {
        Format::Json => {
            let envelope = match result {
                Some(value) => json!({"ok": true, "result": value}),
                None => json!({"ok": true}),
            };
            println!("{}", serde_json::to_string(&envelope).unwrap());
        }
        Format::Text => match result {
            Some(Value::String(s)) => println!("{s}"),
            Some(other) => println!("{}", serde_json::to_string_pretty(&other).unwrap()),
            None => {}
        },
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
            eprintln!("error ({:?}): {}", err.kind, err.message);
        }
    }
}
