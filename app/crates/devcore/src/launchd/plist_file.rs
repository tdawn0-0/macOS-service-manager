//! The part of a launchd property list the app reads.
//!
//! Decoding is strict for the keys it knows, matching howett's decoding into
//! concrete Go types: a recognized key of the wrong type is a parse error.
//! Unknown keys with arbitrary values are ignored here and preserved by
//! [`super::spec::Spec::marshal`].

use std::io::Cursor;
use std::path::Path;

use plist::{Dictionary, Value};

use super::error::LaunchdError;

/// The part of a property list the app reads.
#[derive(Debug, Clone, Default)]
pub(crate) struct FileModel {
    pub label: String,
    pub disabled: Option<bool>,
    pub program: Option<String>,
    /// Presence is distinguished from an absent array.
    pub program_arguments: Option<Vec<String>>,
    pub run_at_load: Option<bool>,
    pub keep_alive: Option<Value>,
    pub start_interval: Option<i64>,
    pub start_calendar_interval: Option<Value>,
    pub working_directory: Option<String>,
    pub standard_out_path: Option<String>,
    pub standard_error_path: Option<String>,
    pub environment: std::collections::BTreeMap<String, String>,
    pub start_on_mount: Option<bool>,
    pub queue_directories: Vec<String>,
    pub user_name: Option<String>,
    pub group_name: Option<String>,
    pub root_directory: Option<String>,
}

impl FileModel {
    /// Whether the job runs at times of its own.
    pub fn scheduled(&self) -> bool {
        self.start_interval.is_some() || self.start_calendar_interval.is_some()
    }

    /// Whether launchd may start the job again when its process ends.
    /// Conditions count too: only unloading the job is sure to stop it.
    pub fn keep_alive(&self) -> bool {
        match &self.keep_alive {
            None => false,
            Some(Value::Boolean(on)) => *on,
            Some(_) => true,
        }
    }

    /// Whether launchd keeps the job from loading: the domain's override first,
    /// then the file.
    pub fn disabled_in(&self, state: &super::parse::DomainState) -> bool {
        if let Some(off) = state.disabled.get(&self.label) {
            return *off;
        }
        self.disabled == Some(true)
    }

    /// Says in words when the job runs, once loaded.
    pub fn runs(&self) -> String {
        let mut when: Vec<String> = Vec::new();
        if self.run_at_load == Some(true) {
            when.push("when loaded".to_string());
        }
        match &self.keep_alive {
            Some(Value::Boolean(true)) => when.push("kept alive".to_string()),
            Some(Value::Boolean(false)) | None => {}
            Some(Value::Dictionary(dict)) => {
                let crashed = dict.len() == 1
                    && matches!(dict.get("SuccessfulExit"), Some(Value::Boolean(false)));
                if crashed {
                    when.push("restarted after a crash".to_string());
                } else {
                    when.push("kept alive on conditions".to_string());
                }
            }
            Some(_) => when.push("kept alive on conditions".to_string()),
        }
        if let Some(seconds) = self.start_interval {
            when.push(format!("every {seconds} s"));
        }
        if self.start_calendar_interval.is_some() {
            when.push("on a schedule".to_string());
        }
        if self.start_on_mount == Some(true) {
            when.push("when a disk mounts".to_string());
        }
        if !self.queue_directories.is_empty() {
            when.push("while a queue has files".to_string());
        }
        if when.is_empty() {
            return "On demand".to_string();
        }
        let text = when.join(", ");
        let mut chars = text.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => text,
        }
    }
}

/// Reads the property list at `path`, XML or binary.
pub(crate) fn read_file(path: &Path) -> Result<FileModel, LaunchdError> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(LaunchdError::not_found(format!(
                "plist file not found: {}",
                path.display()
            )));
        }
        Err(error) => return Err(LaunchdError::io(error.to_string())),
    };
    parse_file(&data)
}

/// Decodes property-list bytes into the model.
pub(crate) fn parse_file(data: &[u8]) -> Result<FileModel, LaunchdError> {
    let value = Value::from_reader(Cursor::new(data))
        .map_err(|error| LaunchdError::plist(format!("failed to parse plist: {error}")))?;
    model_from_value(&value)
}

