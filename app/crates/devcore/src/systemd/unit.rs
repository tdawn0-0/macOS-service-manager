//! Structured systemd `.service` and `.timer` unit parsing, validation, and rendering.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::launchd::CommandError;

static UNIT_BASE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9:_@.-]{0,200}$").unwrap());

/// Allowed `Restart=` values in systemd service units.
const ALLOWED_RESTART: [&str; 7] = [
    "no",
    "on-success",
    "on-failure",
    "on-abnormal",
    "on-watchdog",
    "on-abort",
    "always",
];

/// Errors produced by the systemd unit parser, journal reader, or provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemdError {
    Invalid(String),
    NotFound(String),
    HelperNotInstalled,
    Helper(String),
    Command(CommandError),
    Io(String),
}

impl SystemdError {
    pub fn invalid(message: impl Into<String>) -> Self {
        SystemdError::Invalid(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        SystemdError::NotFound(message.into())
    }

    pub fn helper(message: impl Into<String>) -> Self {
        SystemdError::Helper(message.into())
    }

    pub fn io(message: impl Into<String>) -> Self {
        SystemdError::Io(message.into())
    }
}

impl fmt::Display for SystemdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SystemdError::Invalid(msg)
            | SystemdError::NotFound(msg)
            | SystemdError::Helper(msg)
            | SystemdError::Io(msg) => f.write_str(msg),
            SystemdError::HelperNotInstalled => f.write_str(crate::helper::NOT_INSTALLED_MESSAGE),
            SystemdError::Command(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for SystemdError {}

impl From<std::io::Error> for SystemdError {
    fn from(error: std::io::Error) -> Self {
        SystemdError::Io(error.to_string())
    }
}

impl From<CommandError> for SystemdError {
    fn from(error: CommandError) -> Self {
        SystemdError::Command(error)
    }
}

/// Structured representation of a systemd `.service` (and optional `.timer`) unit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitSpec {
    /// Unit name, e.g. `"my-worker.service"` or `"my-worker"`.
    pub name: String,
    pub description: String,
    pub exec_start: Vec<String>,
    pub working_directory: String,
    pub environment: BTreeMap<String, String>,
    /// Systemd `Restart=` policy (`"no"`, `"on-failure"`, `"always"`, etc.).
    pub restart: String,
    pub restart_sec: Option<u32>,
    pub wanted_by: Vec<String>,
    pub user: String,
    pub group: String,
    /// Optional stdout log file path (rendered as `StandardOutput=append:<path>`).
    pub stdout_path: String,
    /// Optional stderr log file path (rendered as `StandardError=append:<path>`).
    pub stderr_path: String,
    /// Optional systemd `OnCalendar=` expression for a companion `.timer`.
    pub on_calendar: String,
    /// Optional systemd `OnUnitActiveSec=` interval in seconds for a companion `.timer`.
    pub on_unit_active_sec: Option<u64>,
}

/// Validates and normalizes a systemd service unit name to end with `.service`.
pub fn normalize_service_unit_name(name: &str) -> Result<String, SystemdError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(SystemdError::invalid("unit name is required"));
    }
    if trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
        || trimmed.contains("..")
        || trimmed.starts_with('-')
    {
        return Err(SystemdError::invalid(format!(
            "invalid unit name: {trimmed:?}"
        )));
    }
    if !UNIT_BASE_RE.is_match(trimmed) {
        return Err(SystemdError::invalid(format!(
            "unit name contains invalid characters: {trimmed:?}"
        )));
    }
    if let Some(stem) = trimmed.strip_suffix(".service") {
        if stem.is_empty() {
            return Err(SystemdError::invalid("unit name cannot be '.service'"));
        }
        return Ok(trimmed.to_string());
    }
    // Reject other unit suffixes when a service unit is expected.
    for forbidden in [".socket", ".timer", ".target", ".mount", ".path", ".slice"] {
        if trimmed.ends_with(forbidden) {
            return Err(SystemdError::invalid(format!(
                "expected a .service unit name, got {trimmed:?}"
            )));
        }
    }
    Ok(format!("{trimmed}.service"))
}

