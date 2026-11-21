//! Freedesktop `.desktop` autostart entry parser and renderer.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Errors produced when parsing, validating, or managing XDG autostart entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutostartError {
    Invalid(String),
    NotFound(String),
    Io(String),
}

impl AutostartError {
    pub fn invalid(message: impl Into<String>) -> Self {
        AutostartError::Invalid(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        AutostartError::NotFound(message.into())
    }

    pub fn io(message: impl Into<String>) -> Self {
        AutostartError::Io(message.into())
    }
}

impl fmt::Display for AutostartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AutostartError::Invalid(msg)
            | AutostartError::NotFound(msg)
            | AutostartError::Io(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for AutostartError {}

impl From<std::io::Error> for AutostartError {
    fn from(error: std::io::Error) -> Self {
        AutostartError::Io(error.to_string())
    }
}

/// Structured representation of a Freedesktop `[Desktop Entry]` autostart file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopEntry {
    pub name: String,
    pub exec: String,
    pub try_exec: String,
    pub comment: String,
    pub icon: String,
    pub hidden: bool,
    pub gnome_autostart_enabled: Option<bool>,
    pub terminal: bool,
    pub no_display: bool,
    pub entry_type: String,
}

impl Default for DesktopEntry {
    fn default() -> Self {
        DesktopEntry {
            name: String::new(),
            exec: String::new(),
            try_exec: String::new(),
            comment: String::new(),
            icon: String::new(),
            hidden: false,
            gnome_autostart_enabled: Some(true),
            terminal: false,
            no_display: false,
            entry_type: "Application".to_string(),
        }
    }
}

impl DesktopEntry {
    /// Whether this entry is configured to start automatically at login.
    ///
    /// Per the XDG Autostart spec, `Hidden=true` disables the entry everywhere;
    /// GNOME/KDE/XFCE also respect `X-GNOME-Autostart-enabled=false`.
    pub fn is_enabled(&self) -> bool {
        !self.hidden && self.gnome_autostart_enabled != Some(false)
    }

    /// Validates required fields and rejects control characters / newlines.
    pub fn validate(&self) -> Result<(), AutostartError> {
        if self.name.trim().is_empty() {
            return Err(AutostartError::invalid("desktop entry Name is required"));
        }
        if self.exec.trim().is_empty() {
            return Err(AutostartError::invalid("desktop entry Exec is required"));
        }
        for (field, value) in [
            ("Name", &self.name),
            ("Exec", &self.exec),
            ("TryExec", &self.try_exec),
            ("Comment", &self.comment),
            ("Icon", &self.icon),
            ("Type", &self.entry_type),
        ] {
            if value.contains('\n') || value.contains('\r') || value.contains('\0') {
                return Err(AutostartError::invalid(format!(
                    "desktop entry {field} contains invalid control characters"
                )));
            }
        }
        Ok(())
    }

