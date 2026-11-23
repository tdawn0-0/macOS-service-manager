//! The app's state: the selected section stays behind these functions, so
//! pages read and change it without owning it. A global is per surface, so
//! each `nsh run` and each headless test starts with its own.

use nsh_core::prelude::*;

pub(crate) mod databases;
pub(crate) mod development;
pub(crate) mod ports;
pub(crate) mod proxy;
pub(crate) mod services;
pub(crate) mod software;
pub(crate) mod system;

/// Semantic grouping for top-level sections in the sidebar rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionGroup {
    /// Primary developer workspace: software/runtimes, databases, and HTTPS proxy.
    Workspace,
    /// Secondary system inspection, diagnostics, and app settings.
    System,
}

impl SectionGroup {
    /// Sidebar groups in display order.
    pub(crate) const ALL: [SectionGroup; 2] = [SectionGroup::Workspace, SectionGroup::System];

    /// Stable `test_id` for the group container in the sidebar.
    pub(crate) fn test_id(self) -> &'static str {
        match self {
            SectionGroup::Workspace => "sidebar-group-workspace",
            SectionGroup::System => "sidebar-group-system",
        }
    }

    /// Sections belonging to this group, in sidebar order.
    pub(crate) fn sections(self) -> &'static [Section] {
        match self {
            SectionGroup::Workspace => &[
                Section::Development,
                Section::Databases,
                Section::HttpsProxy,
            ],
            SectionGroup::System => &[
                Section::Services,
                Section::Ports,
                Section::Diagnostics,
                Section::Settings,
            ],
        }
    }
}

/// A top-level section, in sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Section {
    /// Software, runtimes and the development workspace.
    #[default]
    Development,
    /// Database instances, connections and backups.
    Databases,
    /// Local HTTPS rules, the CA and the Cloudflare tunnel.
    HttpsProxy,
    /// launchd / systemd jobs and their lifecycle.
    Services,
    /// Active listening TCP ports, starring/watching, HTTPS handoff, and safe process stop.
    Ports,
    /// Environment checks, logs and operations.
    Diagnostics,
    /// Helper, CLI, login items and preferences.
    Settings,
}

impl Section {
    /// Every section, in sidebar order (`Cmd+1..7`).
    pub(crate) const ALL: [Section; 7] = [
        Section::Development,
        Section::Databases,
        Section::HttpsProxy,
        Section::Services,
        Section::Ports,
        Section::Diagnostics,
        Section::Settings,
    ];

    /// The sidebar group this section belongs to.
    #[allow(dead_code)]
    pub(crate) fn group(self) -> SectionGroup {
        match self {
            Section::Development | Section::Databases | Section::HttpsProxy => {
                SectionGroup::Workspace
            }
            Section::Services | Section::Ports | Section::Diagnostics | Section::Settings => {
                SectionGroup::System
            }
        }
    }

    /// Its label in the sidebar and the window title.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Section::Development => "Development",
            Section::Databases => "Databases",
            Section::HttpsProxy => "HTTPS Proxy",
            Section::Services => "Services",
            Section::Ports => "Ports",
            Section::Diagnostics => "Diagnostics",
            Section::Settings => "Settings",
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
        assert_eq!(Section::default(), Section::Development);
        assert_eq!(
            Section::ALL.map(Section::label),
            [
                "Development",
                "Databases",
                "HTTPS Proxy",
                "Services",
                "Ports",
                "Diagnostics",
                "Settings",
            ]
        );
        assert_eq!(
            SectionGroup::Workspace.sections(),
            &[
                Section::Development,
                Section::Databases,
                Section::HttpsProxy,
            ]
        );
        assert_eq!(
            SectionGroup::System.sections(),
            &[
                Section::Services,
                Section::Ports,
                Section::Diagnostics,
                Section::Settings,
            ]
        );
        for section in Section::ALL {
            assert!(section.group().sections().contains(&section));
        }
    }
}