/// Validates any systemd unit name (`.service`, `.timer`, `.socket`, etc.),
/// rejecting path traversal, flags, and control characters.
pub fn validate_any_unit_name(name: &str) -> Result<(), SystemdError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
        || trimmed.contains("..")
        || trimmed.starts_with('-')
        || !UNIT_BASE_RE.is_match(trimmed)
    {
        return Err(SystemdError::invalid(format!(
            "invalid systemd unit name: {name:?}"
        )));
    }
    Ok(())
}

impl UnitSpec {
    /// Returns the normalized `.service` filename.
    pub fn unit_filename(&self) -> Result<String, SystemdError> {
        normalize_service_unit_name(&self.name)
    }

    /// Returns the companion `.timer` filename.
    pub fn timer_filename(&self) -> Result<String, SystemdError> {
        let service = self.unit_filename()?;
        let stem = service.strip_suffix(".service").unwrap_or(&service);
        Ok(format!("{stem}.timer"))
    }

    /// Whether this spec carries a timer schedule (`on_calendar` or `on_unit_active_sec`).
    pub fn has_timer(&self) -> bool {
        !self.on_calendar.trim().is_empty() || self.on_unit_active_sec.is_some_and(|secs| secs > 0)
    }

    /// Validates unit name, `ExecStart`, restart policy, and control-character safety.
    pub fn validate(&self) -> Result<(), SystemdError> {
        normalize_service_unit_name(&self.name)?;
        if self.exec_start.is_empty() || self.exec_start[0].trim().is_empty() {
            return Err(SystemdError::invalid("ExecStart command is required"));
        }
        for arg in &self.exec_start {
            reject_control("ExecStart argument", arg)?;
        }
        reject_control("Description", &self.description)?;
        reject_control("WorkingDirectory", &self.working_directory)?;
        reject_control("User", &self.user)?;
        reject_control("Group", &self.group)?;
        reject_control("StandardOutput", &self.stdout_path)?;
        reject_control("StandardError", &self.stderr_path)?;
        reject_control("OnCalendar", &self.on_calendar)?;
        for (key, value) in &self.environment {
            if key.trim().is_empty()
                || key.contains('=')
                || key.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                return Err(SystemdError::invalid(format!(
                    "invalid environment variable name: {key:?}"
                )));
            }
            reject_control("Environment value", value)?;
        }
        for target in &self.wanted_by {
            reject_control("WantedBy", target)?;
        }
        let restart = self.restart.trim();
        if !restart.is_empty() && !ALLOWED_RESTART.contains(&restart) {
            return Err(SystemdError::invalid(format!(
                "unsupported Restart policy: {restart:?}"
            )));
        }
        Ok(())
    }

    /// Renders a clean systemd `.service` unit file.
    pub fn render_unit(&self) -> String {
        let mut out = String::new();
        out.push_str("[Unit]\n");
        if !self.description.trim().is_empty() {
            out.push_str(&format!("Description={}\n", self.description.trim()));
        }
        out.push('\n');

        out.push_str("[Service]\n");
        if self.has_timer() {
            out.push_str("Type=oneshot\n");
        }
        if !self.exec_start.is_empty() {
            let formatted = format_exec_start(&self.exec_start);
            out.push_str(&format!("ExecStart={formatted}\n"));
        }
        if !self.working_directory.trim().is_empty() {
            out.push_str(&format!(
                "WorkingDirectory={}\n",
                self.working_directory.trim()
            ));
        }
        if !self.user.trim().is_empty() {
            out.push_str(&format!("User={}\n", self.user.trim()));
        }
        if !self.group.trim().is_empty() {
            out.push_str(&format!("Group={}\n", self.group.trim()));
        }
        for (key, value) in &self.environment {
            out.push_str(&format!("Environment={}\n", quote_systemd_env(key, value)));
        }
        let restart = self.restart.trim();
        if !restart.is_empty() && !self.has_timer() {
            out.push_str(&format!("Restart={restart}\n"));
        }
        if let Some(secs) = self.restart_sec
            && secs > 0
            && !self.has_timer()
        {
            out.push_str(&format!("RestartSec={secs}\n"));
        }
        if !self.stdout_path.trim().is_empty() {
            out.push_str(&format!(
                "StandardOutput=append:{}\n",
                self.stdout_path.trim()
            ));
        }
        if !self.stderr_path.trim().is_empty() {
            out.push_str(&format!(
                "StandardError=append:{}\n",
                self.stderr_path.trim()
            ));
        }

        if !self.wanted_by.is_empty() {
            out.push('\n');
            out.push_str("[Install]\n");
            let targets: Vec<&str> = self
                .wanted_by
                .iter()
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty())
                .collect();
            if !targets.is_empty() {
                out.push_str(&format!("WantedBy={}\n", targets.join(" ")));
            }
        }
        out
    }

    /// Renders a companion systemd `.timer` unit file if a schedule is configured.
    pub fn render_timer(&self) -> Option<String> {
        if !self.has_timer() {
            return None;
        }
        let service_name = normalize_service_unit_name(&self.name).ok()?;
        let mut out = String::new();
        out.push_str("[Unit]\n");
        if !self.description.trim().is_empty() {
            out.push_str(&format!(
                "Description={} (Timer)\n",
                self.description.trim()
            ));
        }
        out.push('\n');
        out.push_str("[Timer]\n");
        if !self.on_calendar.trim().is_empty() {
            out.push_str(&format!("OnCalendar={}\n", self.on_calendar.trim()));
        }
        if let Some(secs) = self.on_unit_active_sec
            && secs > 0
        {
            out.push_str("OnBootSec=15s\n");
            out.push_str(&format!("OnUnitActiveSec={secs}s\n"));
        }
        out.push_str(&format!("Unit={service_name}\n"));
        out.push('\n');
        out.push_str("[Install]\n");
        out.push_str("WantedBy=timers.target\n");
        Some(out)
    }
}

