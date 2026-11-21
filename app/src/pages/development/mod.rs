//! Development: the runtime workspace, ported from `view_development.go`,
//! `view_runtime_terminal.go`, `view_java.go` and `view_additional_runtimes.go`.
//!
//! Four tabs: the software workspace, the installed runtimes, the shell
//! startup integration and the overview. The runtime terminal is a page of its
//! own, opened from a runtime or the overview. The page reads
//! [`crate::store::development`] and never calls devcore itself.

mod defaults;
mod overview;
mod runtimes;
mod shell;
mod terminal;

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{controls as ui, icons, note},
    store::{
        development::{self as dev, Mode, Tab},
        software as sw,
    },
    theme as tokens,
};

pub(crate) fn development() -> impl IntoElement {
    // Keep `ui::tab_bar` referenced until sibling branches merge its allow attribute.
    let _ = || ui::tab_bar("", std::iter::empty::<AnyElement>());
    view()
        .test_id("development")
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .child(when(
            move || dev::mode() == Mode::Terminal,
            || {
                view()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .flex_col()
                    .child(terminal::terminal_page())
            },
        ))
        .child(when(
            move || dev::mode() == Mode::Workspace,
            || {
                view()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .flex_col()
                    .child(toolbar())
                    .child(notes())
                    .child(tab_content())
            },
        ))
}

/// Unified 52pt toolbar: Development title on the left, inline segmented
/// control for the four tabs, and status + Refresh icon on the right.
fn toolbar() -> impl IntoElement {
    let is_software = move || dev::tab() == Tab::Software;
    view()
        .h(px(tokens::TITLE_BAR_HEIGHT))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(3.0))
        .px(tokens::space(5.0))
        .border_b_1()
        .border_color(tokens::hairline)
        .window_drag(true)
        .child(
            text("Development")
                .flex_none()
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(segmented_tabs())
        .child(view().flex_1().min_w(px(0.0)))
        .child(
            view()
                .flex_none()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(when(
                    move || !is_software() && dev::busy(),
                    || {
                        text("Working…")
                            .test_id("dev-busy")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(tokens::warning)
                    },
                ))
                .child(when(
                    move || {
                        if is_software() {
                            sw::loading()
                        } else {
                            dev::loading()
                        }
                    },
                    || {
                        text("Reading…")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    },
                ))
                .child(when(is_software, || {
                    ui::toolbar_icon_button(
                        icons::refresh(),
                        "Refresh software",
                        "software-refresh",
                        sw::refresh,
                    )
                }))
                .child(when(
                    move || !is_software(),
                    || {
                        ui::toolbar_icon_button(
                            icons::refresh(),
                            "Refresh development",
                            "dev-refresh",
                            dev::refresh,
                        )
                    },
                )),
        )
}

/// Inline macOS-style segmented control inside the unified 52pt toolbar.
fn segmented_tabs() -> impl IntoElement {
    view()
        .flex_none()
        .p(px(2.0))
        .rounded(px(8.0))
        .bg(tokens::inset)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            nsh_base::tabs()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .accessibility_label("Development views")
                .indicator_style(|style| {
                    style
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .rounded(px(6.0))
                        .bg(tokens::panel)
                })
                .children(Tab::ALL.into_iter().map(tab_button)),
        )
}

/// The read error and an action's error.
pub(super) fn notes() -> impl IntoElement {
    view()
        .flex_col()
        .flex_none()
        .child(dynamic(move || {
            dev::load_error().map(|message| note(icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            dev::action_error().map(|message| note(icons::warn(), message, true))
        }))
}

fn tab_button(tab: Tab) -> impl IntoElement {
    ui::tab_item(
        tab.label(),
        format!("dev-tab-{}", tab.key()),
        move || dev::tab() == tab,
        move || dev::select_tab(tab),
    )
}

/// One tab's screen at a time.
fn tab_content() -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .child(when(
            move || dev::tab() == Tab::Software,
            super::software::software,
        ))
        .child(when(
            move || dev::tab() == Tab::Runtimes,
            runtimes::runtimes,
        ))
        .child(when(
            move || dev::tab() == Tab::ShellIntegration,
            shell::shell_integration,
        ))
        .child(when(
            move || dev::tab() == Tab::Overview,
            overview::overview,
        ))
}