    /// Renders a valid Freedesktop `.desktop` file.
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        lines.push("[Desktop Entry]".to_string());
        let entry_type = if self.entry_type.trim().is_empty() {
            "Application"
        } else {
            self.entry_type.trim()
        };
        lines.push(format!("Type={entry_type}"));
        lines.push(format!("Name={}", self.name.trim()));
        if !self.comment.trim().is_empty() {
            lines.push(format!("Comment={}", self.comment.trim()));
        }
        lines.push(format!("Exec={}", self.exec.trim()));
        if !self.try_exec.trim().is_empty() {
            lines.push(format!("TryExec={}", self.try_exec.trim()));
        }
        if !self.icon.trim().is_empty() {
            lines.push(format!("Icon={}", self.icon.trim()));
        }
        if self.terminal {
            lines.push("Terminal=true".to_string());
        }
        if self.no_display {
            lines.push("NoDisplay=true".to_string());
        }
        lines.push(format!(
            "Hidden={}",
            if self.hidden { "true" } else { "false" }
        ));
        if let Some(enabled) = self.gnome_autostart_enabled {
            lines.push(format!(
                "X-GNOME-Autostart-enabled={}",
                if enabled { "true" } else { "false" }
            ));
        }
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

/// Validates a `.desktop` filename (basename only, no path traversal).
pub fn validate_desktop_filename(filename: &str) -> Result<(), AutostartError> {
    if filename.is_empty()
        || !filename.ends_with(".desktop")
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains('\0')
        || filename.contains("..")
        || filename.starts_with('.')
    {
        return Err(AutostartError::invalid(format!(
            "invalid desktop entry filename: {filename:?}"
        )));
    }
    Ok(())
}

/// Quotes one argument for a Freedesktop `.desktop` `Exec=` line.
pub fn quote_exec_arg(arg: &str) -> String {
    let needs_quote = arg.is_empty()
        || arg.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\''
                        | '\\'
                        | '`'
                        | '$'
                        | '&'
                        | ';'
                        | '<'
                        | '>'
                        | '('
                        | ')'
                        | '|'
                        | '*'
                        | '?'
                        | '#'
                        | '~'
                        | '='
                )
        });
    if !needs_quote {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for ch in arg.chars() {
        if matches!(ch, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

/// Parses a Freedesktop `.desktop` file's `[Desktop Entry]` group.
pub fn parse_desktop(content: &str) -> Result<DesktopEntry, AutostartError> {
    let mut in_desktop_entry = false;
    let mut saw_desktop_entry = false;
    let mut entry = DesktopEntry {
        gnome_autostart_enabled: None,
        entry_type: String::new(),
        ..DesktopEntry::default()
    };

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let group = &line[1..line.len() - 1];
            in_desktop_entry = group == "Desktop Entry";
            if in_desktop_entry {
                saw_desktop_entry = true;
            }
            continue;
        }
        if !in_desktop_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "Name" => entry.name = value.to_string(),
            "Exec" => entry.exec = value.to_string(),
            "TryExec" => entry.try_exec = value.to_string(),
            "Comment" => entry.comment = value.to_string(),
            "Icon" => entry.icon = value.to_string(),
            "Type" => entry.entry_type = value.to_string(),
            "Hidden" => entry.hidden = parse_bool(value),
            "X-GNOME-Autostart-enabled" => {
                entry.gnome_autostart_enabled = Some(parse_bool(value));
            }
            "Terminal" => entry.terminal = parse_bool(value),
            "NoDisplay" => entry.no_display = parse_bool(value),
            _ => {}
        }
    }

    if !saw_desktop_entry {
        return Err(AutostartError::invalid(
            "desktop file is missing [Desktop Entry] header",
        ));
    }
    if entry.entry_type.is_empty() {
        entry.entry_type = "Application".to_string();
    }
    Ok(entry)
}