/// Parses a systemd `.service` (and optionally `[Timer]`) unit file into a [`UnitSpec`].
pub fn parse_unit(name: &str, content: &str) -> Result<UnitSpec, SystemdError> {
    let mut spec = UnitSpec {
        name: name.to_string(),
        ..UnitSpec::default()
    };
    let mut section = String::new();
    let mut saw_section = false;

    for logical_line in join_continuations(content) {
        let trimmed = logical_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].trim().to_string();
            saw_section = true;
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match (section.as_str(), key) {
            ("Unit", "Description") => spec.description = value.to_string(),
            ("Service", "ExecStart") => {
                if value.is_empty() {
                    spec.exec_start.clear();
                } else {
                    spec.exec_start = split_exec_start(value);
                }
            }
            ("Service", "WorkingDirectory") => {
                spec.working_directory = value.trim_start_matches('-').to_string();
            }
            ("Service", "Environment") => {
                for pair in split_systemd_words(value) {
                    if let Some((k, v)) = pair.split_once('=') {
                        spec.environment.insert(k.to_string(), v.to_string());
                    }
                }
            }
            ("Service", "Restart") => spec.restart = value.to_string(),
            ("Service", "RestartSec") => {
                spec.restart_sec = parse_seconds(value).map(|s| s as u32);
            }
            ("Service", "User") => spec.user = value.to_string(),
            ("Service", "Group") => spec.group = value.to_string(),
            ("Service", "StandardOutput") => {
                if let Some(path) = extract_log_path(value) {
                    spec.stdout_path = path.to_string();
                }
            }
            ("Service", "StandardError") => {
                if let Some(path) = extract_log_path(value) {
                    spec.stderr_path = path.to_string();
                }
            }
            ("Install", "WantedBy") => {
                for target in value.split_whitespace() {
                    if !spec.wanted_by.iter().any(|t| t == target) {
                        spec.wanted_by.push(target.to_string());
                    }
                }
            }
            ("Timer", "OnCalendar") => spec.on_calendar = value.to_string(),
            ("Timer", "OnUnitActiveSec") => {
                spec.on_unit_active_sec = parse_seconds(value);
            }
            _ => {}
        }
    }

    if !saw_section {
        return Err(SystemdError::invalid(
            "unit file contains no systemd section headers",
        ));
    }
    Ok(spec)
}

