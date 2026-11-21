//! Shared UI kit built on `nsh_base` primitives: buttons, icon buttons with
//! tooltips, switches, tabs, tags, chips, cards, insets, key-value rows, text
//! fields, and notes.

use nsh_base::theme;
use nsh_core::{element::Svg, prelude::*};

use crate::theme as tokens;

// --- Preserved inspector controls (used by pages/services/*) -----------------

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
        .hover(move |style| {
            if primary {
                style.bg(theme().primary.opacity(0.86))
            } else {
                style.bg(tokens::row_hover())
            }
        })
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
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

/// A button that shows only an icon, with an accessible label and native tooltip.
pub(crate) fn icon_button(
    icon: Svg,
    label: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let tip = label.clone();
    view().flex_none().tooltip(tooltip(tip)).child(
        nsh_base::button()
            .size(px(26.0))
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(move || theme().radius)
            .hover(move |style| style.bg(tokens::row_hover()))
            .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
            .accessibility_label(label)
            .on_press(on_press)
            .child(
                icon.size(px(14.0))
                    .text_color(move || theme().muted_foreground),
            ),
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

// --- Shared page UI kit ------------------------------------------------------

/// Unified 52pt draggable page header matching macOS title bar height.
pub(crate) fn page_header(
    title: impl IntoReactive<SharedString>,
    subtitle: Option<impl IntoReactive<SharedString>>,
    trailing: impl IntoElement,
) -> impl IntoElement {
    page_header_with_leading(None::<AnyElement>, title, subtitle, Some(trailing))
}

/// Unified 52pt draggable page header with an optional leading control
/// (for example, a back button on nested sub-pages) and optional trailing controls.
pub(crate) fn page_header_with_leading(
    leading: Option<impl IntoElement>,
    title: impl IntoReactive<SharedString>,
    subtitle: Option<impl IntoReactive<SharedString>>,
    trailing: Option<impl IntoElement>,
) -> impl IntoElement {
    view()
        .h(px(tokens::TITLE_BAR_HEIGHT))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
        .border_b_1()
        .border_color(tokens::hairline)
        .window_drag(true)
        .children(leading)
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text(title)
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground)
                        .truncate(),
                )
                .children(subtitle.map(|sub| {
                    text(sub)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .tabular_nums()
                        .truncate()
                })),
        )
        .children(trailing)
}

/// A styled macOS-like tooltip bubble backed by `nsh_base::tooltip()`.
pub(crate) fn tooltip(label: impl Into<String>) -> nsh_base::TooltipElement {
    let label = label.into();
    nsh_base::tooltip()
        .placement(Placement::Top)
        .offset(px(6.0))
        .px(tokens::space(2.0))
        .py(tokens::space(1.0))
        .rounded(move || theme().radius)
        .bg(move || theme().popover)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().popover_foreground),
        )
}

/// A small button's emphasis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Default,
    Primary,
    Danger,
}

