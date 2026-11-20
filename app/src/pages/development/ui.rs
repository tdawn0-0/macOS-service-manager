//! The development workspace's shared shapes: cards, tags and the small
//! action buttons the tabs compose.

use nsh_base::theme;
use nsh_core::{
    element::{ImageSrc, Svg, svg},
    prelude::*,
};

use crate::theme as tokens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Default,
    Primary,
    Danger,
}

/// A small action button with its test id and disabled state.
pub(super) fn action_button(
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

/// A chip, used for a version in a picker.
pub(super) fn chip(
    label: impl Into<String>,
    test_id: impl Into<String>,
    selected: bool,
    disabled: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let test_id = test_id.into();
    let fill = selected;
    nsh_base::button()
        .test_id(&test_id)
        .h(px(24.0))
        .flex_none()
        .items_center()
        .justify_center()
        .px(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(move || {
            if fill {
                tokens::row_selected()
            } else {
                tokens::control()
            }
        })
        .border_1()
        .border_color(tokens::hairline)
        .hover(move |style| style.bg(tokens::row_hover()))
        .disabled(disabled)
        .opacity(if disabled { 0.45 } else { 1.0 })
        .on_press(on_press)
        .child(text(label).text_size(px(tokens::TEXT_CAPTION)))
}

/// A pill naming a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TagTone {
    Neutral,
    Success,
    Warning,
}

pub(super) fn tag(label: impl Into<String>, tone: TagTone) -> impl IntoElement {
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

/// A card on the sheet: a title and its content.
pub(super) fn card(title: impl Into<String>, body: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(3.0))
        .p(tokens::space(4.0))
        .rounded(tokens::panel_radius)
        .bg(move || tokens::panel())
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

/// A thin line between rows inside a card.
pub(super) fn hairline() -> impl IntoElement {
    view().h(px(1.0)).flex_none().w_full().bg(tokens::hairline)
}

/// A small label above a value.
pub(super) fn field_label(label: impl Into<String>) -> impl IntoElement {
    text(label.into())
        .text_size(px(tokens::TEXT_CAPTION))
        .font_weight(FontWeight::MEDIUM)
        .text_color(move || theme().muted_foreground)
}

/// A check mark for a successful notice.
pub(super) fn check() -> Svg {
    svg(ImageSrc::svg_markup(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>"#,
    ))
}
