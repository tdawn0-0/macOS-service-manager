//! Diagnostics: the runtime snapshot, environment checks, the port
//! inventory and the cross-process task journal, ported from
//! `view_diagnostics.go`, `view_ports.go` and `view_operations.go`.
//!
//! The page reads [`crate::store::system`] and never calls devcore itself: a
//! snapshot, a diagnosis, an export, a port check and every retry or dismiss
//! is a task that writes the store when it answers. Anything that comes or
//! goes with the store is a `dynamic`/`when` region: NSH components run once.

mod checks;
mod operations;
mod ports;
pub(crate) mod ui;

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
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
        .child(dynamic(|| Some(header().into_element())))
        .child(dynamic(|| Some(toolbar().into_element())))
        .child(scroll().flex_1().min_h(px(0.0)).child(dynamic(|| {
            Some(
                view()
                    .w_full()
                    .max_w(px(880.0))
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(6.0))
                    .py(tokens::space(5.0))
                    .children(system::report_notice().map(|notice| ui::note(notice, false)))
                    .child(checks::runtime_card())
                    .child(checks::checks_card())
                    .child(ports::ports_card())
                    .child(operations::operations_card())
                    .into_element(),
            )
        })))
}

fn header() -> impl IntoElement {
    let summary = checks::summary();
    let status = checks::status();
    let loading = system::report_loading();
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
            text(summary)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .truncate(),
        )
        .child(view().flex_1())
        .child(when(!status.is_empty(), move || {
            ui::status_tag(status.clone())
        }))
        .child(ui::action_button(
            "Run diagnostics again",
            ui::Tone::Primary,
            "diagnostics-run",
            loading,
            system::run_diagnostics,
        ))
}

fn toolbar() -> impl IntoElement {
    let has_report = system::report().is_some();
    let loading = system::report_loading();
    let exporting = system::report_exporting();
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
            ui::Tone::Default,
            "diagnostics-refresh",
            loading,
            system::refresh,
        ))
        .child(ui::action_button(
            "Copy redacted report",
            ui::Tone::Default,
            "diagnostics-copy",
            !has_report || loading,
            system::copy_redacted_report,
        ))
        .child(ui::action_button(
            "Export redacted report",
            ui::Tone::Default,
            "diagnostics-export",
            !has_report || loading || exporting,
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
