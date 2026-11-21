//! The runtime terminal page: a folder's Node.js version resolved through
//! devcore's shell walking, then Terminal opened with that version on `PATH`.
//! Ported from `view_runtime_terminal.go` and `internal/devplatform`'s
//! `SelectNode` semantics.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::controls::{self as ui, Tone, action_button, card, field_label},
    store::development as dev,
    theme as tokens,
};

pub(super) fn terminal_page() -> impl IntoElement {
    dev::load();
    view()
        .test_id("dev-terminal")
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .flex_col()
        .child(header())
        .child(super::notes())
        .child(
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
                            .child(folder_card())
                            .child(selection_card()),
                    ),
            ),
        )
}

fn header() -> impl IntoElement {
    ui::page_header_with_leading(
        Some(action_button(
            "← Development",
            Tone::Default,
            "terminal-back",
            false,
            dev::close_terminal,
        )),
        "Runtime terminal",
        None::<&str>,
        None::<AnyElement>,
    )
}

fn folder_card() -> impl IntoElement {
    card(
        "Folder",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(
                view()
                    .flex_col()
                    .gap(tokens::space(1.0))
                    .child(field_label("Directory"))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(tokens::space(1.5))
                            .child(view().flex_1().min_w(px(0.0)).child(ui::text_field(
                                "terminal-dir",
                                "/path/to/project",
                                dev::terminal_dir,
                                dev::set_terminal_dir,
                            )))
                            .child(resolve_row()),
                    ),
            )
            .child(
                view()
                    .flex_col()
                    .gap(tokens::space(1.0))
                    .child(field_label("Version override"))
                    .child(dynamic(|| Some(overrides().into_element()))),
            )
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
    view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(tokens::space(1.0))
        .child(ui::chip(
            "Default",
            || dev::override_spec().is_none(),
            "terminal-override-default",
            false,
            || dev::set_override_spec(None),
        ))
        .child(overrides.into_iter().map(move |node| {
            let version = node.version.clone();
            let spec = node.version.clone();
            let selected_version = node.version.clone();
            ui::chip(
                version,
                move || dev::override_spec().as_deref() == Some(selected_version.as_str()),
                format!("terminal-override-{}", node.version),
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
            "Resolve",
            Tone::Primary,
            "terminal-resolve",
            dev::selection_loading,
            dev::resolve,
        ))
        .child(when(dev::selection_loading, || {
            text("Resolving…")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
}

fn selection_card() -> impl IntoElement {
    card(
        "Selected runtime",
        dynamic(|| Some(selection_body().into_element())),
    )
}

fn selection_body() -> impl IntoElement {
    let selection = dev::selection();
    let has_selection = selection.is_some();
    view()
        .test_id("terminal-selection")
        .flex_col()
        .gap(tokens::space(2.5))
        .child(when(!has_selection, || {
            text("No folder resolved")
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
                        .tabular_nums()
                        .text_color(move || theme().foreground),
                )
                .child(ui::kv_mono("Folder", dir))
                .child(ui::kv_mono(
                    "Source",
                    if source.is_empty() {
                        "default".to_string()
                    } else {
                        source
                    },
                ))
                .child(ui::kv_mono("PATH", path))
                .child(when(selection.system, || {
                    text("System Node.js selected; PATH unchanged.")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(tokens::warning)
                }))
        }))
        .child(view().flex_row().items_center().child(action_button(
            "Open Terminal",
            Tone::Primary,
            "terminal-launch",
            move || !has_selection || dev::is_busy("launch"),
            dev::launch_terminal,
        )))
}