/// Joins trailing-backslash continuation lines according to systemd.syntax(7).
fn join_continuations(content: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut continuing = false;

    for raw_line in content.lines() {
        let trimmed_start = raw_line.trim_start();
        if continuing && (trimmed_start.starts_with('#') || trimmed_start.starts_with(';')) {
            // Comments are ignored during continuation per systemd.syntax(7).
            continue;
        }
        let line = raw_line.trim_end_matches('\r');
        if let Some(without_slash) = line.strip_suffix('\\') {
            if continuing {
                current.push(' ');
            }
            current.push_str(without_slash);
            continuing = true;
        } else {
            if continuing {
                current.push(' ');
                current.push_str(line);
                result.push(std::mem::take(&mut current));
                continuing = false;
            } else {
                result.push(line.to_string());
            }
        }
    }
    if continuing {
        result.push(current);
    }
    result
}

fn extract_log_path(value: &str) -> Option<&str> {
    value
        .strip_prefix("append:")
        .or_else(|| value.strip_prefix("file:"))
        .or_else(|| value.strip_prefix("truncate:"))
        .filter(|path| !path.is_empty())
}

fn parse_seconds(value: &str) -> Option<u64> {
    let v = value.trim();
    if let Ok(n) = v.parse::<u64>() {
        return Some(n);
    }
    if let Some(s) = v.strip_suffix('s')
        && let Ok(n) = s.trim().parse::<u64>()
    {
        return Some(n);
    }
    if let Some(m) = v.strip_suffix('m')
        && let Ok(n) = m.trim().parse::<u64>()
    {
        return Some(n * 60);
    }
    if let Some(h) = v.strip_suffix('h')
        && let Ok(n) = h.trim().parse::<u64>()
    {
        return Some(n * 3600);
    }
    None
}

fn reject_control(field: &str, value: &str) -> Result<(), SystemdError> {
    if value.contains('\n') || value.contains('\r') || value.contains('\0') {
        return Err(SystemdError::invalid(format!(
            "{field} contains invalid control characters"
        )));
    }
    Ok(())
}

