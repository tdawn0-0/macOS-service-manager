//! Dismissible Readiness Checklist strip mounted at the top of the workspace.
//!
//! Reads existing reactive store signals (zero blocking I/O) and surfaces at
//! most four actionable setup pills:
//! 1. Privileged Helper (`system::helper_status().installed`) -> `Section::Settings`
//! 2. Local CA Trust (`proxy::saved_rules().len() > 0` & CA untrusted) -> `Section::HttpsProxy` Authority
//! 3. `devbase` CLI (`system::cli_status().installed`) -> `Section::Settings`
//! 4. Shell Integration (node/java shell loaded and disabled) -> `Section::Development` Shell tab
//!
//! Hidden automatically when dismissed for the session, when no store has
//! loaded actionable readiness data (e.g. empty fake-only test states), or
//! once every relevant item is ready.

use devproxy::ca::material::TrustState;
use nsh_base::{button, theme};
use nsh_core::prelude::*;

use crate::{
    components::{
        controls,
        icons::{self, Icon},
    },
    store::{self, Section, development, proxy, system},
    theme as tokens,
};

#[derive(Debug, Clone, Default)]
struct ReadinessState {
    dismissed: bool,
}

fn state() -> Signal<ReadinessState> {
    global::<ReadinessState>()
}

/// Dismisses the readiness strip for the current session.
pub(crate) fn dismiss() {
    state().update(|s| s.dismissed = true);
}

/// Whether the user has dismissed the readiness strip for this session.
pub(crate) fn is_dismissed() -> bool {
    state().with(|s| s.dismissed)
}

/// One actionable readiness checklist item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadinessItem {
    Helper,
    CaTrust,
    Cli,
    ShellIntegration,
}

impl ReadinessItem {
    const ALL: [ReadinessItem; 4] = [
        ReadinessItem::Helper,
        ReadinessItem::CaTrust,
        ReadinessItem::Cli,
        ReadinessItem::ShellIntegration,
    ];

    fn test_id(self) -> &'static str {
        match self {
            ReadinessItem::Helper => "readiness-pill-helper",
            ReadinessItem::CaTrust => "readiness-pill-ca-trust",
            ReadinessItem::Cli => "readiness-pill-cli",
            ReadinessItem::ShellIntegration => "readiness-pill-shell",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ReadinessItem::Helper => "Privileged Helper",
            ReadinessItem::CaTrust => "Local CA Trust",
            ReadinessItem::Cli => "devbase CLI",
            ReadinessItem::ShellIntegration => "Shell Integration",
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            ReadinessItem::Helper => "Install the privileged helper in Settings",
            ReadinessItem::CaTrust => "Trust the local certificate authority for HTTPS rules",
            ReadinessItem::Cli => "Install the devbase CLI wrapper in Settings",
            ReadinessItem::ShellIntegration => {
                "Enable terminal integration for Node.js or Java runtimes"
            }
        }
    }

    fn icon(self) -> Icon {
        match self {
            ReadinessItem::Helper => Icon::Wrench,
            ReadinessItem::CaTrust => Icon::ShieldCheck,
            ReadinessItem::Cli => Icon::Terminal,
            ReadinessItem::ShellIntegration => Icon::Plug,
        }
    }

    fn navigate(self) {
        match self {
            ReadinessItem::Helper | ReadinessItem::Cli => {
                store::select_section(Section::Settings);
            }
            ReadinessItem::CaTrust => {
                store::select_section(Section::HttpsProxy);
                proxy::open_authority();
            }
            ReadinessItem::ShellIntegration => {
                store::select_section(Section::Development);
                development::close_terminal();
                development::select_tab(development::Tab::ShellIntegration);
            }
        }
    }
}

/// Pure snapshot of store readiness inputs for reactive evaluation and unit tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReadinessInputs {
    settings_loaded: bool,
    helper_installed: bool,
    cli_installed: bool,
    saved_rules_count: usize,
    ca_trusted: bool,
    available_shells: bool,
    has_node: bool,
    has_java: bool,
    node_shell_enabled: bool,
    java_shell_enabled: bool,
    ca_env_enabled: bool,
}

impl ReadinessInputs {
    fn from_stores() -> Self {
        let helper = system::helper_status();
        let cli = system::cli_status();
        let settings_loaded = !system::workspace_root().is_empty()
            || !helper.message.is_empty()
            || helper.installed
            || cli.installed
            || !cli.wrapper_path.is_empty()
            || !cli.target_executable.is_empty();
        let saved_rules_count = proxy::saved_rules().len();
        let ca_trusted =
            proxy::authority().is_some_and(|status| status.active.trust == TrustState::Trusted);
        let shells = development::shells();
        let available_shells = shells.iter().any(|row| row.available);
        Self {
            settings_loaded,
            helper_installed: helper.installed,
            cli_installed: cli.installed,
            saved_rules_count,
            ca_trusted,
            available_shells,
            has_node: !development::node_versions().is_empty(),
            has_java: !development::java_versions().is_empty(),
            node_shell_enabled: development::node_shell_enabled(),
            java_shell_enabled: development::java_shell_enabled(),
            ca_env_enabled: development::ca_env_enabled(),
        }
    }

