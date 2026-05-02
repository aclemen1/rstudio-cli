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
            eprintln!("error ({:?}): {}", err.kind, err.message);
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
