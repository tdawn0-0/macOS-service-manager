//! The Overview tab: the defaults at a glance, whether the shells are wired,
//! the resolution rules, and the way into the runtime terminal.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{store::development as dev, theme as tokens};

use super::{
    defaults,
    ui::{Tone, action_button, card},
};

pub(super) fn overview() -> impl IntoElement {
    dev::load();
    dynamic(|| Some(overview_body().into_element()))
}

fn overview_body() -> impl IntoElement {
    scroll().flex_1().min_h(px(0.0)).child(
        view()
            .flex_col()
            .gap(tokens::space(4.0))
            .px(tokens::space(5.0))
            .py(tokens::space(4.0))
            .child(defaults::node_defaults())
            .child(defaults::java_defaults())
            .child(defaults::python_defaults())
            .child(shell_card())
            .child(resolution_card()),
    )
}

fn shell_card() -> impl IntoElement {
    let node = dev::node_shell_enabled();
    let java = dev::java_shell_enabled();
    card(
        "Shell integration",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(shell_line("Node.js", node))
            .child(shell_line("Java", java))
            .child(
                text("The startup block makes each folder's .node-version, .nvmrc or package.json engines.node the folder's Node.js, else the default.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            )
            .child(action_button(
                "Change shell integration",
                Tone::Default,
                "overview-shell",
                false,
                || dev::select_tab(dev::Tab::ShellIntegration),
            )),
    )
}

fn shell_line(label: &'static str, enabled: bool) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .child(
            text(label)
                .w(px(80.0))
                .flex_none()
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(if enabled {
                "Enabled for detected shells"
            } else {
                "Not enabled"
            })
            .flex_1()
            .text_size(px(tokens::TEXT_SMALL))
            .text_color(move || {
                if enabled {
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
            .flex_col()
            .gap(tokens::space(2.0))
            .child(
                text("Pick a folder, see which Node.js version it resolves, then open Terminal with that version first on PATH.")
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().foreground),
            )
            .child(
                text("Resolution walks up from the folder: .node-version, .nvmrc, then package.json engines.node, then the default. The terminal is the developer's; Devbase does not start project servers.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
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
