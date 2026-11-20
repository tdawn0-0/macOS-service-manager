//! The app's state: the selected section stays behind these functions, so
//! pages read and change it without owning it. A global is per surface, so
//! each `nsh run` and each headless test starts with its own.

use nsh_core::prelude::*;

pub(crate) mod databases;
pub(crate) mod development;
pub(crate) mod proxy;
pub(crate) mod services;
pub(crate) mod software;
pub(crate) mod system;

/// A top-level section, in sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Section {
    /// launchd jobs and their lifecycle (Phase 1).
    #[default]
    Services,
    /// Software, runtimes and the development workspace (Phase 2).
    Development,
    /// Database instances, connections and backups (Phase 2).
    Databases,
    /// Local HTTPS rules, the CA and the Cloudflare tunnel (Phase 3).
    HttpsProxy,
    /// Environment checks, ports, logs and operations.
    Diagnostics,
    /// Helper, CLI, login items and preferences (Phase 4).
    Settings,
}

impl Section {
    /// Every section, in sidebar order.
    pub(crate) const ALL: [Section; 6] = [
        Section::Services,
        Section::Development,
        Section::Databases,
        Section::HttpsProxy,
        Section::Diagnostics,
        Section::Settings,
    ];

    /// Its label in the sidebar and the window title.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Section::Services => "Services",
            Section::Development => "Development",
            Section::Databases => "Databases",
            Section::HttpsProxy => "HTTPS Proxy",
            Section::Diagnostics => "Diagnostics",
            Section::Settings => "Settings",
        }
    }

    /// One line saying what the section is for.
    pub(crate) fn summary(self) -> &'static str {
        match self {
            Section::Services => "launchd jobs, their status and lifecycle controls.",
            Section::Development => "Runtimes and software the development workspace manages.",
            Section::Databases => "Database instances, ports, connection profiles and backups.",
            Section::HttpsProxy => "Local HTTPS rules, the local CA and the Cloudflare tunnel.",
            Section::Diagnostics => "Environment checks, listening ports, logs and operations.",
            Section::Settings => "Helper installation, CLI, login items and workspace preferences.",
        }
    }
}

fn section() -> Signal<Section> {
    global::<Section>()
}

/// The section the sidebar shows (tracked where it is read).
pub(crate) fn active_section() -> Section {
    section().get()
}

/// Shows a section.
pub(crate) fn select_section(next: Section) {
    section().set(next);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_in_sidebar_order() {
        assert_eq!(
            Section::ALL.map(Section::label),
            [
                "Services",
                "Development",
                "Databases",
                "HTTPS Proxy",
                "Diagnostics",
                "Settings",
            ]
        );
    }
}