    fn is_pending(&self, item: ReadinessItem) -> bool {
        match item {
            ReadinessItem::Helper => self.settings_loaded && !self.helper_installed,
            ReadinessItem::CaTrust => self.saved_rules_count > 0 && !self.ca_trusted,
            ReadinessItem::Cli => self.settings_loaded && !self.cli_installed,
            ReadinessItem::ShellIntegration => {
                if !self.available_shells {
                    return false;
                }
                match (self.has_node, self.has_java) {
                    (true, true) => !self.node_shell_enabled || !self.java_shell_enabled,
                    (true, false) => !self.node_shell_enabled,
                    (false, true) => !self.java_shell_enabled,
                    (false, false) => {
                        !self.node_shell_enabled && !self.java_shell_enabled && !self.ca_env_enabled
                    }
                }
            }
        }
    }

    fn any_pending(&self) -> bool {
        ReadinessItem::ALL.iter().any(|item| self.is_pending(*item))
    }
}

fn should_show() -> bool {
    !is_dismissed() && ReadinessInputs::from_stores().any_pending()
}

fn readiness_pill(item: ReadinessItem) -> impl IntoElement {
    when(
        move || ReadinessInputs::from_stores().is_pending(item),
        move || {
            view()
                .flex_none()
                .tooltip(controls::tooltip(item.tooltip()))
                .child(
                    button()
                        .test_id(item.test_id())
                        .accessibility_label(format!("Configure {}", item.label()))
                        .h(px(22.0))
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .px(tokens::space(2.0))
                        .rounded_full()
                        .bg(tokens::control)
                        .border_1()
                        .border_color(tokens::hairline)
                        .hover(|s| s.bg(tokens::row_hover()))
                        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
                        .on_press(move || item.navigate())
                        .child(
                            icons::render(item.icon())
                                .size(px(11.0))
                                .flex_none()
                                .text_color(tokens::warning),
                        )
                        .child(
                            text(item.label())
                                .text_size(px(tokens::TEXT_CAPTION))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground),
                        )
                        .child(
                            icons::chevron_right()
                                .size(px(10.0))
                                .flex_none()
                                .text_color(move || theme().muted_foreground),
                        ),
                )
        },
    )
}

/// Compact, dismissible top bar surfacing actionable environment readiness pills.
pub(crate) fn strip() -> impl IntoElement {
    when(should_show, || {
        view()
            .test_id("readiness-bar")
            .w_full()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(tokens::space(2.0))
            .px(tokens::space(4.0))
            .py(px(6.0))
            .bg(tokens::inset)
            .border_b_1()
            .border_color(tokens::hairline)
            .child(
                view()
                    .flex_none()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        view()
                            .w(px(6.0))
                            .h(px(6.0))
                            .flex_none()
                            .rounded_full()
                            .bg(tokens::warning),
                    )
                    .child(
                        text("Setup")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(move || theme().muted_foreground),
                    ),
            )
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens::space(1.5))
                    .child(ReadinessItem::ALL.iter().map(|item| readiness_pill(*item))),
            )
            .child(
                view()
                    .flex_none()
                    .tooltip(controls::tooltip("Dismiss for this session"))
                    .child(
                        button()
                            .test_id("readiness-dismiss")
                            .accessibility_label("Dismiss readiness checklist")
                            .size(px(20.0))
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .rounded(move || theme().radius)
                            .hover(|s| s.bg(tokens::row_hover()))
                            .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
                            .on_press(dismiss)
                            .child(
                                icons::close()
                                    .size(px(12.0))
                                    .text_color(move || theme().muted_foreground),
                            ),
                    ),
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_unloaded_inputs_have_no_pending_items() {
        let inputs = ReadinessInputs::default();
        assert!(!inputs.any_pending());
    }

    #[test]
    fn readiness_items_activate_only_when_relevant_and_hide_when_ready() {
        let mut inputs = ReadinessInputs {
            settings_loaded: true,
            helper_installed: false,
            cli_installed: false,
            saved_rules_count: 1,
            ca_trusted: false,
            available_shells: true,
            has_node: true,
            has_java: false,
            node_shell_enabled: false,
            java_shell_enabled: false,
            ca_env_enabled: false,
        };
        assert!(inputs.is_pending(ReadinessItem::Helper));
        assert!(inputs.is_pending(ReadinessItem::CaTrust));
        assert!(inputs.is_pending(ReadinessItem::Cli));
        assert!(inputs.is_pending(ReadinessItem::ShellIntegration));
        assert!(inputs.any_pending());

        // Resolving every item hides the entire strip.
        inputs.helper_installed = true;
        inputs.cli_installed = true;
        inputs.ca_trusted = true;
        inputs.node_shell_enabled = true;
        assert!(!inputs.any_pending());

        // CA trust is not required when there are no saved proxy rules.
        inputs.saved_rules_count = 0;
        inputs.ca_trusted = false;
        assert!(!inputs.is_pending(ReadinessItem::CaTrust));
    }
}
