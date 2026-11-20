//! The navigation rail: the app's identity over the six sections. The rail
//! sits on the sidebar's Liquid Glass fallback color; the workspace beside it
//! is opaque.

use nsh_base::{button, theme};
use nsh_core::prelude::*;

use crate::{
    meta,
    store::{self, Section},
    theme as tokens,
};

pub(crate) fn sidebar() -> impl IntoElement {
    view()
        .w_52()
        .flex_none()
        .flex_col()
        .bg(move || theme().sidebar)
        .px(tokens::space(2.0))
        .py(tokens::space(3.0))
        .child(identity())
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .child(Section::ALL.iter().map(|section| nav_item(*section))),
        )
        .child(view().flex_1())
        .child(footer())
}

/// The app's name and what it is, above the sections.
fn identity() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(0.5))
        .px(tokens::space(3.0))
        .pb(tokens::space(4.0))
        .child(
            text(meta::NAME)
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            text("macOS developer services")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground),
        )
}

/// One section: a full-width row, selected while it is the workspace's.
fn nav_item(section: Section) -> impl IntoElement {
    button()
        .w_full()
        .flex_row()
        .items_center()
        .justify_start()
        .px(tokens::space(2.5))
        .py(tokens::space(1.5))
        .rounded(move || theme().radius)
        .selected(move || store::active_section() == section)
        .bg(move || {
            if store::active_section() == section {
                theme().sidebar_accent
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(|s| s.bg(tokens::row_hover()))
        .active(|s| s.bg(move || theme().sidebar_accent))
        .on_press(move || store::select_section(section))
        .child(
            text(section.label())
                .text_sm()
                .text_color(move || theme().sidebar_foreground),
        )
}

/// The build's identity, at the bottom of the rail.
fn footer() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(0.5))
        .px(tokens::space(3.0))
        .pt(tokens::space(4.0))
        .child(
            text(format!("Version {}", meta::VERSION))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(meta::IDENTIFIER)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(format!("macOS {}+", meta::MINIMUM_MACOS))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
}
