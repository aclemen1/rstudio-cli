use serde::Serialize;
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
}

impl CliError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: 1,
            message: message.into(),
        }
    }

    pub fn with_code(kind: ErrorKind, code: i32, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
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
            };
        }
        Self::internal(format!("{err:#}"))
    }
}
