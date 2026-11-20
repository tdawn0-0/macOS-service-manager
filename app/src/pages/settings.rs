//! Settings: the privileged helper, macOS permissions, the workspace
//! preferences and the public `devbase` CLI wrapper, ported from
//! `view_settings.go` and `settings.go`.
//!
//! The page reads [`crate::store::system`]. Helper installs and removals run
//! in background tasks and surface their busy line, status and verbatim error;
//! the single administrator prompt happens inside devcore's injected
//! `AdminRunner`, never in this page. NSH components run once, so each card's
//! body is a `dynamic` region bound to the store it reads.

use devcore::helper::{HELPER_DIR, PLIST_PATH, SOCKET_PATH};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    store::{Section, system},
    theme as tokens,
};

use super::diagnostics::ui;

pub(crate) fn settings() -> impl IntoElement {
    system::load_settings();

    view()
        .test_id("settings-page")
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .child(dynamic(|| Some(header().into_element())))
        .child(
            scroll().flex_1().min_h(px(0.0)).child(dynamic(|| {
                Some(
                    view()
                        .w_full()
                        .max_w(px(880.0))
                        .flex_col()
                        .gap(tokens::space(4.0))
                        .px(tokens::space(6.0))
                        .py(tokens::space(5.0))
                        .children(
                            system::settings_error()
                                .map(|error| ui::note(error, true)),
                        )
                        .children(
                            system::settings_action_error()
                                .map(|error| ui::note(error, true)),
                        )
                        .children(
                            system::settings_notice()
                                .map(|notice| ui::note(notice, false)),
                        )
                        .child(ui::card(
                            "Privileged helper",
                            dynamic(|| Some(helper_body().into_element())),
                        ))
                        .child(ui::card(
                            "macOS permissions",
                            dynamic(|| Some(permissions_body().into_element())),
                        ))
                        .child(ui::card(
                            "Workspace",
                            dynamic(|| Some(workspace_body().into_element())),
                        ))
                        .child(ui::card(
                            "Command-line interface",
                            dynamic(|| Some(cli_body().into_element())),
                        ))
                        .into_element(),
                )
            })),
        )
}

