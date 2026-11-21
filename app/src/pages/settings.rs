//! Settings: workspace preferences, privileged helper, public `devbase` CLI,
//! macOS permissions, and updates/about metadata.
//!
//! The page reads [`crate::store::system`]. Helper installs and removals run
//! in background tasks and surface their busy line, status and verbatim error;
//! the single administrator prompt happens inside devcore's injected
//! `AdminRunner`, never in this page. The scroll container and cards mount
//! once, with scoped `dynamic` regions only for banners and reactive card
//! content so notices or busy toggles never tear down the cards or reset scroll.

use devcore::helper::{HELPER_DIR, PLIST_PATH, SOCKET_PATH};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::controls::{self as ui, Tone},
    meta,
    store::{Section, system},
    theme as tokens,
    updater::UpdateState,
};

pub(crate) fn settings() -> impl IntoElement {
    system::load_settings();

    view()
        .test_id("settings-page")
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .child(header())
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .w_full()
                    .max_w(px(760.0))
                    .flex_col()
                    .gap(tokens::space(5.0))
                    .px(tokens::space(6.0))
                    .py(tokens::space(5.0))
                    .child(dynamic(|| {
                        system::settings_error().map(|error| ui::plain_note(error, true))
                    }))
                    .child(dynamic(|| {
                        system::settings_action_error().map(|error| ui::plain_note(error, true))
                    }))
                    .child(settings_group("Workspace", workspace_body()))
                    .child(settings_group(
                        "Privileged helper",
                        dynamic(|| Some(helper_body().into_element())),
                    ))
                    .child(settings_group(
                        "Command-line interface",
                        dynamic(|| Some(cli_body().into_element())),
                    ))
                    .child(settings_group("Permissions", permissions_body()))
                    .child(
                        view()
                            .test_id("settings-updates-card")
                            .w_full()
                            .child(settings_group(
                                "Updates & About",
                                dynamic(|| Some(updates_body().into_element())),
                            )),
                    ),
            ),
        )
}

fn header() -> impl IntoElement {
    ui::page_header(
        "Settings",
        None::<&str>,
        view()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(tokens::space(2.0))
            .child(when(system::settings_busy, || {
                text("Loading")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
            }))
            .child(ui::action_button(
                "Refresh",
                Tone::Default,
                "settings-refresh",
                system::settings_busy,
                system::refresh_settings,
            )),
    )
}

fn settings_group(title: &'static str, body: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(
            text(title)
                .px(tokens::space(1.0))
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_col()
                .rounded(tokens::panel_radius)
                .bg(tokens::panel)
                .border_1()
                .border_color(tokens::hairline)
                .overflow_hidden()
                .child(body),
        )
}

fn settings_row(leading: impl IntoElement, trailing: impl IntoElement) -> impl IntoElement {
    view()
        .min_h(px(40.0))
        .flex_row()
        .items_center()
        .justify_between()
        .gap(tokens::space(3.0))
        .px(tokens::space(4.0))
        .py(tokens::space(2.25))
        .child(view().flex_1().min_w(px(0.0)).child(leading))
        .child(
            view()
                .flex_none()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(trailing),
        )
}

fn row_title(label: &'static str) -> impl IntoElement {
    text(label)
        .text_size(px(tokens::TEXT_BODY))
        .text_color(move || theme().foreground)
}

fn workspace_body() -> impl IntoElement {
    view()
        .flex_col()
        .child(settings_row(
            row_title("Automatic log rotation"),
            ui::switch(
                "settings-log-rotation",
                system::log_rotation_enabled,
                || system::log_rotation_busy() || system::settings_busy(),
                "Automatic log rotation",
                system::set_log_rotation,
            ),
        ))
        .child(ui::hairline())
        .child(settings_row(
            row_title("Start at login in menu bar"),
            ui::switch(
                "settings-login-item",
                system::login_at_login,
                system::login_busy,
                "Start at login in menu bar",
                system::set_login_item,
            ),
        ))
        .child(dynamic(|| Some(workspace_details().into_element())))
}

