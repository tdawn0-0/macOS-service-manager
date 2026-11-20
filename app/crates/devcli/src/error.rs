//! `CliError`: the two exit-code classes the Go CLI distinguishes.
//!
//! `UsageError` (flags, missing arguments, safety guards) exits 2. Every other
//! error exits 1. `--dev` maps both to 1.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    Usage(String),
    Operation(String),
}

impl CliError {
    pub fn usage(message: impl Into<String>) -> Self {
        CliError::Usage(message.into())
    }

    pub fn operation(message: impl Into<String>) -> Self {
        CliError::Operation(message.into())
    }

    pub fn message(&self) -> &str {
        match self {
            CliError::Usage(message) | CliError::Operation(message) => message,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            CliError::Operation(_) => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        CliError::Operation(error.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        CliError::Operation(error.to_string())
    }
}

impl From<devcore::state::StateError> for CliError {
    fn from(error: devcore::state::StateError) -> Self {
        CliError::Operation(error.to_string())
    }
}

impl From<devcore::launchd::LaunchdError> for CliError {
    fn from(error: devcore::launchd::LaunchdError) -> Self {
        CliError::Operation(error.to_string())
    }
}

impl From<devcore::software::SoftwareError> for CliError {
    fn from(error: devcore::software::SoftwareError) -> Self {
        CliError::Operation(error.to_string())
    }
}

impl From<devcore::databases::DatabaseError> for CliError {
    fn from(error: devcore::databases::DatabaseError) -> Self {
        CliError::Operation(error.to_string())
    }
}

impl From<devproxy::proxy::lifecycle::ProxyLifecycleError> for CliError {
    fn from(error: devproxy::proxy::lifecycle::ProxyLifecycleError) -> Self {
        CliError::Operation(error.to_string())
    }
}

impl From<devproxy::ca::CaError> for CliError {
    fn from(error: devproxy::ca::CaError) -> Self {
        CliError::Operation(error.to_string())
    }
}

pub type CliResult<T> = Result<T, CliError>;
