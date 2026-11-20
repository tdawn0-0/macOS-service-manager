//! The pieces every database screen composes: cards, small buttons, pills,
//! fields and the notes above a pane. They follow the software screen's look
//! (`pages/software/mod.rs`) so both halves of the workspace match.

use nsh_base::{input, theme};
use nsh_core::element::Svg;
use nsh_core::prelude::*;

use crate::theme as tokens;

/// A small button's emphasis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Default,
    Primary,
    Danger,
}

pub(crate) fn action_button(
    label: impl Into<String>,
    tone: Tone,
    test_id: impl Into<String>,
    disabled: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let test_id = test_id.into();
    nsh_base::button()
        .test_id(&test_id)
        .h(px(26.0))
        .flex_none()
        .items_center()
        .justify_center()
        .px(tokens::space(2.5))
        .rounded(move || theme().radius)
        .bg(move || match tone {
            Tone::Primary => theme().primary,
            Tone::Default | Tone::Danger => tokens::control(),
        })
        .border_1()
        .border_color(move || match tone {
            Tone::Primary => theme().primary,
            Tone::Default | Tone::Danger => tokens::hairline(),
        })
        .hover(move |style| {
            if tone == Tone::Danger {
                style.bg(theme().destructive.opacity(0.12))
            } else {
                style.bg(tokens::row_hover())
            }
        })
        .disabled(disabled)
        .opacity(if disabled { 0.45 } else { 1.0 })
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || match tone {
                    Tone::Primary => theme().primary_foreground,
                    Tone::Default => theme().foreground,
                    Tone::Danger => theme().destructive,
                })
                .truncate(),
        )
}

/// A chip: a choice in a row of them.
pub(crate) fn chip(
    label: impl Into<String>,
    selected: bool,
    test_id: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let test_id = test_id.into();
    nsh_base::button()
        .test_id(&test_id)
        .h(px(24.0))
        .flex_none()
        .items_center()
        .justify_center()
        .px(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(move || {
            if selected {
                tokens::row_selected()
            } else {
                tokens::control()
            }
        })
        .border_1()
        .border_color(tokens::hairline)
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || {
                    if selected {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagTone {
    Neutral,
    Success,
    Warning,
}

pub(crate) fn tag(label: impl Into<String>, tone: TagTone) -> impl IntoElement {
    let label = label.into();
    view()
        .flex_none()
        .px(px(7.0))
        .py(px(1.0))
        .rounded(px(99.0))
        .bg(move || match tone {
            TagTone::Neutral => tokens::control(),
            TagTone::Success => tokens::success().opacity(0.14),
            TagTone::Warning => tokens::warning().opacity(0.16),
        })
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || match tone {
                    TagTone::Neutral => theme().muted_foreground,
                    TagTone::Success => tokens::success(),
                    TagTone::Warning => tokens::warning(),
                }),
        )
}

pub(crate) fn status_tag(status: &str) -> impl IntoElement {
    let tone = match status {
        "Running" | "Healthy" => TagTone::Success,
        "Starting" | "Restarting" | "Stopping" | "Unknown" => TagTone::Warning,
        _ => TagTone::Neutral,
    };
    tag(status.to_string(), tone)
}

/// A card on the sheet: a title and its content.
pub(crate) fn card(title: impl Into<String>, body: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(3.0))
        .p(tokens::space(4.0))
        .rounded(tokens::panel_radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            text(title.into())
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(body)
}

/// The card whose actions remove or erase something.
pub(crate) fn danger_card(title: impl Into<String>, body: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(3.0))
        .p(tokens::space(4.0))
        .rounded(tokens::panel_radius)
        .bg(move || theme().destructive.opacity(0.06))
        .border_1()
        .border_color(move || theme().destructive.opacity(0.35))
        .child(
            text(title.into())
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().destructive),
        )
        .child(body)
}

/// A thin line between parts of a card.
pub(crate) fn hairline() -> impl IntoElement {
    view().h(px(1.0)).flex_none().w_full().bg(tokens::hairline)
}

/// A labelled control.
pub(crate) fn field(label: impl Into<String>, control: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .min_w(px(0.0))
        .child(
            text(label.into())
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
        .child(control)
}

/// A single-line text field bound to the store.
pub(crate) fn text_field(
    test_id: impl Into<String>,
    placeholder: impl Into<String>,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    let test_id = test_id.into();
    let placeholder = placeholder.into();
    view()
        .h(px(28.0))
        .flex_row()
        .items_center()
        .rounded(move || theme().radius)
        .bg(tokens::control)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            input()
                .test_id(&test_id)
                .flex_1()
                .min_w(px(0.0))
                .h(px(26.0))
                .px(tokens::space(2.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground)
                .placeholder(placeholder)
                .value(value)
                .on_change(move |text: &str| on_change(text.to_owned())),
        )
}

/// A read-only value in a well, selectable so it can be copied by hand.
pub(crate) fn read_only_value(value: impl Into<String>) -> impl IntoElement {
    let value = value.into();
    view()
        .min_h(px(28.0))
        .flex_row()
        .items_center()
        .px(tokens::space(2.0))
        .py(tokens::space(1.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            text(value)
                .flex_1()
                .min_w(px(0.0))
                .font_family("monospace")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().foreground)
                .selectable(true),
        )
}

/// A small icon for an action inside a row.
pub(crate) fn row_icon_button(
    icon: Svg,
    label: impl Into<String>,
    test_id: impl Into<String>,
    disabled: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let test_id = test_id.into();
    nsh_base::button()
        .test_id(&test_id)
        .h(px(24.0))
        .flex_none()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(tokens::space(1.0))
        .px(tokens::space(1.5))
        .rounded(move || theme().radius)
        .hover(move |style| style.bg(tokens::row_hover()))
        .disabled(disabled)
        .opacity(if disabled { 0.4 } else { 1.0 })
        .accessibility_label(label.into())
        .on_press(on_press)
        .child(
            icon.size(px(13.0))
                .text_color(move || theme().muted_foreground),
        )
}
