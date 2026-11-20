//! The app's state: the selected section stays behind these functions, so
//! pages read and change it without owning it. A global is per surface, so
//! each `nsh run` and each headless test starts with its own.

use nsh_core::prelude::*;

pub(crate) mod services;
pub(crate) mod software;

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

    /// What the screen will hold once its phase lands.
    pub(crate) fn lands(self) -> &'static str {
        match self {
            Section::Services => {
                "The launchd job inspector: discovery, property-list detail, status and start/stop/restart controls, ported from view.go, view_detail.go and view_status.go."
            }
            Section::Development => {
                "The development workspace: installed software, Node/JDK/Python versions, health checks, package tasks and runtime terminals, ported from view_development.go and its child views."
            }
            Section::Databases => {
                "Managed MySQL, PostgreSQL, Redis and MongoDB instances: creation, ports, isolated runtimes, connection profiles, backups and restores, ported from view_databases.go and the database views."
            }
            Section::HttpsProxy => {
                "HTTPS rules for <name>.devbase.localhost, the local CA, proxy activation and the Cloudflare tunnel, ported from view_https_proxy.go, view_authority.go and view_proxy_activation.go."
            }
            Section::Diagnostics => {
                "The diagnostics surface: environment and file checks, listening ports, live logs and in-flight operations, ported from view_diagnostics.go, view_ports.go, view_operations.go and view_dev_logs.go."
            }
            Section::Settings => {
                "Settings: privileged helper install/reinstall/removal, the public devbase CLI wrapper, start-at-login and workspace preferences, ported from view_settings.go and settings.go."
            }
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
