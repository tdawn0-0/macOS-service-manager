//! The typed error the launchd port returns.
//!
//! Every user-facing message fragment from the Go package is preserved: the UI
//! and its tests depend on those strings.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::host::CommandError;

/// What kind of failure this is, for callers that branch (the UI offers helper
/// installation only for [`ErrorCode::HelperNotInstalled`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Io,
    Plist,
    NotFound,
    InvalidItem,
    Validation,
    Command,
    HelperNotInstalled,
    Helper,
    SavedNotLoaded,
    UnknownAction,
}

/// A launchd failure carrying a user-facing message and, where relevant, the
/// path of a file that was saved before loading failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchdError {
    pub code: ErrorCode,
    pub message: String,
    pub saved_path: Option<PathBuf>,
}

impl LaunchdError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        LaunchdError {
            code,
            message: message.into(),
            saved_path: None,
        }
    }

    pub fn io(message: impl Into<String>) -> Self {
        LaunchdError::new(ErrorCode::Io, message)
    }

    pub fn plist(message: impl Into<String>) -> Self {
        LaunchdError::new(ErrorCode::Plist, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        LaunchdError::new(ErrorCode::NotFound, message)
    }

    pub fn invalid_item(message: impl Into<String>) -> Self {
        LaunchdError::new(ErrorCode::InvalidItem, message)
    }

    pub fn validation(message: impl Into<String>) -> Self {
        LaunchdError::new(ErrorCode::Validation, message)
    }

    pub fn command(message: impl Into<String>) -> Self {
        LaunchdError::new(ErrorCode::Command, message)
    }

    pub fn helper(message: impl Into<String>) -> Self {
        LaunchdError::new(ErrorCode::Helper, message)
    }

    pub fn helper_not_installed() -> Self {
        LaunchdError::new(
            ErrorCode::HelperNotInstalled,
            "the privileged helper is not installed",
        )
    }

    pub fn unknown_action(action: impl std::fmt::Display) -> Self {
        LaunchdError::new(
            ErrorCode::UnknownAction,
            format!("launchd: unknown action \"{action}\""),
        )
    }

    /// Attach the path of a file that was saved before loading failed.
    pub fn with_saved_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.saved_path = Some(path.into());
        self
    }
}

impl std::fmt::Display for LaunchdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LaunchdError {}

impl From<CommandError> for LaunchdError {
    fn from(error: CommandError) -> Self {
        LaunchdError::command(error.message())
    }
}
