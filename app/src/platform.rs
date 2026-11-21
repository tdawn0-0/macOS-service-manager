//! Thin platform UI facade centralizing macOS vs Linux capability flags,
//! native copy, default service scopes, and desktop shell actions.
//!
//! All desktop shell actions (`open_url`, `open_log_file`, `reveal_path`,
//! `open_in_terminal`) delegate directly to [`devcore::platform::desktop`].

use std::path::Path;

use devcore::service::Source;
use nsh_core::{platform::Os, prelude::os};

/// Whether this binary was compiled for macOS.
pub const fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

/// Whether this binary was compiled for Linux.
pub const fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

/// Whether macOS 26 Liquid Glass (`NSGlassEffectView`) is available on the
/// active runtime surface. Always `false` on Linux and in non-macOS headless
/// surfaces.
pub fn has_liquid_glass() -> bool {
    is_macos() && os() == Os::MacOs
}

/// Whether the host platform has macOS Full Disk Access prompts/guides.
pub const fn has_full_disk_access() -> bool {
    is_macos()
}

/// Whether the host platform has macOS Privacy/TCC System Settings permissions.
pub const fn has_tcc_permissions() -> bool {
    is_macos()
}

/// The default user-scoped service source (`UserAgent` on macOS, `SystemdUser` on Linux).
pub const fn default_user_source() -> Source {
    if is_macos() {
        Source::UserAgent
    } else {
        Source::SystemdUser
    }
}

/// Native label for revealing a file or folder in the system file manager.
pub const fn reveal_label() -> &'static str {
    reveal_label_for(is_macos())
}

/// Native label for revealing a named item in the system file manager.
pub fn show_in_files_label(name: &str) -> String {
    if is_macos() {
        format!("Show {name} in Finder")
    } else {
        format!("Show {name} in Files")
    }
}

/// Native label for opening a log file in the system log viewer / editor.
pub const fn open_log_label() -> &'static str {
    open_log_label_for(is_macos())
}

/// Menu / palette label for creating a new service definition.
pub const fn new_service_label() -> &'static str {
    new_service_label_for(is_macos())
}

/// Short toolbar button label for creating a new service definition.
pub const fn new_service_button_label() -> &'static str {
    if is_macos() { "New Job" } else { "New Unit" }
}

/// Context-menu label for editing a service definition file.
pub const fn edit_service_label(source: Source) -> &'static str {
    match source {
        Source::SystemdUser | Source::SystemdSystem => "Edit Unit…",
        _ => "Edit Plist…",
    }
}

/// Action tooltip for registering/loading a service definition.
pub const fn service_register_label() -> &'static str {
    service_register_label_for(is_macos())
}

/// Keyboard shortcut hint shown on the sidebar Command Palette trigger.
pub const fn command_palette_shortcut_label() -> &'static str {
    if is_macos() { "⌘K" } else { "Ctrl+K" }
}

/// Opens `url` in the system's default browser / URL handler.
pub fn open_url(url: &str) -> Result<(), String> {
    devcore::platform::desktop::open_url(url).map_err(|error| error.to_string())
}

/// Opens a log file in macOS Console.app or the Linux default log viewer (`xdg-open`).
pub fn open_log_file(path: &Path) -> Result<(), String> {
    devcore::platform::desktop::open_log_file(path).map_err(|error| error.to_string())
}

/// Reveals `path` in Finder on macOS or the user's file manager on Linux.
pub fn reveal_path(path: &Path) -> Result<(), String> {
    devcore::platform::desktop::reveal_path(path).map_err(|error| error.to_string())
}

/// Launches `script_path` in Terminal.app on macOS or the detected terminal emulator on Linux.
pub fn open_in_terminal(script_path: &Path) -> Result<(), String> {
    devcore::platform::desktop::open_in_terminal(script_path).map_err(|error| error.to_string())
}

const fn reveal_label_for(macos: bool) -> &'static str {
    if macos {
        "Reveal in Finder"
    } else {
        "Show in Files"
    }
}

const fn open_log_label_for(macos: bool) -> &'static str {
    if macos {
        "Open in Console"
    } else {
        "Open Log File"
    }
}

const fn new_service_label_for(macos: bool) -> &'static str {
    if macos { "New Job…" } else { "New Unit…" }
}

const fn service_register_label_for(macos: bool) -> &'static str {
    if macos {
        "Register with launchd"
    } else {
        "Load unit"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_labels_match_macos_and_linux_conventions() {
        assert_eq!(reveal_label_for(true), "Reveal in Finder");
        assert_eq!(reveal_label_for(false), "Show in Files");

        assert_eq!(open_log_label_for(true), "Open in Console");
        assert_eq!(open_log_label_for(false), "Open Log File");

        assert_eq!(new_service_label_for(true), "New Job…");
        assert_eq!(new_service_label_for(false), "New Unit…");

        assert_eq!(service_register_label_for(true), "Register with launchd");
        assert_eq!(service_register_label_for(false), "Load unit");

        assert_eq!(edit_service_label(Source::UserAgent), "Edit Plist…");
        assert_eq!(edit_service_label(Source::SystemdUser), "Edit Unit…");
        assert_eq!(edit_service_label(Source::SystemdSystem), "Edit Unit…");
    }

    #[test]
    fn platform_flags_are_mutually_consistent() {
        assert_ne!(is_macos(), is_linux());
        assert_eq!(has_full_disk_access(), is_macos());
        assert_eq!(has_tcc_permissions(), is_macos());
        if is_macos() {
            assert_eq!(default_user_source(), Source::UserAgent);
        } else {
            assert_eq!(default_user_source(), Source::SystemdUser);
        }
    }
}
