//! Whisper-quiet, dismissible readiness chip mounted at the top of the
//! workspace only when a genuinely blocking item needs attention.
//!
//! Reads existing reactive store signals (zero blocking I/O) and surfaces at
//! most one inline action chip in priority order:
//! 1. Local CA untrusted while saved HTTPS proxy rules exist -> `Section::HttpsProxy` Authority
//! 2. Privileged helper missing once settings have loaded -> `Section::Settings`
//!
//! Optional conveniences (CLI wrapper, shell integration) stay out of the way
//! in Settings and Development so the workspace stays serene by default.

use devproxy::ca::material::TrustState;
use nsh_base::{button, theme};
use nsh_core::prelude::*;

use crate::{
    components::{controls, icons},
    store::{self, Section, proxy, system},
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

/// One genuinely blocking readiness item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadinessItem {
    CaTrust,
    Helper,
}

impl ReadinessItem {
    fn test_id(self) -> &'static str {
        match self {
            ReadinessItem::CaTrust => "readiness-pill-ca-trust",
            ReadinessItem::Helper => "readiness-pill-helper",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ReadinessItem::CaTrust => "Trust local CA",
            ReadinessItem::Helper => "Install privileged helper",
        }
    }

    fn navigate(self) {
        match self {
            ReadinessItem::CaTrust => {
                store::select_section(Section::HttpsProxy);
                proxy::open_authority();
            }
            ReadinessItem::Helper => {
                store::select_section(Section::Settings);
            }
        }
    }
}

/// Pure snapshot of blocking readiness inputs for reactive evaluation and unit tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReadinessInputs {
    settings_loaded: bool,
    helper_installed: bool,
    saved_rules_count: usize,
    ca_trusted: bool,
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
        Self {
            settings_loaded,
            helper_installed: helper.installed,
            saved_rules_count,
            ca_trusted,
        }
    }

    /// Returns the single highest-priority blocking item, if any.
    fn primary_pending(&self) -> Option<ReadinessItem> {
        if self.saved_rules_count > 0 && !self.ca_trusted {
            Some(ReadinessItem::CaTrust)
        } else if self.settings_loaded && !self.helper_installed {
            Some(ReadinessItem::Helper)
        } else {
            None
        }
    }
}

fn active_item() -> Option<ReadinessItem> {
    if is_dismissed() {
        None
    } else {
        ReadinessInputs::from_stores().primary_pending()
    }
}

fn should_show() -> bool {
    active_item().is_some()
}

fn readiness_chip(item: ReadinessItem) -> impl IntoElement {
    button()
        .test_id(item.test_id())
        .accessibility_label(item.label())
        .h(px(24.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(tokens::space(2.5))
        .rounded_full()
        .bg(tokens::control)
        .border_1()
        .border_color(tokens::hairline)
        .hover(|s| s.bg(tokens::row_hover()))
        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
        .on_press(move || item.navigate())
        .child(
            view()
                .size(px(6.0))
                .flex_none()
                .rounded_full()
                .bg(tokens::warning),
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
        )
}

/// Quiet, dismissible top bar surfacing at most one blocking action chip.
pub(crate) fn strip() -> impl IntoElement {
    when(should_show, || {
        view()
            .test_id("readiness-bar")
            .w_full()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(tokens::space(2.0))
            .px(tokens::space(5.0))
            .py(px(6.0))
            .bg(tokens::inset)
            .border_b_1()
            .border_color(tokens::hairline)
            .child(dynamic(|| {
                active_item().map(|item| readiness_chip(item).into_element())
            }))
            .child(view().flex_1().h(px(24.0)).window_drag(true))
            .child(
                view()
                    .flex_none()
                    .tooltip(controls::tooltip("Dismiss"))
                    .child(
                        button()
                            .test_id("readiness-dismiss")
                            .accessibility_label("Dismiss")
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
    fn empty_unloaded_inputs_stay_silent() {
        let inputs = ReadinessInputs::default();
        assert_eq!(inputs.primary_pending(), None);
    }

    #[test]
    fn prioritizes_untrusted_ca_then_missing_helper_and_auto_hides() {
        let mut inputs = ReadinessInputs {
            settings_loaded: true,
            helper_installed: false,
            saved_rules_count: 1,
            ca_trusted: false,
        };
        assert_eq!(inputs.primary_pending(), Some(ReadinessItem::CaTrust));

        // Once CA is trusted, surfaces missing helper.
        inputs.ca_trusted = true;
        assert_eq!(inputs.primary_pending(), Some(ReadinessItem::Helper));

        // Resolving helper hides the strip completely.
        inputs.helper_installed = true;
        assert_eq!(inputs.primary_pending(), None);

        // Untrusted CA with zero saved rules never nags.
        inputs.saved_rules_count = 0;
        inputs.ca_trusted = false;
        assert_eq!(inputs.primary_pending(), None);
    }
}
