//! Diagnostics: the runtime snapshot, environment checks, and the
//! cross-process task journal.
//!
//! The page reads [`crate::store::system`] and never calls devcore itself: a
//! snapshot, a diagnosis, an export, and every retry or dismiss is a task
//! that writes the store when it answers. The scroll container and cards
//! mount once with scoped reactive regions inside each card.

mod checks;
mod operations;

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{
        controls::{self as ui, Tone},
        icons,
    },
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
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .w_full()
                    .flex_col()
                    .items_center()
                    .px(tokens::space(6.0))
                    .py(tokens::space(5.0))
                    .child(
                        view()
                            .w_full()
                            .max_w(px(780.0))
                            .flex_col()
                            .gap(tokens::space(5.0))
                            .child(checks::runtime_card())
                            .child(checks::checks_card())
                            .child(operations::operations_card()),
                    ),
            ),
        )
}

fn header() -> impl IntoElement {
    let refresh_busy =
        || system::report_loading() || system::runtime_loading() || system::operations_loading();
    ui::page_header(
        "Diagnostics",
        None::<&str>,
        view()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(tokens::space(1.5))
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
            .child(
                view().flex_none().tooltip(ui::tooltip("Refresh")).child(
                    nsh_base::button()
                        .test_id("diagnostics-refresh")
                        .size(px(26.0))
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(move || theme().radius)
                        .hover(move |style| style.bg(tokens::row_hover()))
                        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
                        .disabled(refresh_busy)
                        .opacity(move || if refresh_busy() { 0.45_f32 } else { 1.0_f32 })
                        .accessibility_label("Refresh")
                        .on_press(system::refresh)
                        .child(
                            icons::refresh()
                                .size(px(14.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                ),
            )
            .child(ui::action_button(
                "Run diagnostics",
                Tone::Primary,
                "diagnostics-run",
                system::report_loading,
                system::run_diagnostics,
            )),
    )
}

pub(super) fn inset_hairline() -> impl IntoElement {
    view().w_full().px(tokens::space(4.0)).child(ui::hairline())
}

pub(super) fn section_card(
    title: &'static str,
    trailing: Option<AnyElement>,
    body: impl IntoElement,
) -> impl IntoElement {
    view()
        .w_full()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(tokens::space(2.0))
                .px(tokens::space(1.5))
                .child(
                    text(title)
                        .flex_none()
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().muted_foreground),
                )
                .children(trailing),
        )
        .child(
            view()
                .w_full()
                .flex_col()
                .rounded(tokens::panel_radius)
                .bg(tokens::panel)
                .border_1()
                .border_color(tokens::hairline)
                .overflow_hidden()
                .child(body),
        )
}
