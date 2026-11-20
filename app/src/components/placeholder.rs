//! The shared shape of a screen that has not been built yet: a page header
//! and one card saying what will land here. Screens replace this by composing
//! their own components; nothing else in the shell changes.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{store::Section, theme as tokens};

pub(crate) fn placeholder(section: Section) -> impl IntoElement {
    scroll().flex_1().child(
        view()
            .w_full()
            .flex_col()
            .gap(tokens::space(6.0))
            .p(tokens::space(8.0))
            .child(header(section))
            .child(card(section)),
    )
}

/// The section's name and one-line summary.
fn header(section: Section) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(
            text(section.label())
                .text_2xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            text(section.summary())
                .text_size(px(tokens::TEXT_BODY))
                .text_color(move || theme().muted_foreground),
        )
}

/// What the screen holds once its phase lands.
fn card(section: Section) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .p(tokens::space(6.0))
        .rounded(move || tokens::panel_radius())
        .bg(move || tokens::panel())
        .border_1()
        .border_color(move || tokens::hairline())
        .child(
            text("Scaffold")
                .text_size(px(tokens::TEXT_BODY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().foreground),
        )
        .child(
            text(section.lands())
                .text_size(px(tokens::TEXT_BODY))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text("Phase 0 placeholder · the real screen lands in a later phase.")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
}
