//! Typed failures for the persistence layer.
//!
//! User-facing message fragments from the Go package are preserved: the UI and
//! its tests match on them, and the schema-version message is part of the
//! "existing files were left unchanged" contract.

use std::io;

/// Another process holds the exclusive lease for a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageBusyError {
    pub package_id: String,
}

impl std::fmt::Display for PackageBusyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "package {} is in use by another operation or CLI command; wait for it to finish",
            self.package_id
        )
    }
}

impl std::error::Error for PackageBusyError {}

/// Another operation reserved a `(kind, id)` resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceBusyError {
    pub kind: String,
    pub id: String,
}

impl std::fmt::Display for ResourceBusyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} is reserved by another operation; finish or cancel it in its caller and retry",
            self.kind, self.id
        )
    }
}

impl std::error::Error for ResourceBusyError {}

/// Everything the state store, model validators and leases can fail with.
#[derive(Debug)]
pub enum StateError {
    Io(io::Error),
    Json(serde_json::Error),
    ReadJson(serde_json::Error),
    Invalid(String),
    UnsupportedSchema { found: i64, expected: i64 },
    Cancelled,
    PackageBusy(PackageBusyError),
    ResourceBusy(ResourceBusyError),
}

impl StateError {
    pub fn invalid(message: impl Into<String>) -> Self {
        StateError::Invalid(message.into())
    }

    /// Whether this is the "another operation owns it" case the UI retries.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            StateError::PackageBusy(_) | StateError::ResourceBusy(_)
        )
    }
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Io(error) => error.fmt(f),
            StateError::Json(error) => error.fmt(f),
            StateError::ReadJson(error) => write!(f, "read state: {error}"),
            StateError::Invalid(message) => f.write_str(message),
            StateError::UnsupportedSchema { found, expected } => write!(
                f,
                "saved settings use unsupported format {found} (expected {expected}); existing files were left unchanged"
            ),
            StateError::Cancelled => f.write_str("context canceled"),
            StateError::PackageBusy(error) => error.fmt(f),
            StateError::ResourceBusy(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateError::Io(error) => Some(error),
            StateError::Json(error) | StateError::ReadJson(error) => Some(error),
            StateError::PackageBusy(error) => Some(error),
            StateError::ResourceBusy(error) => Some(error),
            StateError::Invalid(_)
            | StateError::UnsupportedSchema { .. }
            | StateError::Cancelled => None,
        }
    }
}

impl From<io::Error> for StateError {
    fn from(error: io::Error) -> Self {
        StateError::Io(error)
    }
}

impl From<serde_json::Error> for StateError {
    fn from(error: serde_json::Error) -> Self {
        StateError::Json(error)
    }
}
