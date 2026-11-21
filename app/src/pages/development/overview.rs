//! The Overview tab: the defaults at a glance, whether the shells are wired,
//! the resolution rules, and the way into the runtime terminal.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::controls::{Tone, action_button, card, hairline},
    store::development as dev,
    theme as tokens,
};

use super::defaults;

pub(super) fn overview() -> impl IntoElement {
    dev::load();
    scroll().flex_1().min_h(px(0.0)).child(
        view()
            .w_full()
            .items_center()
            .px(tokens::space(5.0))
            .py(tokens::space(5.0))
            .child(
                view()
                    .w_full()
                    .max_w(px(840.0))
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .child(defaults::node_defaults())
                    .child(defaults::java_defaults())
                    .child(defaults::python_defaults())
                    .child(shell_card())
                    .child(resolution_card()),
            ),
    )
}

fn shell_card() -> impl IntoElement {
    card(
        "Shell integration",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(shell_line("Node.js", dev::node_shell_enabled))
            .child(hairline())
            .child(shell_line("Java", dev::java_shell_enabled))
            .child(hairline())
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .pt(tokens::space(1.0))
                    .child(action_button(
                        "Configure shells",
                        Tone::Default,
                        "overview-shell",
                        false,
                        || dev::select_tab(dev::Tab::ShellIntegration),
                    )),
            ),
    )
}

fn shell_line(
    label: &'static str,
    enabled: impl Fn() -> bool + Copy + 'static,
) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(tokens::space(2.0))
        .py(tokens::space(0.5))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
        )
        .child(
            text(move || {
                if enabled() {
                    "Enabled".to_string()
                } else {
                    "Not enabled".to_string()
                }
            })
            .text_size(px(tokens::TEXT_SMALL))
            .text_color(move || {
                if enabled() {
                    tokens::success()
                } else {
                    theme().muted_foreground
                }
            }),
        )
}

fn resolution_card() -> impl IntoElement {
    card(
        "Runtime terminal",
        view()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::space(3.0))
            .child(
                text("Node.js on PATH")
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().foreground),
            )
            .child(action_button(
                "Open runtime terminal",
                Tone::Primary,
                "overview-open-terminal",
                false,
                dev::open_terminal,
            )),
    )
}
