use serde::Serialize;
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    UserError,
    RpcError,
    SessionUnavailable,
    RError,
    Timeout,
    Internal,
}

#[derive(Debug)]
pub struct CliError {
    pub kind: ErrorKind,
    pub code: i32,
    pub message: String,
    /// Optional structured extras merged into the `error` object of the
    /// JSON envelope (alongside `code`/`kind`/`message`). Used to attach
    /// partial output to an R error — e.g. the stdout/messages/warnings a
    /// `r send` captured before the code raised — so an agent debugging
    /// at a `Browse[n]>` prompt doesn't lose what ran before the failure.
    /// `None` for the vast majority of errors. Must be a JSON object when
    /// set (its keys are spread into `error`).
    pub details: Option<Value>,
}

impl CliError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: 1,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_code(kind: ErrorKind, code: i32, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
            details: None,
        }
    }

    /// Attach structured extras (a JSON object) to be merged into the
    /// `error` envelope. Chainable.
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn user(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::UserError, message)
    }

    pub fn rpc(code: i32, message: impl Into<String>) -> Self {
        Self::with_code(ErrorKind::RpcError, code, message)
    }

    pub fn session(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::SessionUnavailable, message)
    }

    pub fn r(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::RError, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Timeout, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CliError {}

impl From<anyhow::Error> for CliError {
    fn from(err: anyhow::Error) -> Self {
        if let Some(e) = err.downcast_ref::<CliError>() {
            return Self {
                kind: e.kind,
                code: e.code,
                message: e.message.clone(),
                details: e.details.clone(),
            };
        }
        Self::internal(format!("{err:#}"))
    }
}