/// A compact action button with a `test_id` and static or reactive `disabled`
/// state.
pub(crate) fn action_button(
    label: impl Into<String>,
    tone: Tone,
    test_id: impl Into<String>,
    disabled: impl IntoReactive<bool> + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let test_id = test_id.into();
    let disabled = disabled.into_reactive();
    let disabled_opacity = disabled.clone();
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
        .hover(move |style| match tone {
            Tone::Primary => style.bg(theme().primary.opacity(0.86)),
            Tone::Danger => style.bg(theme().destructive.opacity(0.12)),
            Tone::Default => style.bg(tokens::row_hover()),
        })
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .disabled(move || disabled.get())
        .opacity(move || {
            if disabled_opacity.get() {
                0.45_f32
            } else {
                1.0_f32
            }
        })
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

/// A 26x26 icon-only toolbar button with a `test_id`, accessible label, and
/// native `nsh_base::tooltip`.
pub(crate) fn toolbar_icon_button(
    icon: Svg,
    label: impl Into<String>,
    test_id: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let tip = label.clone();
    let test_id = test_id.into();
    view().flex_none().tooltip(tooltip(tip)).child(
        nsh_base::button()
            .test_id(&test_id)
            .size(px(26.0))
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(move || theme().radius)
            .hover(move |style| style.bg(tokens::row_hover()))
            .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
            .accessibility_label(label)
            .on_press(on_press)
            .child(
                icon.size(px(14.0))
                    .text_color(move || theme().muted_foreground),
            ),
    )
}

/// A compact 24px icon-only action button inside a row, with a `test_id`,
/// reactive `disabled` state, accessible label, and native `nsh_base::tooltip`.
pub(crate) fn row_icon_button(
    icon: Svg,
    label: impl Into<String>,
    test_id: impl Into<String>,
    disabled: impl IntoReactive<bool> + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let tip = label.clone();
    let test_id = test_id.into();
    let disabled = disabled.into_reactive();
    let disabled_opacity = disabled.clone();
    view().flex_none().tooltip(tooltip(tip)).child(
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
            .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
            .disabled(move || disabled.get())
            .opacity(move || {
                if disabled_opacity.get() {
                    0.4_f32
                } else {
                    1.0_f32
                }
            })
            .accessibility_label(label)
            .on_press(on_press)
            .child(
                icon.size(px(13.0))
                    .text_color(move || theme().muted_foreground),
            ),
    )
}

/// A selectable chip: one of a small set of choices, supporting static or
/// reactive `selected` and `disabled` states.
pub(crate) fn chip(
    label: impl Into<String>,
    selected: impl IntoReactive<bool> + 'static,
    test_id: impl Into<String>,
    disabled: impl IntoReactive<bool> + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let test_id = test_id.into();
    let selected = selected.into_reactive();
    let selected_bg = selected.clone();
    let selected_text = selected.clone();
    let disabled = disabled.into_reactive();
    let disabled_opacity = disabled.clone();
    nsh_base::button()
        .test_id(&test_id)
        .h(px(24.0))
        .flex_none()
        .items_center()
        .justify_center()
        .px(tokens::space(2.0))
        .rounded(move || theme().radius)
        .selected(move || selected.get())
        .bg(move || {
            if selected_bg.get() {
                tokens::row_selected()
            } else {
                tokens::control()
            }
        })
        .border_1()
        .border_color(tokens::hairline)
        .hover(move |style| style.bg(tokens::row_hover()))
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .disabled(move || disabled.get())
        .opacity(move || {
            if disabled_opacity.get() {
                0.45_f32
            } else {
                1.0_f32
            }
        })
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || {
                    if selected_text.get() {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
}

/// A pill's semantic emphasis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagTone {
    Neutral,
    Success,
    Warning,
    Error,
}

/// A rounded status or metadata pill.
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
        "OK" | "Healthy" | "Running" | "Available" | "Succeeded" | "Installed" | "Published"
        | "Trusted" | "Ready" | "Expiring soon" => TagTone::Success,
        "Warning" | "Not ready" | "Unknown" | "Stopped" | "Disabled" | "Interrupted"
        | "Cancelled" | "In use" | "Checking" | "Starting" | "Restarting" | "Stopping"
        | "Not trusted" | "Not created" | "Pending" | "Not checked" => TagTone::Warning,
        "Failed" | "Invalid" | "Expired" => TagTone::Error,
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

/// A card whose actions remove or erase something.
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

/// A well inside a card, for checks, port results, and grouped metadata.
pub(crate) fn inset(body: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .p(tokens::space(2.5))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .child(body)
}

/// A thin dividing line between rows inside a card.
pub(crate) fn hairline() -> impl IntoElement {
    view().h(px(1.0)).flex_none().w_full().bg(tokens::hairline)
}

/// A small label above a control or section.
pub(crate) fn field_label(label: impl Into<String>) -> impl IntoElement {
    text(label.into())
        .text_size(px(tokens::TEXT_CAPTION))
        .font_weight(FontWeight::MEDIUM)
        .text_color(move || theme().muted_foreground)
}

/// A labelled control.
pub(crate) fn field(label: impl Into<String>, control: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .min_w(px(0.0))
        .child(field_label(label))
        .child(control)
}

/// A key and its value, with a 120pt label column.
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
                .tabular_nums()
                .text_color(move || theme().foreground)
                .selectable(true),
        )
}

/// A key-value row with a 120pt label column and selectable monospace value
/// for filesystem paths, addresses, and identifiers.
pub(crate) fn kv_mono(label: impl Into<String>, value: impl Into<String>) -> impl IntoElement {
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
                .font_family("monospace")
                .tabular_nums()
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().foreground)
                .selectable(true),
        )
}

