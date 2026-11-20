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
mod ui;

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{icons, note},
    store::{
        Section,
        development::{self as dev, Mode, Tab},
    },
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

/// The workspace's title, summary and a fresh read of the runtime home.
fn header() -> impl IntoElement {
    let software = move || dev::tab() == Tab::Software;
    view()
        .h(px(60.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
        .pt(tokens::space(2.0))
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text("Development")
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(
                    text(Section::Development.summary())
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .truncate(),
                ),
        )
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
            || {
                ui::action_button(
                    "Refresh",
                    ui::Tone::Default,
                    "dev-refresh",
                    false,
                    dev::refresh,
                )
            },
        ))
}

/// The read error, an action's error and a short notice.
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
        .child(dynamic(move || {
            dev::notice().map(|message| note(ui::check(), message, false))
        }))
}

/// The four tabs, left to right.
fn tabs() -> impl IntoElement {
    view()
        .h(px(38.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(0.5))
        .px(tokens::space(5.0))
        .border_b_1()
        .border_color(tokens::hairline)
        .child(Tab::ALL.into_iter().map(tab_button))
        .child(view().flex_1())
}

fn tab_button(tab: Tab) -> impl IntoElement {
    let active = move || dev::tab() == tab;
    nsh_base::button()
        .test_id(&format!("dev-tab-{}", tab.key()))
        .h(px(28.0))
        .px(tokens::space(2.5))
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .bg(move || {
            if active() {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_press(move || dev::select_tab(tab))
        .child(
            text(tab.label())
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    if active() {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
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
