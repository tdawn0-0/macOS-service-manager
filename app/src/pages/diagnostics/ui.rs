//! The pieces the diagnostics screens compose: cards, buttons, pills, key
//! rows and notes. They follow the software and HTTPS proxy screens' look.

use nsh_base::theme;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagTone {
    Neutral,
    Success,
    Warning,
    Error,
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
            TagTone::Error => theme().destructive.opacity(0.12),
        })
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || match tone {
                    TagTone::Neutral => theme().muted_foreground,
                    TagTone::Success => tokens::success(),
                    TagTone::Warning => tokens::warning(),
                    TagTone::Error => theme().destructive,
                }),
        )
}

/// A status pill; `Unknown` is deliberately a warning, never success.
pub(crate) fn status_tag(status: impl Into<String>) -> impl IntoElement {
    let status = status.into();
    let tone = match status.as_str() {
        "OK" | "Healthy" | "Running" | "Available" | "Succeeded" | "Installed" => TagTone::Success,
        "Warning" | "Not ready" | "Unknown" | "Stopped" | "Disabled" | "Interrupted"
        | "Cancelled" | "In use" | "Checking" => TagTone::Warning,
        "Failed" => TagTone::Error,
        _ => TagTone::Neutral,
    };
    tag(status, tone)
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

/// A well inside a card, as the Go `devInset` drew checks and port results.
pub(crate) fn inset(body: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .p(tokens::space(2.5))
        .rounded(move || theme().radius)
        .bg(tokens::inset())
        .child(body)
}

/// A key and its value, as `devKV`.
pub(crate) fn kv(label: impl Into<String>, value: impl Into<String>) -> impl IntoElement {
    let label = label.into();
    let value = value.into();
    view()
        .flex_row()
        .items_start()
        .gap(tokens::space(2.0))
        .child(
            text(label)
                .w(px(120.0))
                .flex_none()
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(value)
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground)
                .selectable(true),
        )
}

/// A note above the cards: verbatim error text, or a hint.
pub(crate) fn note(message: impl Into<String>, error: bool) -> impl IntoElement {
    let message = message.into();
    view()
        .px(tokens::space(3.0))
        .py(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(move || {
            if error {
                theme().destructive.opacity(0.1)
            } else {
                tokens::inset()
            }
        })
        .child(
            text(message)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || {
                    if error {
                        theme().destructive
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
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
            nsh_base::input()
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

/// The switch the settings screen uses.
pub(crate) fn switch(
    test_id: impl Into<String>,
    on: bool,
    busy: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let test_id = test_id.into();
    let track = view()
        .w(px(30.0))
        .h(px(18.0))
        .flex_none()
        .flex_row()
        .items_center()
        .px(px(2.0))
        .rounded(px(9.0))
        .bg(move || {
            if on {
                theme().primary
            } else {
                theme().secondary
            }
        });
    let track = if on {
        track.justify_end()
    } else {
        track.justify_start()
    };
    nsh_base::button()
        .test_id(&test_id)
        .accessibility_label(if on { "On" } else { "Off" })
        .disabled(busy)
        .opacity(if busy { 0.5 } else { 1.0 })
        .on_press(on_press)
        .child(
            track.child(
                view()
                    .size(px(14.0))
                    .rounded(px(7.0))
                    .bg(move || theme().primary_foreground),
            ),
        )
}