fn header() -> impl IntoElement {
    let busy = system::settings_busy();
    view()
        .h(px(44.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(3.5))
        .border_b_1()
        .border_color(move || tokens::hairline())
        .child(
            text("Settings")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            text(Section::Settings.summary())
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .truncate(),
        )
        .child(view().flex_1())
        .child(when(busy, || {
            text("Loading")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
        .child(ui::action_button(
            "Refresh",
            ui::Tone::Default,
            "settings-refresh",
            busy,
            system::refresh_settings,
        ))
}

fn helper_body() -> impl IntoElement {
    let helper = system::helper_status();
    let busy = system::helper_busy();
    let activity = system::helper_activity();
    let settings_busy = system::settings_busy();
    let installed = helper.installed;
    let message = helper.message;
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(ui::status_tag(if installed {
                    "Installed"
                } else {
                    "Not installed"
                }))
                .child(
                    text(message)
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(ui::kv("Daemon", PLIST_PATH))
        .child(ui::kv("Socket", SOCKET_PATH))
        .child(ui::kv("Helper", HELPER_DIR))
        .children(activity.map(|activity| {
            text(activity)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(when(!installed, move || {
                    ui::action_button(
                        "Install helper",
                        ui::Tone::Primary,
                        "settings-install-helper",
                        busy || settings_busy,
                        system::install_helper,
                    )
                }))
                .child(when(installed, move || {
                    ui::action_button(
                        "Remove helper",
                        ui::Tone::Danger,
                        "settings-remove-helper",
                        busy || settings_busy,
                        system::uninstall_helper,
                    )
                }))
                .child(ui::action_button(
                    "Refresh status",
                    ui::Tone::Default,
                    "settings-refresh-helper",
                    settings_busy,
                    system::refresh_settings,
                )),
        )
}

fn permissions_body() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(
            text("Full Disk Access and login items are managed by macOS.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Open Full Disk Access…",
                    ui::Tone::Default,
                    "settings-open-fda",
                    false,
                    system::open_full_disk_access,
                ))
                .child(ui::action_button(
                    "Open Login Items…",
                    ui::Tone::Default,
                    "settings-open-login-items",
                    false,
                    system::open_login_items,
                )),
        )
}

fn workspace_body() -> impl IntoElement {
    let cache = system::cache();
    let cache_busy = system::cache_busy();
    let busy = system::settings_busy();
    let workspace_root = system::workspace_root();
    let rotation = system::log_rotation_enabled();
    let rotation_busy = system::log_rotation_busy() || busy;
    let login = system::login_at_login();
    let login_busy = system::login_busy();
    let cache_text = if cache.files > 0 {
        format!(
            "{} · {}",
            crate::store::software::format_bytes(cache.bytes),
            crate::store::software::plural(cache.files as usize, "archive")
        )
    } else {
        "0 B".to_string()
    };
    let cache_folder = cache.directory;
    let root_for_reveal = workspace_root.clone();
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(ui::switch(
                    "settings-log-rotation",
                    rotation,
                    rotation_busy,
                    move || system::set_log_rotation(!rotation),
                ))
                .child(
                    text("Automatic log rotation")
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().foreground),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(ui::switch(
                    "settings-login-item",
                    login,
                    login_busy,
                    move || system::set_login_item(!login),
                ))
                .child(
                    text("Start at login in menu bar")
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().foreground),
                ),
        )
        .child(when(!workspace_root.is_empty(), {
            let root = workspace_root.clone();
            move || ui::kv("Workspace", root.clone())
        }))
        .child(ui::kv("Download cache", cache_text))
        .child(when(!cache_folder.is_empty(), {
            let folder = cache_folder.clone();
            move || ui::kv("Cache folder", folder.clone())
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Show workspace in Finder",
                    ui::Tone::Default,
                    "settings-reveal-workspace",
                    root_for_reveal.is_empty(),
                    system::reveal_workspace,
                ))
                .child(ui::action_button(
                    "Clear download cache",
                    ui::Tone::Default,
                    "settings-clear-cache",
                    cache_busy || busy || cache.files == 0,
                    system::request_clear_cache,
                ))
                .child(ui::action_button(
                    "Run diagnostics",
                    ui::Tone::Default,
                    "settings-run-diagnostics",
                    busy,
                    || {
                        crate::store::select_section(Section::Diagnostics);
                        system::run_diagnostics();
                    },
                )),
        )
}

fn cli_body() -> impl IntoElement {
    let cli = system::cli_status();
    let busy = system::cli_busy();
    let activity = system::cli_activity();
    let settings_busy = system::settings_busy();
    let installed = cli.installed;
    let managed = cli.managed_by_devbase;
    let wrapper = cli.wrapper_path;
    let in_path = cli.in_path;
    let path_hint = (installed && !in_path && !wrapper.is_empty()).then(|| {
        let dir = std::path::Path::new(&wrapper)
            .parent()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        format!("Add {dir} to PATH to run devbase from any shell.")
    });
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(ui::status_tag(if installed {
                    "Installed"
                } else {
                    "Not installed"
                }))
                .child(
                    text(if installed {
                        "The public devbase command is available."
                    } else {
                        "The public devbase command is not installed."
                    })
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().muted_foreground),
                ),
        )
        .child(when(!wrapper.is_empty(), {
            let wrapper = wrapper.clone();
            move || ui::kv("Command", wrapper.clone())
        }))
        .children(path_hint.map(|hint| ui::note(hint, false)))
        .children(activity.map(|activity| {
            text(activity)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(when(!installed, move || {
                    ui::action_button(
                        "Install devbase CLI",
                        ui::Tone::Primary,
                        "settings-install-cli",
                        busy || settings_busy,
                        system::install_cli,
                    )
                }))
                .child(when(installed && managed, move || {
                    ui::action_button(
                        "Remove devbase CLI",
                        ui::Tone::Danger,
                        "settings-remove-cli",
                        busy || settings_busy,
                        system::uninstall_cli,
                    )
                }))
                .child(ui::action_button(
                    "Copy AI schema command",
                    ui::Tone::Default,
                    "settings-copy-schema",
                    false,
                    system::copy_schema_command,
                )),
        )
}
