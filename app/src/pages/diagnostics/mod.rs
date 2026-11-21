//! Diagnostics: the runtime snapshot, environment checks, the port
//! inventory and the cross-process task journal.
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
    store::system,
    theme as tokens,
};

const REDACTED_TIP: &str = "Excludes paths, domains, commands, env, logs, and credentials";

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
                    .max_w(px(760.0))
                    .flex_col()
                    .gap(tokens::space(5.0))
                    .px(tokens::space(6.0))
                    .py(tokens::space(5.0))
                    .child(checks::runtime_card())
                    .child(checks::checks_card())
                    .child(ports::ports_card())
                    .child(operations::operations_card()),
            ),
        )
}

fn header() -> impl IntoElement {
    ui::page_header(
        "Diagnostics",
        None::<&str>,
        view()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(tokens::space(2.0))
            .child(
                text(checks::summary)
                    .text_size(px(tokens::TEXT_SMALL))
                    .tabular_nums()
                    .text_color(move || theme().muted_foreground)
                    .truncate(),
            )
            .child(dynamic(|| {
                let status = checks::status();
                (!status.is_empty()).then(|| ui::status_tag(status))
            }))
            .child(ui::action_button(
                "Run diagnostics",
                Tone::Primary,
                "diagnostics-run",
                system::report_loading,
                system::run_diagnostics,
            )),
    )
}

fn toolbar() -> impl IntoElement {
    view()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
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
        .child(
            view()
                .flex_none()
                .tooltip(ui::tooltip(REDACTED_TIP))
                .child(ui::action_button(
                    "Copy redacted report",
                    Tone::Default,
                    "diagnostics-copy",
                    || system::report().is_none() || system::report_loading(),
                    system::copy_redacted_report,
                )),
        )
        .child(
            view()
                .flex_none()
                .tooltip(ui::tooltip(REDACTED_TIP))
                .child(ui::action_button(
                    "Export redacted report",
                    Tone::Default,
                    "diagnostics-export",
                    || {
                        system::report().is_none()
                            || system::report_loading()
                            || system::report_exporting()
                    },
                    system::export_report,
                )),
        )
}

pub(super) fn section_card(
    title: &'static str,
    trailing: Option<AnyElement>,
    body: impl IntoElement,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .px(tokens::space(1.0))
                .child(
                    text(title)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().muted_foreground),
                )
                .children(trailing),
        )
        .child(
            view()
                .flex_col()
                .rounded(tokens::panel_radius)
                .bg(tokens::panel)
                .border_1()
                .border_color(tokens::hairline)
                .overflow_hidden()
                .child(body),
        )
}
