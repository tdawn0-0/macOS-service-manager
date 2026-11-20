//! Small controls the inspector composes: outlined and filled buttons, icon
//! buttons, and the note cards above a list.

use nsh_base::theme;
use nsh_core::{element::Svg, prelude::*};

use crate::theme as tokens;

/// A small button with a word on it.
pub(crate) fn labeled_button(
    label: impl Into<String>,
    primary: bool,
    disabled: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    nsh_base::button()
        .h(px(28.0))
        .flex_1()
        .items_center()
        .justify_center()
        .px(tokens::space(2.5))
        .rounded(move || theme().radius)
        .bg(move || {
            if primary {
                theme().primary
            } else {
                tokens::control()
            }
        })
        .border_1()
        .border_color(move || {
            if primary {
                theme().primary
            } else {
                tokens::hairline()
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .disabled(disabled)
        .opacity(if disabled { 0.5 } else { 1.0 })
        .text_color(move || {
            if primary {
                theme().primary_foreground
            } else {
                theme().foreground
            }
        })
        .on_press(on_press)
        .child(
            text(label.into())
                .text_size(px(tokens::TEXT_SMALL))
                .truncate(),
        )
}

/// A button that shows only an icon.
pub(crate) fn icon_button(
    icon: Svg,
    label: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    nsh_base::button()
        .size(px(26.0))
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .hover(move |style| style.bg(tokens::row_hover()))
        .accessibility_label(label.into())
        .on_press(on_press)
        .child(
            icon.size(px(14.0))
                .text_color(move || theme().muted_foreground),
        )
}

/// A rounded note above a list: an error, or what macOS keeps the source
/// from doing.
pub(crate) fn note(icon: Svg, message: impl Into<String>, error: bool) -> impl IntoElement {
    view()
        .flex_row()
        .items_start()
        .gap(tokens::space(2.0))
        .mx(tokens::space(3.0))
        .mt(tokens::space(2.0))
        .px(tokens::space(3.0))
        .py(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(move || note_fill(error))
        .child(
            icon.size(px(13.0))
                .flex_none()
                .mt(px(1.0))
                .text_color(move || note_tint(error)),
        )
        .child(
            text(message.into())
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || note_tint(error)),
        )
}

fn note_fill(error: bool) -> AdaptiveColor {
    if error {
        theme().destructive.opacity(0.1)
    } else {
        tokens::inset()
    }
}

fn note_tint(error: bool) -> AdaptiveColor {
    if error {
        theme().destructive
    } else {
        theme().muted_foreground
    }
}