/// Decodes a property-list value into the model. Used for both a top-level
/// dictionary and, by tests, generated fixtures.
pub(crate) fn model_from_value(value: &Value) -> Result<FileModel, LaunchdError> {
    let dict = match value {
        Value::Dictionary(dict) => dict,
        _ => {
            return Err(LaunchdError::plist(
                "failed to parse plist: the top level is not a dictionary",
            ));
        }
    };
    Ok(FileModel {
        label: string_field(dict, "Label")?.unwrap_or_default(),
        disabled: bool_field(dict, "Disabled")?,
        program: string_field(dict, "Program")?,
        program_arguments: string_array_field(dict, "ProgramArguments")?,
        run_at_load: bool_field(dict, "RunAtLoad")?,
        keep_alive: dict.get("KeepAlive").cloned(),
        start_interval: integer_field(dict, "StartInterval")?,
        start_calendar_interval: dict.get("StartCalendarInterval").cloned(),
        working_directory: string_field(dict, "WorkingDirectory")?,
        standard_out_path: string_field(dict, "StandardOutPath")?,
        standard_error_path: string_field(dict, "StandardErrorPath")?,
        environment: string_map_field(dict, "EnvironmentVariables")?,
        start_on_mount: bool_field(dict, "StartOnMount")?,
        queue_directories: string_array_field(dict, "QueueDirectories")?.unwrap_or_default(),
        user_name: string_field(dict, "UserName")?,
        group_name: string_field(dict, "GroupName")?,
        root_directory: string_field(dict, "RootDirectory")?,
    })
}

/// Says why launchd cannot load the file at `path`, or `""` if it can.
pub(crate) fn problem(path: &Path) -> String {
    match read_file(path) {
        Err(error) => error.message,
        Ok(file) if file.label.is_empty() => {
            "it has no Label, so launchd cannot load it".to_string()
        }
        Ok(_) => String::new(),
    }
}

fn type_error(key: &str, expected: &str) -> LaunchdError {
    LaunchdError::plist(format!("failed to parse plist: {key} is not {expected}"))
}

fn string_field(dict: &Dictionary, key: &str) -> Result<Option<String>, LaunchdError> {
    match dict.get(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(type_error(key, "a string")),
    }
}

fn bool_field(dict: &Dictionary, key: &str) -> Result<Option<bool>, LaunchdError> {
    match dict.get(key) {
        None => Ok(None),
        Some(Value::Boolean(on)) => Ok(Some(*on)),
        Some(_) => Err(type_error(key, "a boolean")),
    }
}

fn integer_field(dict: &Dictionary, key: &str) -> Result<Option<i64>, LaunchdError> {
    match dict.get(key) {
        None => Ok(None),
        Some(Value::Integer(number)) => number
            .as_signed()
            .or_else(|| number.as_unsigned().and_then(|n| i64::try_from(n).ok()))
            .map(Some)
            .ok_or_else(|| type_error(key, "an integer")),
        Some(_) => Err(type_error(key, "an integer")),
    }
}

fn string_array_field(dict: &Dictionary, key: &str) -> Result<Option<Vec<String>>, LaunchdError> {
    match dict.get(key) {
        None => Ok(None),
        Some(Value::Array(values)) => {
            let mut strings = Vec::with_capacity(values.len());
            for value in values {
                match value {
                    Value::String(text) => strings.push(text.clone()),
                    _ => return Err(type_error(key, "an array of strings")),
                }
            }
            Ok(Some(strings))
        }
        Some(_) => Err(type_error(key, "an array of strings")),
    }
}

fn string_map_field(
    dict: &Dictionary,
    key: &str,
) -> Result<std::collections::BTreeMap<String, String>, LaunchdError> {
    let mut map = std::collections::BTreeMap::new();
    match dict.get(key) {
        None => Ok(map),
        Some(Value::Dictionary(entries)) => {
            for (name, value) in entries {
                match value {
                    Value::String(text) => {
                        map.insert(name.clone(), text.clone());
                    }
                    _ => return Err(type_error(key, "a dictionary of strings")),
                }
            }
            Ok(map)
        }
        Some(_) => Err(type_error(key, "a dictionary of strings")),
    }
}