/// Quotes one argument for systemd `ExecStart=`.
pub fn quote_systemd_arg(arg: &str) -> String {
    let needs_quote = arg.is_empty()
        || arg
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | ';' | '$'));
    if !needs_quote {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for ch in arg.chars() {
        if matches!(ch, '"' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn format_exec_start(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_systemd_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_systemd_env(key: &str, value: &str) -> String {
    let combined = format!("{key}={value}");
    let needs_quote = value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\'));
    if !needs_quote {
        return combined;
    }
    let mut out = String::with_capacity(combined.len() + 2);
    out.push('"');
    for ch in combined.chars() {
        if matches!(ch, '"' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

/// Splits a systemd `ExecStart=` value, stripping leading command modifier
/// prefixes (`-`, `@`, `+`, `!`, `:`) on the first token.
fn split_exec_start(value: &str) -> Vec<String> {
    let mut words = split_systemd_words(value);
    if let Some(first) = words.first_mut() {
        let stripped = first.trim_start_matches(['-', '@', '+', '!', ':']);
        if !stripped.is_empty() {
            *first = stripped.to_string();
        }
    }
    words
}

/// Splits whitespace-delimited words respecting systemd single/double quotes and
/// backslash escapes.
pub fn split_systemd_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            Some(q) => {
                if ch == '\\' && q == '"' {
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                    }
                } else if ch == q {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '"' | '\'' => {
                    in_word = true;
                    quote = Some(ch);
                }
                '\\' => {
                    in_word = true;
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                    }
                }
                c if c.is_whitespace() => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                c => {
                    in_word = true;
                    current.push(c);
                }
            },
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_spec_renders_and_round_trips() {
        let spec = UnitSpec {
            name: "devbase-api".into(),
            description: "Devbase API Worker".into(),
            exec_start: vec![
                "/usr/local/bin/worker".into(),
                "--config".into(),
                "/etc/my app/config.toml".into(),
            ],
            working_directory: "/srv/api".into(),
            environment: BTreeMap::from([
                ("LANG".into(), "C.UTF-8".into()),
                ("GREETING".into(), "hello world".into()),
            ]),
            restart: "on-failure".into(),
            restart_sec: Some(5),
            wanted_by: vec!["default.target".into()],
            user: "alice".into(),
            group: "alice".into(),
            stdout_path: "/var/log/api.log".into(),
            stderr_path: "/var/log/api.err.log".into(),
            on_calendar: String::new(),
            on_unit_active_sec: None,
        };
        spec.validate().unwrap();
        assert_eq!(spec.unit_filename().unwrap(), "devbase-api.service");
        let rendered = spec.render_unit();
        assert!(
            rendered
                .contains("ExecStart=/usr/local/bin/worker --config \"/etc/my app/config.toml\"")
        );
        assert!(rendered.contains("Environment=\"GREETING=hello world\""));
        assert!(rendered.contains("StandardOutput=append:/var/log/api.log"));

        let parsed = parse_unit("devbase-api", &rendered).unwrap();
        assert_eq!(parsed, spec);
        assert!(spec.render_timer().is_none());
    }

    #[test]
    fn timer_spec_renders_companion_timer() {
        let spec = UnitSpec {
            name: "devbase-maintain.service".into(),
            description: "Log maintenance".into(),
            exec_start: vec!["/usr/bin/devbase".into(), "--dev".into(), "maintain".into()],
            on_unit_active_sec: Some(60),
            ..UnitSpec::default()
        };
        spec.validate().unwrap();
        assert_eq!(spec.timer_filename().unwrap(), "devbase-maintain.timer");
        let service = spec.render_unit();
        assert!(service.contains("Type=oneshot"));
        let timer = spec.render_timer().unwrap();
        assert!(timer.contains("OnUnitActiveSec=60s"));
        assert!(timer.contains("Unit=devbase-maintain.service"));
    }

    #[test]
    fn unit_validation_rejects_traversal_and_empty_exec() {
        let bad_name = UnitSpec {
            name: "../etc/passwd".into(),
            exec_start: vec!["/bin/true".into()],
            ..UnitSpec::default()
        };
        assert!(bad_name.validate().is_err());

        let empty_exec = UnitSpec {
            name: "valid.service".into(),
            ..UnitSpec::default()
        };
        assert!(empty_exec.validate().is_err());

        let bad_restart = UnitSpec {
            name: "valid.service".into(),
            exec_start: vec!["/bin/true".into()],
            restart: "invalid-policy".into(),
            ..UnitSpec::default()
        };
        assert!(bad_restart.validate().is_err());
    }

    #[test]
    fn parse_unit_handles_continuations_and_prefixes() {
        let raw = "\
# comment
[Unit]
Description=Sample

[Service]
ExecStart=-/usr/bin/env \\
  # inline comment inside continuation
  foo=bar
Environment=A=1 \"B=two words\"
RestartSec=2m
";
        let parsed = parse_unit("sample.service", raw).unwrap();
        assert_eq!(parsed.exec_start, vec!["/usr/bin/env", "foo=bar"]);
        assert_eq!(parsed.environment.get("A").map(String::as_str), Some("1"));
        assert_eq!(
            parsed.environment.get("B").map(String::as_str),
            Some("two words")
        );
        assert_eq!(parsed.restart_sec, Some(120));
    }
}