fn workspace_details() -> impl IntoElement {
    let cache = system::cache();
    let cache_busy = system::cache_busy();
    let busy = system::settings_busy();
    let workspace_root = system::workspace_root();
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
        .child(ui::hairline())
        .child(settings_row(
            view()
                .flex_col()
                .gap(px(2.0))
                .child(row_title("Workspace folder"))
                .child(when(!workspace_root.is_empty(), {
                    let root = workspace_root.clone();
                    move || {
                        text(root.clone())
                            .font_family("monospace")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                            .selectable(true)
                            .truncate()
                    }
                })),
            ui::action_button(
                "Reveal",
                Tone::Default,
                "settings-reveal-workspace",
                root_for_reveal.is_empty(),
                system::reveal_workspace,
            ),
        ))
        .child(ui::hairline())
        .child(settings_row(
            view()
                .flex_col()
                .gap(px(2.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.0))
                        .child(row_title("Download cache"))
                        .child(
                            text(cache_text)
                                .text_size(px(tokens::TEXT_SMALL))
                                .tabular_nums()
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(when(!cache_folder.is_empty(), {
                    let folder = cache_folder.clone();
                    move || {
                        text(folder.clone())
                            .font_family("monospace")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                            .selectable(true)
                            .truncate()
                    }
                })),
            ui::action_button(
                "Clear",
                Tone::Default,
                "settings-clear-cache",
                cache_busy || busy || cache.files == 0,
                system::request_clear_cache,
            ),
        ))
        .child(ui::hairline())
        .child(settings_row(
            row_title("Diagnostics"),
            ui::action_button(
                "Run diagnostics",
                Tone::Default,
                "settings-run-diagnostics",
                busy,
                || {
                    crate::store::select_section(Section::Diagnostics);
                    system::run_diagnostics();
                },
            ),
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
        .child(settings_row(
            view()
                .flex_col()
                .gap(tokens::space(1.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.0))
                        .child(row_title("Status"))
                        .child(ui::status_tag(if installed {
                            "Installed"
                        } else {
                            "Not installed"
                        })),
                )
                .child(when(!message.is_empty(), {
                    let message = message.clone();
                    move || {
                        text(message.clone())
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    }
                }))
                .children(activity.map(|activity| {
                    text(activity)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                })),
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(when(!installed, move || {
                    ui::action_button(
                        "Install helper",
                        Tone::Primary,
                        "settings-install-helper",
                        busy || settings_busy,
                        system::install_helper,
                    )
                }))
                .child(when(installed, move || {
                    ui::action_button(
                        "Remove helper",
                        Tone::Danger,
                        "settings-remove-helper",
                        busy || settings_busy,
                        system::uninstall_helper,
                    )
                }))
                .child(ui::action_button(
                    "Refresh",
                    Tone::Default,
                    "settings-refresh-helper",
                    settings_busy,
                    system::refresh_settings,
                )),
        ))
        .child(ui::hairline())
        .child(
            view()
                .flex_col()
                .gap(tokens::space(1.5))
                .px(tokens::space(4.0))
                .py(tokens::space(2.5))
                .child(ui::kv_mono("Daemon", PLIST_PATH))
                .child(ui::kv_mono("Socket", SOCKET_PATH))
                .child(ui::kv_mono("Helper", HELPER_DIR)),
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
        .child(settings_row(
            view()
                .flex_col()
                .gap(tokens::space(1.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.0))
                        .child(row_title("devbase"))
                        .child(ui::status_tag(if installed {
                            "Installed"
                        } else {
                            "Not installed"
                        })),
                )
                .child(when(!wrapper.is_empty(), {
                    let wrapper = wrapper.clone();
                    move || {
                        text(wrapper.clone())
                            .font_family("monospace")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                            .selectable(true)
                            .truncate()
                    }
                }))
                .children(activity.map(|activity| {
                    text(activity)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                })),
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(when(!installed, move || {
                    ui::action_button(
                        "Install CLI",
                        Tone::Primary,
                        "settings-install-cli",
                        busy || settings_busy,
                        system::install_cli,
                    )
                }))
                .child(when(installed && managed, move || {
                    ui::action_button(
                        "Remove CLI",
                        Tone::Danger,
                        "settings-remove-cli",
                        busy || settings_busy,
                        system::uninstall_cli,
                    )
                })),
        ))
        .children(path_hint.map(|hint| {
            view()
                .px(tokens::space(4.0))
                .pb(tokens::space(2.5))
                .child(ui::plain_note(hint, false))
        }))
        .child(ui::hairline())
        .child(settings_row(
            row_title("Agent & MCP"),
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Copy schema",
                    Tone::Default,
                    "settings-copy-schema",
                    false,
                    system::copy_schema_command,
                ))
                .child(ui::action_button(
                    "Copy agent prompt",
                    Tone::Default,
                    "settings-copy-agent-prompt",
                    false,
                    system::copy_agent_instructions,
                ))
                .child(ui::action_button(
                    "Copy MCP config",
                    Tone::Default,
                    "settings-copy-mcp-config",
                    false,
                    system::copy_mcp_config,
                )),
        ))
}

