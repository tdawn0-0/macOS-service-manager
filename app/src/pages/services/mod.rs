//! System-service inspector: the launchd sources, their jobs, and the
//! inspector of the job chosen.
//!
//! The page reads [`crate::store::services`] and never calls the provider
//! itself: a list, a detail or an action is a task that writes the store when
//! it answers.

mod inspector;
mod list;

use std::time::Duration;

use devcore::service::Source;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::icons,
    store::services::{self as svc, SOURCES},
    theme as tokens,
};

pub(crate) fn services() -> impl IntoElement {
    // The page is made once; reading the source happens in a task, not in a
    // binding or in render.
    svc::load_active();
    // The log tab follows its file while it is open.
    set_interval(Duration::from_secs(1), svc::tick_log);

    view()
        .flex_1()
        .flex_row()
        .min_w(px(0.0))
        .child(source_rail())
        .child(list::list_pane())
        .child(when(
            move || svc::selected_id().is_some(),
            inspector::inspector,
        ))
}

/// The three launchd sources, with how many of their jobs there are and how
/// many run.
fn source_rail() -> impl IntoElement {
    view()
        .w(px(176.0))
        .flex_none()
        .flex_col()
        .bg(move || theme().background)
        .border_r_1()
        .border_color(tokens::hairline)
        .px(tokens::space(2.0))
        .py(tokens::space(3.0))
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .px(tokens::space(2.0))
                .pb(tokens::space(3.0))
                .child(
                    text("Services")
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(
                    text(crate::store::Section::Services.summary())
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .number_of_lines(2),
                ),
        )
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .child(SOURCES.iter().map(|source| source_row(*source))),
        )
}

/// A subtle tooltip bubble for icon-only actions in the Services screen.
pub(super) fn hint_tooltip(label: impl Into<String>) -> nsh_base::TooltipElement {
    nsh_base::tooltip()
        .px(tokens::space(2.0))
        .py(tokens::space(1.0))
        .rounded(move || theme().radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            text(label.into())
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().foreground),
        )
}

fn source_row(source: Source) -> impl IntoElement {
    nsh_base::button()
        .test_id(match source {
            Source::Brew => "source-brew",
            Source::LoginItem => "source-login-item",
            Source::UserAgent => "source-user-agent",
            Source::SystemAgent => "source-system-agent",
            Source::Daemon => "source-daemon",
        })
        .w_full()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.5))
        .py(tokens::space(1.5))
        .rounded(move || theme().radius)
        .bg(move || {
            if svc::active_source() == source {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_press(move || svc::select_source(source))
        .child(
            icons::source(source)
                .size(px(14.0))
                .flex_none()
                .text_color(move || {
                    if svc::active_source() == source {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .child(
            text(source.title())
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .text_color(move || theme().sidebar_foreground)
                .truncate(),
        )
        .child(
            text(move || {
                let (total, running) = svc::counts(source);
                if total == 0 {
                    String::new()
                } else if running > 0 {
                    format!("{running}/{total}")
                } else {
                    total.to_string()
                }
            })
            .text_size(px(tokens::TEXT_CAPTION))
            .text_color(move || theme().muted_foreground)
            .flex_none(),
        )
}
