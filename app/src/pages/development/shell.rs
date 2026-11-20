//! The Shell integration tab: what the Devbase block does in each installed
//! shell, whether Node.js and Java are applied, and the apply/remove actions
//! with the Go confirmations. The same tab carries the Bash environment
//! template the Go terminal page copied.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{store::development as dev, theme as tokens};

use super::ui::{TagTone, Tone, action_button, card, field_label, hairline, tag};

pub(super) fn shell_integration() -> impl IntoElement {
    dev::load();
    dynamic(|| Some(shell_body().into_element()))
}

fn shell_body() -> impl IntoElement {
    scroll().flex_1().min_h(px(0.0)).child(
        view()
            .flex_col()
            .gap(tokens::space(4.0))
            .px(tokens::space(5.0))
            .py(tokens::space(4.0))
            .child(shells_card())
            .child(runtime_card("node"))
            .child(runtime_card("java")),
    )
}

fn shells_card() -> impl IntoElement {
    let shells = dev::shells();
    let empty = shells.is_empty();
    let mut rows: Vec<AnyElement> = Vec::new();
    for row in shells {
        if !rows.is_empty() {
            rows.push(hairline().into_element());
        }
        rows.push(shell_row(row).into_element());
    }
    card(
        "Shell startup files",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(
                text("Devbase edits only its own marked block. It never evaluates a startup file.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            )
            .child(when(empty, || {
                text("No supported shell was found.")
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().muted_foreground)
            }))
            .child(rows),
    )
}

fn shell_row(row: dev::ShellRow) -> impl IntoElement {
    let label = row.label.clone();
    let files = row.files.clone();
    let available = row.available;
    let node = shell_state(row.node_applied, available);
    let java = shell_state(row.java_applied, available);
    view()
        .test_id(&format!("shell-row-{}", row.shell))
        .flex_col()
        .gap(tokens::space(1.0))
        .py(tokens::space(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(
                    text(label)
                        .text_size(px(tokens::TEXT_BODY))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || theme().foreground),
                )
                .child(if available {
                    tag("Installed", TagTone::Neutral)
                } else {
                    tag("Not installed", TagTone::Warning)
                })
                .child(status_text("Node.js", node, row.node_applied))
                .child(status_text("Java", java, row.java_applied)),
        )
        .child(
            text(files.join(", "))
                .font_family("monospace")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .selectable(true),
        )
}

fn shell_state(applied: bool, available: bool) -> &'static str {
    if applied {
        "applied"
    } else if available {
        "not applied"
    } else {
        "not installed"
    }
}

fn status_text(name: &'static str, state: &'static str, applied: bool) -> impl IntoElement {
    let text_label = format!("{name} {state}");
    view()
        .flex_row()
        .items_center()
        .gap(tokens::space(1.0))
        .child(
            view()
                .size(px(6.0))
                .flex_none()
                .rounded(px(3.0))
                .bg(move || {
                    if applied {
                        tokens::success()
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .child(
            text(text_label)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || {
                    if applied {
                        tokens::success()
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
}

fn runtime_card(runtime: &'static str) -> impl IntoElement {
    let enabled = if runtime == "java" {
        dev::java_shell_enabled()
    } else {
        dev::node_shell_enabled()
    };
    let title = if runtime == "java" {
        "Java in the terminal"
    } else {
        "Node.js in the terminal"
    };
    let enable = format!("shell-enable-{runtime}");
    let remove = format!("shell-remove-{runtime}");
    let copy = format!("dev-copy-bash-{runtime}");
    let description = if runtime == "java" {
        "Configures installed zsh, Bash and fish, including non-interactive commands. Each folder follows .java-version and .sdkmanrc, else the default."
    } else {
        "Configures installed zsh, Bash and fish, including non-interactive commands. Each folder runs its declared Node.js version, else the default."
    };
    let copy_bash = move || dev::copy_bash(runtime);
    card(
        title,
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(
                text(description)
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(tokens::space(1.5))
                            .child(view().size(px(8.0)).flex_none().rounded(px(4.0)).bg(
                                move || {
                                    if enabled {
                                        tokens::success()
                                    } else {
                                        theme().muted_foreground.opacity(0.5)
                                    }
                                },
                            ))
                            .child(
                                text(if enabled {
                                    "Enabled for detected shells"
                                } else {
                                    "Not enabled"
                                })
                                .text_size(px(tokens::TEXT_SMALL))
                                .text_color(move || {
                                    if enabled {
                                        tokens::success()
                                    } else {
                                        theme().muted_foreground
                                    }
                                }),
                            ),
                    )
                    .child(view().flex_1()),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
                    .child(action_button(
                        if enabled {
                            "Re-apply in terminal"
                        } else {
                            "Enable in terminal"
                        },
                        if enabled {
                            Tone::Default
                        } else {
                            Tone::Primary
                        },
                        enable,
                        dev::is_busy(&format!("{runtime}-shell")),
                        move || dev::request_runtime_shell(runtime, true),
                    ))
                    .child(action_button(
                        "Disable in terminal",
                        Tone::Default,
                        remove,
                        dev::is_busy(&format!("{runtime}-shell")),
                        move || dev::request_runtime_shell(runtime, false),
                    ))
                    .child(action_button(
                        "Copy Bash command",
                        Tone::Default,
                        copy,
                        false,
                        copy_bash,
                    )),
            )
            .child(field_label("BASH_ENV template"))
            .child(
                text(dev::bash_command)
                    .font_family("monospace")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
                    .selectable(true),
            )
            .child(
                text("Start a new terminal or AI session after changing this.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            ),
    )
}
