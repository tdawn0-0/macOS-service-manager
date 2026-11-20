//! The defaults pickers on the Overview tab: the version each runtime runs
//! outside a folder that asks for another. This is the Go "Set default" action
//! for Node.js, Java and uv's global pin, with the resolution rules stated
//! beside them.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{store::development as dev, theme as tokens};

use super::ui::{self, TagTone, Tone, action_button, card, tag};

pub(super) fn node_defaults() -> impl IntoElement {
    let nodes = dev::node_versions();
    let current = dev::node_default_id();
    let empty = nodes.is_empty();
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
                        let selected = node.id == current;
                        ui::chip(
                            format!("Node.js {}", node.version),
                            format!("defaults-node-{}", node.id),
                            selected,
                            dev::is_busy("node-default"),
                            move || dev::set_node_default(&id),
                        )
                    })),
            )
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

pub(super) fn java_defaults() -> impl IntoElement {
    let java = dev::java_versions();
    let current = dev::java_default_id();
    let empty = java.is_empty();
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
                        let selected = jdk.id == current;
                        ui::chip(
                            if jdk.product.is_empty() {
                                format!("Java {}", jdk.version)
                            } else {
                                format!("{} {}", jdk.product, jdk.version)
                            },
                            format!("defaults-java-{}", jdk.id),
                            selected,
                            dev::is_busy("java-default"),
                            move || dev::set_java_default(&id),
                        )
                    })),
            )
            .child(
                text("Enable terminal integration in the Shell integration tab to use the default.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            ),
    )
}

pub(super) fn python_defaults() -> impl IntoElement {
    let python = dev::python_versions();
    let empty = python.is_empty();
    let pin = dev::python_pin();
    card(
        "Python default (uv)",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(
                text(if pin.is_empty() {
                    "No uv global pin".to_string()
                } else {
                    format!("uv pins Python {pin} globally.")
                })
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
            )
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
                            format!("defaults-python-{}", row.key),
                            selected,
                            dev::is_busy("python-default"),
                            move || dev::set_python_default(&key),
                        )
                    })),
            )
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
