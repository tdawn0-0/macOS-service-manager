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
    components::{
        controls::{self as ui, Tone},
        icons, note,
    },
    store::development::{self as dev, Mode, Tab},
    theme as tokens,
};

pub(crate) fn development() -> impl IntoElement {
    view()
        .test_id("development")
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .child(header())
        .child(notes())
        .child(when(
            move || dev::mode() == Mode::Terminal,
            terminal::terminal_page,
        ))
        .child(when(
            move || dev::mode() == Mode::Workspace,
            || {
                view()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex_col()
                    .child(tabs())
                    .child(tab_content())
            },
        ))
}

/// The workspace's title and a fresh read of the runtime home.
fn header() -> impl IntoElement {
    let software = move || dev::tab() == Tab::Software;
    ui::page_header(
        "Development",
        None::<&str>,
        view()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(tokens::space(2.0))
            .child(when(
                move || !software() && dev::busy(),
                || {
                    text("Working…")
                        .test_id("dev-busy")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(tokens::warning)
                },
            ))
            .child(when(
                move || !software() && dev::loading(),
                || {
                    text("Reading…")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                },
            ))
            .child(when(
                move || !software(),
                || ui::action_button("Refresh", Tone::Default, "dev-refresh", false, dev::refresh),
            )),
    )
}

/// The read error and an action's error.
fn notes() -> impl IntoElement {
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

/// The four tabs, left to right, backed by `nsh_base::tabs()` + `tab()`.
fn tabs() -> impl IntoElement {
    ui::tab_bar("Development views", Tab::ALL.into_iter().map(tab_button))
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