/// A compact note without a leading icon (used inside cards and diagnostics/settings).
pub(crate) fn plain_note(message: impl Into<String>, error: bool) -> impl IntoElement {
    let message = message.into();
    view()
        .px(tokens::space(3.0))
        .py(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(move || note_fill(error))
        .child(
            text(message)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || note_tint(error)),
        )
}

/// A single-line text field bound to a store signal.
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
        .focus_within(|style| style.border_color(move || theme().ring))
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

/// A compact text field with a fixed width.
pub(crate) fn narrow_field(
    test_id: impl Into<String>,
    width: f32,
    placeholder: impl Into<String>,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    let field = text_field(test_id, placeholder, value, on_change);
    view().w(px(width)).flex_none().child(field)
}

/// A read-only monospace value in a bordered well, selectable for copying.
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

/// A selectable monospaced value with a `test_id`.
pub(crate) fn mono_value(test_id: impl Into<String>, value: impl Into<String>) -> impl IntoElement {
    let test_id = test_id.into();
    let value = value.into();
    view().test_id(&test_id).flex_1().min_w(px(0.0)).child(
        text(value)
            .font_family("monospace")
            .text_size(px(tokens::TEXT_CAPTION))
            .text_color(move || theme().foreground)
            .selectable(true),
    )
}

/// An accessible on/off switch backed by `nsh_base::switch()` (ARIA switch
/// role, Space/Return keyboard toggle, focus ring, and reactive checked/disabled
/// state).
pub(crate) fn switch(
    test_id: impl Into<String>,
    checked: impl IntoReactive<bool> + 'static,
    disabled: impl IntoReactive<bool> + 'static,
    accessibility_label: impl Into<String>,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let test_id = test_id.into();
    let label = accessibility_label.into();
    let checked = checked.into_reactive();
    let disabled = disabled.into_reactive();
    let disabled_opacity = disabled.clone();
    nsh_base::switch()
        .test_id(&test_id)
        .flex_none()
        .rounded(px(9.0))
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .accessibility_label(label)
        .checked(move || checked.get())
        .disabled(move || disabled.get())
        .opacity(move || {
            if disabled_opacity.get() {
                0.5_f32
            } else {
                1.0_f32
            }
        })
        .track_style(|style| {
            style
                .w(px(30.0))
                .h(px(18.0))
                .flex_none()
                .flex_row()
                .items_center()
                .px(px(2.0))
                .rounded(px(9.0))
                .bg(move || theme().secondary)
                .checked(|checked| checked.bg(move || theme().primary))
        })
        .thumb_style(|style| {
            style
                .size(px(14.0))
                .rounded(px(7.0))
                .bg(move || theme().primary_foreground)
                .checked(|checked| checked.translate_x(px(12.0)))
        })
        .on_change(move |next: &bool| on_change(*next))
}

/// A tab bar backed by `nsh_base::tabs()` with roving arrow-key navigation and
/// a spring-animated selection indicator.
pub(crate) fn tab_bar<E: IntoElement>(
    accessibility_label: impl Into<String>,
    tabs: impl IntoIterator<Item = E>,
) -> impl IntoElement {
    view()
        .h(px(38.0))
        .flex_none()
        .flex_row()
        .items_center()
        .px(tokens::space(5.0))
        .border_b_1()
        .border_color(tokens::hairline)
        .child(
            nsh_base::tabs()
                .flex_row()
                .items_center()
                .gap(tokens::space(0.5))
                .accessibility_label(accessibility_label.into())
                .indicator_style(|style| {
                    style
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .rounded(move || theme().radius)
                        .bg(tokens::row_selected)
                })
                .children(tabs),
        )
        .child(view().flex_1())
}

/// A single tab inside [`tab_bar`], backed by `nsh_base::tab()`.
pub(crate) fn tab_item(
    label: impl Into<String>,
    test_id: impl Into<String>,
    selected: impl IntoReactive<bool> + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let test_id = test_id.into();
    let selected = selected.into_reactive();
    let selected_text = selected.clone();
    nsh_base::tab()
        .test_id(&test_id)
        .h(px(28.0))
        .px(tokens::space(2.5))
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .selected(move || selected.get())
        .hover(move |style| style.bg(tokens::row_hover()))
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    if selected_text.get() {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
}
