//! Diagnostics: the runtime snapshot, environment checks, the port
//! inventory and the cross-process task journal, ported from
//! `view_diagnostics.go`, `view_ports.go` and `view_operations.go`.
//!
//! The page reads [`crate::store::system`] and never calls devcore itself: a
//! snapshot, a diagnosis, an export, a port check and every retry or dismiss
//! is a task that writes the store when it answers. The scroll container and
//! cards mount once with scoped reactive regions inside each card.

mod checks;
mod operations;
mod ports;

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::controls::{self as ui, Tone},
    store::{Section, system},
    theme as tokens,
};

pub(crate) fn diagnostics() -> impl IntoElement {
    // The page is made once; reads are tasks, not render work.
    system::load();

    view()
        .test_id("diagnostics-page")
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .child(header())
        .child(toolbar())
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .w_full()
                    .max_w(px(880.0))
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(6.0))
                    .py(tokens::space(5.0))
                    .child(dynamic(|| {
                        system::report_notice().map(|notice| ui::plain_note(notice, false))
                    }))
                    .child(checks::runtime_card())
                    .child(checks::checks_card())
                    .child(ports::ports_card())
                    .child(operations::operations_card()),
            ),
        )
}

fn header() -> impl IntoElement {
    view()
        .h(px(44.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(3.5))
        .border_b_1()
        .border_color(tokens::hairline)
        .child(
            text("Diagnostics")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            text(Section::Diagnostics.summary())
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .truncate(),
        )
        .child(
            text(checks::summary)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .truncate(),
        )
        .child(view().flex_1())
        .child(dynamic(|| {
            let status = checks::status();
            (!status.is_empty()).then(|| ui::status_tag(status))
        }))
        .child(ui::action_button(
            "Run diagnostics again",
            Tone::Primary,
            "diagnostics-run",
            system::report_loading,
            system::run_diagnostics,
        ))
}

fn toolbar() -> impl IntoElement {
    view()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(3.5))
        .py(tokens::space(2.0))
        .border_b_1()
        .border_color(tokens::hairline)
        .child(ui::action_button(
            "Refresh",
            Tone::Default,
            "diagnostics-refresh",
            system::report_loading,
            system::refresh,
        ))
        .child(ui::action_button(
            "Copy redacted report",
            Tone::Default,
            "diagnostics-copy",
            || system::report().is_none() || system::report_loading(),
            system::copy_redacted_report,
        ))
        .child(ui::action_button(
            "Export redacted report",
            Tone::Default,
            "diagnostics-export",
            || system::report().is_none() || system::report_loading() || system::report_exporting(),
            system::export_report,
        ))
        .child(view().flex_1())
        .child(
            text("Anonymous resource names. Excludes local paths, domains, commands, environment values, logs, credentials and private keys.")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .truncate(),
        )
}