fn permissions_body() -> impl IntoElement {
    view()
        .flex_col()
        .child(settings_row(
            row_title("Full Disk Access"),
            ui::action_button(
                "Open…",
                Tone::Default,
                "settings-open-fda",
                false,
                system::open_full_disk_access,
            ),
        ))
        .child(ui::hairline())
        .child(settings_row(
            row_title("Login Items"),
            ui::action_button(
                "Open…",
                Tone::Default,
                "settings-open-login-items",
                false,
                system::open_login_items,
            ),
        ))
}

fn updates_body() -> impl IntoElement {
    let feed = system::update_feed();
    let state = system::update_state();
    let busy = system::updates_busy();

    let (status_tag, status_summary): (&'static str, Option<String>) = match &state {
        UpdateState::NotConfigured => (
            "Not configured",
            Some("No signed update feed is bundled with this build.".to_string()),
        ),
        UpdateState::Idle => ("Ready", None),
        UpdateState::Checking => ("Checking", Some("Checking for signed updates…".to_string())),
        UpdateState::UpToDate => (
            "Up to date",
            Some(format!(
                "Devbase {} is the latest verified version.",
                meta::VERSION
            )),
        ),
        UpdateState::Available(avail) => (
            "Update available",
            Some(format!(
                "Version {} is available and verified.",
                avail.version
            )),
        ),
        UpdateState::Installing { version } => (
            "Installing",
            Some(format!("Downloading and verifying Devbase {version}…")),
        ),
        UpdateState::Installed { version } => (
            "Installed",
            Some(format!(
                "Devbase {version} is installed. Restart to apply the update."
            )),
        ),
        UpdateState::Error(_) => ("Error", None),
    };

    let available = match &state {
        UpdateState::Available(avail) => Some(avail.clone()),
        _ => None,
    };
    let installed = matches!(state, UpdateState::Installed { .. });
    let error_note = match &state {
        UpdateState::Error(error) => Some(error.clone()),
        _ => None,
    };
    let notes = available
        .as_ref()
        .and_then(|avail| avail.notes.clone())
        .filter(|text| !text.trim().is_empty());
    let install_label = available.as_ref().map(|avail| {
        if avail.can_install {
            "Install update"
        } else {
            "Download update"
        }
    });

    view()
        .flex_col()
        .child(settings_row(
            text(format!(
                "{} v{} · macOS {}+ · {}",
                meta::NAME,
                meta::VERSION,
                meta::MINIMUM_MACOS,
                meta::IDENTIFIER
            ))
            .text_size(px(tokens::TEXT_SMALL))
            .tabular_nums()
            .text_color(move || theme().foreground)
            .selectable(true),
            ui::status_tag(status_tag),
        ))
        .child(ui::hairline())
        .child(settings_row(
            view()
                .flex_col()
                .gap(px(2.0))
                .child(row_title("Software updates"))
                .children(status_summary.map(|summary| {
                    text(summary)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                })),
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Check for updates",
                    Tone::Default,
                    "settings-check-updates",
                    busy,
                    system::check_for_updates,
                ))
                .children(install_label.map(|label| {
                    ui::action_button(
                        label,
                        Tone::Primary,
                        "settings-install-update",
                        busy,
                        system::install_available_update,
                    )
                }))
                .child(when(installed, || {
                    ui::action_button(
                        "Restart Devbase",
                        Tone::Primary,
                        "settings-restart-update",
                        false,
                        system::relaunch_after_update,
                    )
                })),
        ))
        .child(ui::hairline())
        .child(
            view()
                .flex_col()
                .gap(tokens::space(1.5))
                .px(tokens::space(4.0))
                .py(tokens::space(2.5))
                .child(ui::kv("Channel", feed.channel))
                .children(
                    feed.source
                        .filter(|source| !source.is_empty())
                        .map(|source| ui::kv_mono("Feed", source)),
                )
                .children(notes.map(|notes| {
                    view()
                        .test_id("settings-update-notes")
                        .flex_col()
                        .gap(tokens::space(1.0))
                        .p(tokens::space(2.5))
                        .rounded(move || theme().radius)
                        .bg(tokens::inset)
                        .border_1()
                        .border_color(tokens::hairline)
                        .child(
                            text("Release notes")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(
                            text(notes)
                                .text_size(px(tokens::TEXT_SMALL))
                                .text_color(move || theme().foreground),
                        )
                }))
                .children(error_note.map(|error| ui::plain_note(error, true))),
        )
}
