//! The defaults pickers on the Overview tab: the version each runtime runs
//! outside a folder that asks for another. This is the Go "Set default" action
//! for Node.js, Java and uv's global pin, with the resolution rules stated
//! beside them.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::controls::{self as ui, TagTone, Tone, action_button, card, tag},
    store::development as dev,
    theme as tokens,
};

pub(super) fn node_defaults() -> impl IntoElement {
    card(
        "Node.js default",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(
                text(move || {
                    let version = dev::node_default_version();
                    if version.is_empty() {
                        "No default Node.js version".to_string()
                    } else {
                        format!("Each folder runs Node.js {version} unless it asks for another.")
                    }
                })
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
            )
            .child(
                text("A folder overrides the default with .node-version, .nvmrc or package.json engines.node. The nearest file wins.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            )
            .child(dynamic(|| Some(node_chips().into_element())))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(1.5))
                    .child(action_button(
                        "Open runtime terminal",
                        Tone::Default,
                        "defaults-open-terminal",
                        false,
                        dev::open_terminal,
                    ))
                    .child(
                        text("Check a folder's version against these rules.")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground),
                    ),
            ),
    )
}

fn node_chips() -> impl IntoElement {
    let nodes = dev::node_versions();
    let empty = nodes.is_empty();
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(when(empty, || {
            text("No Node.js versions installed. Install one in Software.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.0))
                .child(nodes.into_iter().map(move |node| {
                    let id = node.id.clone();
                    let selected_id = node.id.clone();
                    ui::chip(
                        format!("Node.js {}", node.version),
                        move || dev::node_default_id() == selected_id,
                        format!("defaults-node-{}", node.id),
                        || dev::is_busy("node-default"),
                        move || dev::set_node_default(&id),
                    )
                })),
        )
}

pub(super) fn java_defaults() -> impl IntoElement {
    card(
        "Java default",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(
                text(move || {
                    let version = dev::java_default_version();
                    if version.is_empty() {
                        "No default JDK".to_string()
                    } else {
                        format!("Each folder runs JDK {version} unless .java-version or .sdkmanrc asks for another.")
                    }
                })
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
            )
            .child(dynamic(|| Some(java_chips().into_element())))
            .child(
                text("Enable terminal integration in the Shell integration tab to use the default.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            ),
    )
}

fn java_chips() -> impl IntoElement {
    let java = dev::java_versions();
    let empty = java.is_empty();
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(when(empty, || {
            text("No JDKs installed. Install one from the Software tab.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.0))
                .child(java.into_iter().map(move |jdk| {
                    let id = jdk.id.clone();
                    let selected_id = jdk.id.clone();
                    ui::chip(
                        if jdk.product.is_empty() {
                            format!("Java {}", jdk.version)
                        } else {
                            format!("{} {}", jdk.product, jdk.version)
                        },
                        move || dev::java_default_id() == selected_id,
                        format!("defaults-java-{}", jdk.id),
                        || dev::is_busy("java-default"),
                        move || dev::set_java_default(&id),
                    )
                })),
        )
}

pub(super) fn python_defaults() -> impl IntoElement {
    card(
        "Python default (uv)",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(
                text(move || {
                    let pin = dev::python_pin();
                    if pin.is_empty() {
                        "No uv global pin".to_string()
                    } else {
                        format!("uv pins Python {pin} globally.")
                    }
                })
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
            )
            .child(dynamic(|| Some(python_chips().into_element())))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(1.5))
                    .child(tag("uv manages its own PATH", TagTone::Neutral))
                    .child(
                        text("Install or uninstall Python versions in Runtimes.")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground),
                    ),
            ),
    )
}

fn python_chips() -> impl IntoElement {
    let python = dev::python_versions();
    let empty = python.is_empty();
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(when(empty, || {
            text("No Python versions installed by uv.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.0))
                .child(python.into_iter().map(move |row| {
                    let key = row.key.clone();
                    let selected = row.pin;
                    ui::chip(
                        row.version.clone(),
                        selected,
                        format!("defaults-python-{}", row.key),
                        || dev::is_busy("python-default"),
                        move || dev::set_python_default(&key),
                    )
                })),
        )
}