/// Updates `Hidden` and `X-GNOME-Autostart-enabled` inside an existing `.desktop`
/// file while preserving all other groups, comments, and keys.
pub fn update_desktop_enabled(original: &str, enabled: bool) -> String {
    let hidden_line = format!("Hidden={}", if enabled { "false" } else { "true" });
    let gnome_line = format!(
        "X-GNOME-Autostart-enabled={}",
        if enabled { "true" } else { "false" }
    );

    let mut out: Vec<String> = Vec::new();
    let mut in_desktop_entry = false;
    let mut saw_desktop_entry = false;
    let mut wrote_hidden = false;
    let mut wrote_gnome = false;

    for raw_line in original.lines() {
        let trimmed = raw_line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_desktop_entry {
                if !wrote_hidden {
                    out.push(hidden_line.clone());
                    wrote_hidden = true;
                }
                if !wrote_gnome {
                    out.push(gnome_line.clone());
                    wrote_gnome = true;
                }
            }
            let group = &trimmed[1..trimmed.len() - 1];
            in_desktop_entry = group == "Desktop Entry";
            if in_desktop_entry {
                saw_desktop_entry = true;
            }
            out.push(raw_line.to_string());
            continue;
        }
        if in_desktop_entry && let Some((key, _)) = trimmed.split_once('=') {
            match key.trim() {
                "Hidden" => {
                    if !wrote_hidden {
                        out.push(hidden_line.clone());
                        wrote_hidden = true;
                    }
                    continue;
                }
                "X-GNOME-Autostart-enabled" => {
                    if !wrote_gnome {
                        out.push(gnome_line.clone());
                        wrote_gnome = true;
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(raw_line.to_string());
    }

    if !saw_desktop_entry {
        out.push("[Desktop Entry]".to_string());
        out.push(hidden_line);
        out.push(gnome_line);
    } else if in_desktop_entry {
        if !wrote_hidden {
            out.push(hidden_line);
        }
        if !wrote_gnome {
            out.push(gnome_line);
        }
    }

    let mut text = out.join("\n");
    text.push('\n');
    text
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_round_trips() {
        let entry = DesktopEntry {
            name: "Devbase".into(),
            exec: "/usr/bin/devbase --background".into(),
            try_exec: "/usr/bin/devbase".into(),
            comment: "Local developer environment".into(),
            icon: "devbase".into(),
            hidden: false,
            gnome_autostart_enabled: Some(true),
            terminal: false,
            no_display: true,
            entry_type: "Application".into(),
        };
        entry.validate().unwrap();
        let rendered = entry.render();
        let parsed = parse_desktop(&rendered).unwrap();
        assert_eq!(parsed, entry);
        assert!(parsed.is_enabled());
    }

    #[test]
    fn disabled_flags_disable_entry() {
        let hidden =
            parse_desktop("[Desktop Entry]\nName=App\nExec=/bin/app\nHidden=true\n").unwrap();
        assert!(!hidden.is_enabled());

        let gnome_off = parse_desktop(
            "[Desktop Entry]\nName=App\nExec=/bin/app\nX-GNOME-Autostart-enabled=false\n",
        )
        .unwrap();
        assert!(!gnome_off.is_enabled());
    }

    #[test]
    fn update_desktop_enabled_preserves_other_keys_and_groups() {
        let original = "\
[Desktop Entry]
Type=Application
Name=My App
Exec=/usr/bin/myapp --tray
OnlyShowIn=KDE;GNOME;
Hidden=false

[Desktop Action Quit]
Name=Quit
Exec=/usr/bin/myapp --quit
";
        let disabled = update_desktop_enabled(original, false);
        assert!(disabled.contains("OnlyShowIn=KDE;GNOME;"));
        assert!(disabled.contains("[Desktop Action Quit]"));
        let parsed = parse_desktop(&disabled).unwrap();
        assert!(!parsed.is_enabled());
        assert!(parsed.hidden);
        assert_eq!(parsed.gnome_autostart_enabled, Some(false));

        let reenabled = update_desktop_enabled(&disabled, true);
        let parsed2 = parse_desktop(&reenabled).unwrap();
        assert!(parsed2.is_enabled());
        assert!(!parsed2.hidden);
        assert_eq!(parsed2.gnome_autostart_enabled, Some(true));
    }

    #[test]
    fn quote_exec_arg_escapes_special_characters() {
        assert_eq!(quote_exec_arg("/usr/bin/devbase"), "/usr/bin/devbase");
        assert_eq!(
            quote_exec_arg("/opt/My App/devbase"),
            "\"/opt/My App/devbase\""
        );
        assert_eq!(quote_exec_arg("a$b\"c"), "\"a\\$b\\\"c\"");
    }

    #[test]
    fn validate_desktop_filename_rejects_traversal() {
        assert!(validate_desktop_filename("xyz.jyeu.devbase.desktop").is_ok());
        assert!(validate_desktop_filename("../evil.desktop").is_err());
        assert!(validate_desktop_filename("dir/app.desktop").is_err());
        assert!(validate_desktop_filename("app.txt").is_err());
        assert!(validate_desktop_filename(".hidden.desktop").is_err());
    }
}
