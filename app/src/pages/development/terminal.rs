//! The runtime terminal page: a folder's Node.js version resolved through
//! devcore's shell walking, then Terminal opened with that version on `PATH`.
//! Ported from `view_runtime_terminal.go` and `internal/devplatform`'s
//! `SelectNode` semantics.

use nsh_base::{input, theme};
use nsh_core::prelude::*;

use crate::{store::development as dev, theme as tokens};

use super::ui::{self, Tone, action_button, card, field_label};

pub(super) fn terminal_page() -> impl IntoElement {
    dev::load();
    view()
        .test_id("dev-terminal")
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .flex_col()
        .child(header())
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(5.0))
                    .py(tokens::space(4.0))
                    .child(folder_card())
                    .child(dynamic(|| Some(selection_card().into_element()))),
            ),
        )
}

fn header() -> impl IntoElement {
    view()
        .h(px(56.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
        .child(action_button(
            "← Development",
            Tone::Default,
            "terminal-back",
            false,
            dev::close_terminal,
        ))
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text("Runtime terminal")
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(
                    text("Node.js for one folder, in the developer's terminal.")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

fn folder_card() -> impl IntoElement {
    card(
        "Folder",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(field_label("Directory"))
            .child(
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
                            .test_id("terminal-dir")
                            .flex_1()
                            .min_w(px(0.0))
                            .h(px(26.0))
                            .px(tokens::space(2.0))
                            .text_size(px(tokens::TEXT_SMALL))
                            .text_color(move || theme().foreground)
                            .placeholder("Folder to open")
                            .value(dev::terminal_dir)
                            .on_change(|text: &str| dev::set_terminal_dir(text.to_owned())),
                    ),
            )
            .child(
                text("Resolution walks up: .node-version, .nvmrc, package.json engines.node, then the default.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            )
            .child(field_label("Version override"))
            .child(dynamic(|| Some(overrides().into_element())))
            .child(dynamic(|| Some(resolve_row().into_element())))
            .child(dynamic(move || {
                dev::selection_error().map(|message| {
                    text(message)
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().destructive)
                        .selectable(true)
                })
            })),
    )
}

fn overrides() -> impl IntoElement {
    let overrides = dev::node_versions();
    let current = dev::override_spec();
    view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(tokens::space(1.0))
        .child(ui::chip(
            "Default",
            "terminal-override-default",
            current.is_none(),
            false,
            || dev::set_override_spec(None),
        ))
        .child(overrides.into_iter().map(move |node| {
            let version = node.version.clone();
            let spec = node.version.clone();
            let selected = current.as_deref() == Some(node.version.as_str());
            ui::chip(
                version,
                format!("terminal-override-{}", node.version),
                selected,
                false,
                move || dev::set_override_spec(Some(spec.clone())),
            )
        }))
}

fn resolve_row() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(tokens::space(1.5))
        .child(action_button(
            "Resolve version",
            Tone::Primary,
            "terminal-resolve",
            dev::selection_loading(),
            dev::resolve,
        ))
        .child(when(dev::selection_loading(), || {
            text("Resolving…")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
}

fn selection_card() -> impl IntoElement {
    let selection = dev::selection();
    let has_selection = selection.is_some();
    card(
        "Selected runtime",
        view()
            .test_id("terminal-selection")
            .flex_col()
            .gap(tokens::space(2.0))
            .child(when(!has_selection, || {
                text("Resolve a folder to see its Node.js version.")
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().muted_foreground)
            }))
            .child(selection.map(|selection| {
                let version = if selection.version.is_empty() {
                    "system Node.js".to_string()
                } else {
                    selection.version.clone()
                };
                let source = selection.source.clone();
                let dir = dev::shorten_path(&selection.dir);
                let path = selection.path.clone();
                view()
                    .flex_col()
                    .gap(tokens::space(1.5))
                    .child(
                        text(format!("Node.js {version}"))
                            .text_size(px(tokens::TEXT_BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(move || theme().foreground),
                    )
                    .child(kv("Folder", dir))
                    .child(kv(
                        "Asked for by",
                        if source.is_empty() {
                            "the default".to_string()
                        } else {
                            source
                        },
                    ))
                    .child(kv("PATH prefix", path))
                    .child(when(selection.system, || {
                        text("This folder asks for the system Node.js; Devbase leaves PATH unchanged.")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(tokens::warning)
                    }))
            }))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
                    .child(action_button(
                        "Open Terminal",
                        Tone::Primary,
                        "terminal-launch",
                        !has_selection || dev::is_busy("launch"),
                        dev::launch_terminal,
                    ))
                    .child(
                        text("Terminal opens in the folder with this runtime first on PATH. Project servers stay yours to start.")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground),
                    ),
            ),
    )
}

fn kv(label: &'static str, value: String) -> impl IntoElement {
    view()
        .flex_row()
        .items_start()
        .gap(tokens::space(2.0))
        .child(
            text(label)
                .w(px(96.0))
                .flex_none()
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
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
