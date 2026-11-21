//! The Shell integration tab: what the Devbase block does in each installed
//! shell, whether Node.js and Java are applied, and the apply/remove actions
//! with the Go confirmations. The same tab carries the Bash environment
//! template the Go terminal page copied.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::controls::{
        self as ui, TagTone, Tone, action_button, card, field_label, hairline, tag,
    },
    store::development as dev,
    theme as tokens,
};

pub(super) fn shell_integration() -> impl IntoElement {
    dev::load();
    scroll().flex_1().min_h(px(0.0)).child(
        view()
            .flex_col()
            .gap(tokens::space(4.0))
            .px(tokens::space(5.0))
            .py(tokens::space(4.0))
            .child(shells_card())
            .child(ca_env_card())
            .child(runtime_card("node"))
            .child(runtime_card("java")),
    )
}

fn shells_card() -> impl IntoElement {
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
            .child(dynamic(|| Some(shells_list().into_element()))),
    )
}

fn shells_list() -> impl IntoElement {
    let shells = dev::shells();
    let empty = shells.is_empty();
    let mut rows: Vec<AnyElement> = Vec::new();
    for row in shells {
        if !rows.is_empty() {
            rows.push(hairline().into_element());
        }
        rows.push(shell_row(row).into_element());
    }
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(when(empty, || {
            text("No supported shell was found.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(rows)
}

fn shell_row(row: dev::ShellRow) -> impl IntoElement {
    let label = row.label.clone();
    let files = row.files.clone();
    let available = row.available;
    let node = shell_state(row.node_applied, available);
    let java = shell_state(row.java_applied, available);
    let ca_env = shell_state(row.ca_env_applied, available);
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
                .child(status_text("Java", java, row.java_applied))
                .child(status_text("Local CA", ca_env, row.ca_env_applied)),
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

fn ca_env_card() -> impl IntoElement {
    view().test_id("shell-ca-env-card").child(card(
        "Local HTTPS CA environment",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(
                text("Node.js, Python, and curl do not automatically read macOS Keychain trust roots. Enabling this writes a reversible Devbase block in zsh, Bash, and fish startup files so local HTTPS requests trust the Devbase local CA.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(2.0))
                    .child(ui::switch(
                        "shell-ca-env-switch",
                        dev::ca_env_enabled,
                        || {
                            dev::is_busy("ca-env-shell")
                                || (!dev::ca_cert_exists() && !dev::ca_env_enabled())
                        },
                        "Local HTTPS CA environment",
                        dev::set_ca_env_shell,
                    ))
                    .child(
                        text("Export local CA environment variables in detected shells")
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(tokens::TEXT_SMALL))
                            .text_color(move || theme().foreground),
                    )
                    .child(dynamic(|| {
                        let enabled = dev::ca_env_enabled();
                        let exists = dev::ca_cert_exists();
                        Some(if enabled {
                            tag("Enabled", TagTone::Success).into_element()
                        } else if !exists {
                            tag("CA not created", TagTone::Warning).into_element()
                        } else {
                            tag("Not enabled", TagTone::Neutral).into_element()
                        })
                    })),
            )
            .child(when(
                || !dev::ca_cert_exists(),
                || {
                    text("Local CA certificate not found yet. Create or trust the local CA in HTTPS Proxy first.")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(tokens::warning)
                },
            ))
            .child(field_label("Generated shell exports (zsh / Bash)"))
            .child(
                ui::inset(
                    text(dev::ca_env_preview)
                        .test_id("shell-ca-env-preview")
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().foreground)
                        .selectable(true),
                ),
            )
            .child(dynamic(|| {
                let fish_available = dev::shells()
                    .iter()
                    .any(|row| row.shell == "fish" && row.available);
                let preview = dev::ca_env_preview_fish();
                (fish_available && !preview.is_empty()).then(move || {
                    view()
                        .flex_col()
                        .gap(tokens::space(1.5))
                        .child(field_label("Generated shell exports (fish)"))
                        .child(ui::inset(
                            text(preview)
                                .test_id("shell-ca-env-preview-fish")
                                .font_family("monospace")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().foreground)
                                .selectable(true),
                        ))
                        .into_element()
                })
            }))
            .child(
                text("Start a new terminal or AI session after changing this.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground),
            ),
    ))
}

fn runtime_card(runtime: &'static str) -> impl IntoElement {
    let title = if runtime == "java" {
        "Java in the terminal"
    } else {
        "Node.js in the terminal"
    };
    let description = if runtime == "java" {
        "Configures installed zsh, Bash and fish, including non-interactive commands. Each folder follows .java-version and .sdkmanrc, else the default."
    } else {
        "Configures installed zsh, Bash and fish, including non-interactive commands. Each folder runs its declared Node.js version, else the default."
    };
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
            .child(dynamic(move || {
                Some(runtime_controls(runtime).into_element())
            }))
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

fn runtime_controls(runtime: &'static str) -> impl IntoElement {
    let enabled = if runtime == "java" {
        dev::java_shell_enabled()
    } else {
        dev::node_shell_enabled()
    };
    let enable = format!("shell-enable-{runtime}");
    let remove = format!("shell-remove-{runtime}");
    let copy = format!("dev-copy-bash-{runtime}");
    let busy_key = format!("{runtime}-shell");
    let busy_enable = busy_key.clone();
    let copy_bash = move || dev::copy_bash(runtime);
    view()
        .flex_col()
        .gap(tokens::space(2.0))
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
                        .child(
                            view()
                                .size(px(8.0))
                                .flex_none()
                                .rounded(px(4.0))
                                .bg(move || {
                                    if enabled {
                                        tokens::success()
                                    } else {
                                        theme().muted_foreground.opacity(0.5)
                                    }
                                }),
                        )
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
                    move || dev::is_busy(&busy_enable),
                    move || dev::request_runtime_shell(runtime, true),
                ))
                .child(action_button(
                    "Disable in terminal",
                    Tone::Default,
                    remove,
                    move || dev::is_busy(&busy_key),
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
}
