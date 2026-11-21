//! One instance's detail: its state and lifecycle actions, the connection
//! panel, the endpoint and storage facts, its databases and users, its
//! backups and restores, and the two ways to remove it
//! (`view_databases.go`, `view_database_connections.go`).

use devcore::{
    databases::BackupFile,
    databases::inventory::{is_protected_database, is_protected_database_user},
    model::BackupCadence,
};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{
        controls::{self as ui, TagTone, Tone},
        icons, note,
    },
    store::databases as db,
    theme as tokens,
};

use super::{rail, tasks::tasks_section};

pub(crate) fn detail_pane() -> impl IntoElement {
    view()
        .test_id("database-detail")
        .flex_1()
        .min_w(px(0.0))
        .child(
            when(
                move || db::selected_instance().is_some(),
                || {
                    view()
                        .flex_col()
                        .flex_1()
                        .child(dynamic(|| Some(header())))
                        .child(notes())
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
                                            .child(tasks_section())
                                            .child(connections_card())
                                            .child(endpoint_card())
                                            .child(databases_card())
                                            .child(backups_card())
                                            .child(remove_card()),
                                    ),
                            ),
                        )
                },
            )
            .otherwise(empty_state),
        )
}

fn empty_state() -> impl IntoElement {
    view()
        .test_id("db-empty-state")
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .child(
            view()
                .h(px(52.0))
                .flex_none()
                .flex_row()
                .items_center()
                .px(tokens::space(5.0))
                .border_b_1()
                .border_color(tokens::hairline)
                .window_drag(true),
        )
        .child(
            view()
                .flex_1()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(tokens::space(3.0))
                .px(tokens::space(6.0))
                .child(
                    view()
                        .size(px(48.0))
                        .items_center()
                        .justify_center()
                        .rounded(tokens::panel_radius)
                        .bg(tokens::inset)
                        .border_1()
                        .border_color(tokens::hairline)
                        .child(
                            icons::engine("postgresql")
                                .size(px(22.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(
                    text("No database selected")
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(ui::action_button(
                    "New instance",
                    Tone::Primary,
                    "db-empty-new-instance",
                    false,
                    db::open_new,
                )),
        )
}

fn header() -> AnyElement {
    let Some(instance) = db::selected_instance() else {
        return view()
            .h(px(52.0))
            .flex_none()
            .px(tokens::space(5.0))
            .border_b_1()
            .border_color(tokens::hairline)
            .window_drag(true)
            .into_element();
    };
    let id = instance.id.clone();
    let status = db::status(&id);
    let running = matches!(status.as_str(), "Running" | "Healthy");
    let start_id = id.clone();
    let restart_id = id.clone();
    let stop_id = id.clone();
    let log_job = format!("db-{id}");

    ui::page_header_with_leading(
        Some(
            icons::engine(&instance.kind)
                .size(px(16.0))
                .flex_none()
                .text_color(move || theme().muted_foreground),
        ),
        id,
        None::<&str>,
        Some(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(ui::status_tag(&status))
                .child((!running).then(|| {
                    ui::action_button(
                        "Start",
                        Tone::Primary,
                        "db-start",
                        db::operations_busy,
                        move || db::start_instance(&start_id),
                    )
                }))
                .child(running.then(|| {
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.5))
                        .child(ui::action_button(
                            "Restart",
                            Tone::Default,
                            "db-restart",
                            db::operations_busy,
                            move || db::restart_instance(&restart_id),
                        ))
                        .child(ui::action_button(
                            "Stop",
                            Tone::Default,
                            "db-stop",
                            db::operations_busy,
                            move || db::stop_instance(&stop_id),
                        ))
                }))
                .child(ui::action_button(
                    "Logs",
                    Tone::Default,
                    "db-open-logs",
                    false,
                    move || crate::dev_logs::open(&log_job),
                ))
                .child(ui::toolbar_icon_button(
                    icons::refresh(),
                    "Refresh databases",
                    "db-refresh",
                    db::refresh,
                )),
        ),
    )
    .into_element()
}

fn notes() -> impl IntoElement {
    view()
        .flex_col()
        .flex_none()
        .child(dynamic(move || {
            let instance = db::selected_instance()?;
            let status = db::status(&instance.id);
            if status != "Failed" {
                return None;
            }
            let telemetry = db::telemetry(&instance.id)?;
            if telemetry.last_stderr.is_empty() {
                return None;
            }
            let lines = telemetry.last_stderr;
            let log_job = format!("db-{}", instance.id);
            Some(
                view()
                    .test_id("db-failure-stderr")
                    .flex_col()
                    .gap(tokens::space(1.5))
                    .px(tokens::space(5.0))
                    .py(tokens::space(2.5))
                    .bg(move || theme().destructive.opacity(0.08))
                    .border_b_1()
                    .border_color(move || theme().destructive.opacity(0.25))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(tokens::space(2.0))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(tokens::space(1.5))
                                    .child(
                                        icons::warn()
                                            .size(px(13.0))
                                            .text_color(move || theme().destructive),
                                    )
                                    .child(
                                        text("Last stderr output")
                                            .text_size(px(tokens::TEXT_CAPTION))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(move || theme().destructive),
                                    ),
                            )
                            .child(ui::action_button(
                                "Open Logs",
                                Tone::Default,
                                "db-failure-open-logs",
                                false,
                                move || crate::dev_logs::open(&log_job),
                            )),
                    )
                    .child(
                        view()
                            .flex_col()
                            .gap(px(2.0))
                            .child(lines.into_iter().map(|line| {
                                text(line)
                                    .font_family("monospace")
                                    .text_size(px(tokens::TEXT_CAPTION))
                                    .text_color(move || theme().foreground)
                                    .selectable(true)
                            })),
                    ),
            )
        }))
        .child(dynamic(move || {
            db::load_error().map(|message| note(icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            db::action_error().map(|message| note(icons::warn(), message, true))
        }))
}

fn empty() -> impl IntoElement {
    view().flex_1().items_center().justify_center().child(
        text("Choose an instance or create one.")
            .text_size(px(tokens::TEXT_SMALL))
            .text_color(move || theme().muted_foreground),
    )
}

// --- Connections -------------------------------------------------------------

fn connections_card() -> impl IntoElement {
    let chips_key = computed(|| {
        let instance = db::selected_instance()?;
        let profiles = db::profiles(&instance.id);
        Some((instance.id, profiles))
    });
    let editor_key = computed(|| {
        let instance = db::selected_instance()?;
        if db::selected_profile().is_empty() {
            return None;
        }
        Some((instance.id, instance.kind))
    });
    let actions_key = computed(|| {
        let instance = db::selected_instance()?;
        let selected = db::selected_profile();
        Some((
            instance.id,
            instance.kind,
            selected != "new",
            selected.is_empty(),
        ))
    });
    let details = computed(db::connection_details);

    ui::card(
        "Connections",
        view()
            .flex_col()
            .gap(tokens::space(3.0))
            .child(dynamic(move || {
                Some(
                    chips_key
                        .get()
                        .map(|(id, profiles)| profile_chips(id, profiles).into_element())
                        .unwrap_or_else(|| empty().into_element()),
                )
            }))
            .child(when(db::pending_switch, pending_switch_notice))
            .child(dynamic(move || {
                let (id, kind) = editor_key.get()?;
                Some(editor_panel(&kind, id.clone(), id).into_element())
            }))
            .child(dynamic(move || {
                let (id, kind, can_copy, admin) = actions_key.get()?;
                Some(connection_actions(id, kind, can_copy, admin).into_element())
            }))
            .child(dynamic(move || {
                details.get().map(|block| {
                    view()
                        .flex_col()
                        .gap(tokens::space(1.0))
                        .p(tokens::space(2.0))
                        .rounded(move || theme().radius)
                        .bg(tokens::inset)
                        .child(
                            text(block)
                                .font_family("monospace")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground)
                                .selectable(true),
                        )
                })
            })),
    )
}

fn profile_chips(id: String, profiles: Vec<devcore::model::ConnectionProfile>) -> impl IntoElement {
    let id_add = id;

    view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(tokens::space(1.5))
        .child(ui::chip(
            "Administrator",
            || db::selected_profile().is_empty(),
            "profile-admin",
            false,
            move || db::select_profile(""),
        ))
        .child(profiles.into_iter().map(|profile| {
            let press = profile.id.clone();
            let selected_id = profile.id.clone();
            ui::chip(
                profile.name.clone(),
                move || db::selected_profile() == selected_id,
                format!("profile-{}", profile.id),
                false,
                move || db::select_profile(&press),
            )
            .into_element()
        }))
        .child(ui::chip(
            "Add profile",
            || db::selected_profile() == "new",
            "profile-new",
            false,
            move || db::add_profile(&id_add),
        ))
}

fn connection_actions(id: String, kind: String, can_copy: bool, admin: bool) -> impl IntoElement {
    let jdbc = kind == "postgresql" || kind == "mysql";
    let cli = kind != "mongodb";

    let id_preview = id.clone();
    let id_uri = id.clone();
    let id_docker = id.clone();
    let id_env = id.clone();
    let id_jdbc = id.clone();
    let id_cli_copy = id.clone();
    let id_password = id.clone();
    let id_client = id.clone();
    let id_cli = id.clone();
    let id_details = id;

    let copy_disabled = move || db::operations_busy() || !can_copy || db::profile_dirty();

    view()
        .flex_col()
        .gap(tokens::space(2.5))
        .child(dynamic(move || {
            let uri = db::preview_uri(&id_preview)?;
            Some(
                view()
                    .test_id("connection-uri-preview")
                    .min_h(px(32.0))
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(2.0))
                    .px(tokens::space(2.5))
                    .py(tokens::space(1.5))
                    .rounded(move || theme().radius)
                    .bg(tokens::inset)
                    .border_1()
                    .border_color(tokens::hairline)
                    .child(
                        icons::link()
                            .size(px(13.0))
                            .flex_none()
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(
                        text(uri)
                            .flex_1()
                            .min_w(px(0.0))
                            .font_family("monospace")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().foreground)
                            .selectable(true)
                            .truncate(),
                    )
                    .into_element(),
            )
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(2.0))
                .child(ui::action_button(
                    "Copy connection URI",
                    Tone::Primary,
                    "connection-uri-copy",
                    copy_disabled,
                    move || db::copy_uri(&id_uri),
                ))
                .child(cli.then(|| {
                    ui::action_button(
                        "Open CLI in Terminal",
                        Tone::Default,
                        "connection-cli",
                        copy_disabled,
                        move || db::open_cli(&id_cli),
                    )
                }))
                .child(ui::action_button(
                    "Open in client",
                    Tone::Default,
                    "connection-client",
                    copy_disabled,
                    move || db::open_client(&id_client),
                )),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.0))
                .pt(tokens::space(1.0))
                .border_t_1()
                .border_color(tokens::hairline)
                .child(quiet_connection_button(
                    "Copy Docker URI",
                    "connection-docker-uri-copy",
                    copy_disabled,
                    move || db::copy_docker_uri(&id_docker),
                ))
                .child(quiet_connection_button(
                    "Copy .env",
                    "connection-env-copy",
                    copy_disabled,
                    move || db::copy_env(&id_env),
                ))
                .child(jdbc.then(|| {
                    quiet_connection_button(
                        "Copy JDBC URL",
                        "connection-jdbc-copy",
                        copy_disabled,
                        move || db::copy_jdbc(&id_jdbc),
                    )
                }))
                .child(quiet_connection_button(
                    "Copy CLI command",
                    "connection-cli-copy",
                    copy_disabled,
                    move || db::copy_cli_command(&id_cli_copy),
                ))
                .child(admin.then(|| {
                    quiet_connection_button(
                        "Copy database password",
                        "connection-password-copy",
                        db::operations_busy,
                        move || db::copy_password(&id_password),
                    )
                }))
                .child(admin.then(|| {
                    quiet_connection_button(
                        "Details",
                        "connection-details",
                        db::operations_busy,
                        move || db::show_connection_details(&id_details),
                    )
                })),
        )
}

/// Low-emphasis secondary action button inside the Connections card so
/// secondary copy/inspect formats recede cleanly beneath the primary CTA row.
fn quiet_connection_button(
    label: &'static str,
    test_id: &'static str,
    disabled: impl IntoReactive<bool> + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let disabled = disabled.into_reactive();
    let disabled_opacity = disabled.clone();
    nsh_base::button()
        .test_id(test_id)
        .h(px(24.0))
        .flex_none()
        .items_center()
        .justify_center()
        .px(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(AdaptiveColor::TRANSPARENT)
        .hover(move |style| style.bg(tokens::row_hover()))
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .disabled(move || disabled.get())
        .opacity(move || {
            if disabled_opacity.get() {
                0.45_f32
            } else {
                1.0_f32
            }
        })
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
}

fn pending_switch_notice() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(
            text("Unsaved connection changes. Discard or cancel the switch.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(tokens::warning),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Cancel connection switch",
                    Tone::Default,
                    "profile-cancel-switch",
                    false,
                    db::cancel_pending_switch,
                ))
                .child(ui::action_button(
                    "Discard connection changes",
                    Tone::Danger,
                    "profile-discard-switch",
                    false,
                    db::discard_pending_switch,
                )),
        )
}

fn editor_panel(kind: &str, save_id: String, delete_id: String) -> impl IntoElement {
    let redis = kind == "redis";
    let mongo = kind == "mongodb";
    view()
        .flex_col()
        .gap(tokens::space(2.5))
        .p(tokens::space(3.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .child(
            view()
                .flex_row()
                .gap(tokens::space(3.0))
                .flex_wrap()
                .child(view().flex_1().min_w(px(160.0)).child(ui::field(
                    "Connection profile name",
                    ui::text_field(
                        "profile-name",
                        "Application",
                        db::profile_name,
                        db::set_profile_name,
                    ),
                )))
                .child(view().flex_1().min_w(px(160.0)).child(if redis {
                    ui::field(
                        "Profile Redis database (0–15)",
                        ui::text_field(
                            "profile-redis-db",
                            "0",
                            db::profile_redis_db,
                            db::set_profile_redis_db,
                        ),
                    )
                    .into_element()
                } else {
                    ui::field(
                        "Profile database",
                        ui::text_field(
                            "profile-database",
                            "app",
                            db::profile_database,
                            db::set_profile_database,
                        ),
                    )
                    .into_element()
                })),
        )
        .child(when(!redis, move || {
            view()
                .flex_row()
                .gap(tokens::space(3.0))
                .flex_wrap()
                .child(view().flex_1().min_w(px(160.0)).child(ui::field(
                    "Profile username",
                    ui::text_field(
                        "profile-username",
                        "app_user",
                        db::profile_user,
                        db::set_profile_user,
                    ),
                )))
                .child(view().flex_1().min_w(px(160.0)).child(when(mongo, || {
                    ui::field(
                        "Mongo authSource",
                        ui::text_field(
                            "profile-auth-source",
                            "app",
                            db::profile_mongo_auth_source,
                            db::set_profile_mongo_auth_source,
                        ),
                    )
                })))
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Save connection profile",
                    Tone::Primary,
                    "profile-save",
                    db::operations_busy,
                    move || db::save_profile(&save_id),
                ))
                .child(when(
                    move || db::selected_profile() != "new",
                    move || {
                        let delete_id = delete_id.clone();
                        ui::action_button(
                            "Delete connection profile",
                            Tone::Danger,
                            "profile-delete",
                            db::operations_busy,
                            move || db::delete_profile(&delete_id),
                        )
                    },
                )),
        )
}

// --- Endpoint and storage ----------------------------------------------------

fn endpoint_card() -> impl IntoElement {
    let key = computed(|| {
        let instance = db::selected_instance()?;
        let version = db::package_version(&instance.package);
        Some((
            instance.id,
            instance.kind,
            version,
            instance.port,
            instance.data_directory,
        ))
    });
    ui::card(
        "Endpoint & storage",
        dynamic(move || {
            Some(
                key.get()
                    .map(|(id, kind, version, port, data_directory)| {
                        endpoint_body(id, kind, version, port, data_directory).into_element()
                    })
                    .unwrap_or_else(|| empty().into_element()),
            )
        }),
    )
}

fn endpoint_body(
    id: String,
    kind: String,
    version: String,
    port: i64,
    data_directory: String,
) -> impl IntoElement {
    let data = if data_directory.is_empty() {
        format!("instances/{id}")
    } else {
        data_directory.clone()
    };
    let endpoint = format!("127.0.0.1:{port}");
    let summary = format!("{} {}", rail::instance_title(&kind), version);
    let external = !data_directory.is_empty();

    let id_upgrade = id.clone();
    let id_save_port = id.clone();
    let id_save_disabled = id.clone();
    let id_preview = id.clone();
    let id_telemetry = id.clone();

    view()
        .flex_col()
        .gap(tokens::space(2.5))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap(tokens::space(2.0))
                .child(
                    text(summary)
                        .text_size(px(tokens::TEXT_SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .tabular_nums()
                        .text_color(move || theme().foreground)
                        .selectable(true),
                )
                .child(dynamic(move || {
                    let target = db::upgrade_target(&id_upgrade)?;
                    let upgrade_version = db::package_version(&target);
                    let upgrade_id = id_upgrade.clone();
                    let running_id = id_upgrade.clone();
                    Some(ui::action_button(
                        format!("Upgrade to {upgrade_version}…"),
                        Tone::Default,
                        "db-upgrade",
                        move || db::operations_busy() || db::is_running(&running_id),
                        move || db::request_upgrade(&upgrade_id),
                    ))
                })),
        )
        .child(ui::kv_mono("Endpoint", endpoint))
        .child(
            view()
                .flex_col()
                .gap(tokens::space(1.5))
                .child(
                    view()
                        .flex_row()
                        .items_end()
                        .flex_wrap()
                        .gap(tokens::space(2.0))
                        .child(view().w(px(140.0)).flex_none().child(ui::field(
                            "Local port",
                            ui::text_field(
                                "db-edit-port",
                                "5432",
                                db::edit_port,
                                db::set_edit_port,
                            ),
                        )))
                        .child(ui::action_button(
                            "Check port",
                            Tone::Default,
                            "db-edit-check-port",
                            false,
                            db::check_edit_port,
                        ))
                        .child(ui::action_button(
                            "Save port",
                            Tone::Default,
                            "db-save-port",
                            move || {
                                db::operations_busy()
                                    || db::is_running(&id_save_disabled)
                                    || db::edit_port().trim().parse::<i64>().ok() == Some(port)
                            },
                            move || db::save_edit_port(&id_save_port),
                        )),
                )
                .child(edit_port_preview(id_preview)),
        )
        .child(ui::hairline())
        .child(ui::kv_mono("Data", data))
        .child(dynamic(move || {
            let telemetry = db::telemetry(&id_telemetry);
            let disk = telemetry
                .as_ref()
                .and_then(|t| t.data_size_bytes)
                .map(|bytes| db::format_size(bytes as i64))
                .unwrap_or_else(|| "—".to_string());
            let backup = telemetry
                .as_ref()
                .and_then(|t| t.last_backup.as_ref())
                .map(|(summary, size)| format!("{summary} · {}", db::format_size(*size as i64)))
                .unwrap_or_else(|| "None".to_string());
            Some(
                view()
                    .test_id("db-telemetry")
                    .flex_col()
                    .gap(tokens::space(2.0))
                    .child(ui::kv("Disk usage", disk))
                    .child(ui::kv("Latest backup", backup)),
            )
        }))
        .child(when(external, || {
            text("External data is kept when removed; permanent deletion is unavailable.")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(tokens::warning)
        }))
}

fn edit_port_preview(id: String) -> impl IntoElement {
    let id_err = id.clone();
    let id_insp = id.clone();
    let id_load = id;
    view()
        .flex_col()
        .child(dynamic(move || {
            db::edit_port_error(&id_err).map(|message| {
                text(message)
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().destructive)
                    .selectable(true)
            })
        }))
        .child(dynamic(move || {
            let inspection = db::edit_port_inspection(&id_insp)?;
            let tone = match inspection.status.as_str() {
                "Available" => TagTone::Success,
                "In use" => TagTone::Warning,
                "Devbase proxy" => TagTone::Neutral,
                _ => TagTone::Warning,
            };
            let conflict = db::format_port_conflict(&inspection);
            let suggested = db::edit_port_conflict_suggestion(&id_insp);
            let use_id = id_insp.clone();
            let label = format!("Port {}", inspection.port);
            Some(
                view()
                    .test_id("db-edit-port-preview")
                    .flex_col()
                    .gap(tokens::space(1.0))
                    .p(tokens::space(2.0))
                    .rounded(move || theme().radius)
                    .bg(tokens::inset)
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .flex_wrap()
                            .gap(tokens::space(2.0))
                            .child(
                                text(label)
                                    .text_size(px(tokens::TEXT_SMALL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .tabular_nums()
                                    .text_color(move || theme().foreground),
                            )
                            .child(ui::tag(inspection.status.clone(), tone)),
                    )
                    .child(conflict.map(|warning| {
                        view()
                            .test_id("db-port-conflict")
                            .flex_row()
                            .items_center()
                            .flex_wrap()
                            .gap(tokens::space(2.0))
                            .child(
                                text(warning)
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(tokens::TEXT_SMALL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .tabular_nums()
                                    .text_color(tokens::warning)
                                    .selectable(true),
                            )
                            .child(suggested.map(|next_port| {
                                let press_id = use_id.clone();
                                ui::action_button(
                                    format!("Use :{next_port}"),
                                    Tone::Primary,
                                    "db-use-suggested-port",
                                    false,
                                    move || db::apply_suggested_edit_port(&press_id, next_port),
                                )
                            }))
                    })),
            )
        }))
        .child(dynamic(move || {
            db::edit_port_loading(&id_load).then(|| {
                text("Checking…")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
            })
        }))
}

// --- Databases and users -----------------------------------------------------

fn databases_card() -> impl IntoElement {
    let key = computed(|| db::selected_instance().map(|instance| (instance.id, instance.kind)));
    dynamic(move || {
        let Some((id, kind)) = key.get() else {
            return Some(ui::card("Databases & users", empty()).into_element());
        };
        let title = if kind == "redis" {
            "Database"
        } else {
            "Databases & users"
        };
        Some(ui::card(title, databases_body(id, kind)).into_element())
    })
}

fn databases_body(id: String, kind: String) -> impl IntoElement {
    let redis = kind == "redis";
    let catalog_error = computed(db::catalog_error);
    let catalog_databases = computed(|| db::catalog().unwrap_or_default().databases);
    let catalog_users = computed(|| db::catalog().unwrap_or_default().users);

    let id_refresh = id.clone();
    let id_refresh_disabled = id.clone();
    let id_create = id.clone();
    let id_drop = id.clone();
    let id_create_user = id.clone();
    let id_drop_user = id;

    view()
        .flex_col()
        .gap(tokens::space(2.5))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(ui::action_button(
                    "Refresh databases",
                    Tone::Default,
                    "db-catalog-refresh",
                    move || db::operations_busy() || !db::is_running(&id_refresh_disabled),
                    move || db::refresh_catalog(&id_refresh),
                ))
                .child(when(db::catalog_loading, || {
                    text("Reading…")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                })),
        )
        .child(dynamic(move || {
            catalog_error
                .get()
                .map(|message| note(icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            let names = catalog_databases.get();
            (!names.is_empty()).then(|| {
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
                    .child(names.into_iter().map(|database| {
                        let press = database.clone();
                        let selected = database.clone();
                        ui::chip(
                            database.clone(),
                            move || db::database_name() == selected,
                            format!("db-name-{database}"),
                            false,
                            move || db::set_database_name(press.clone()),
                        )
                        .into_element()
                    }))
            })
        }))
        .child(if redis {
            ui::field(
                "Database number (0–15)",
                ui::text_field(
                    "db-database-name",
                    "0",
                    db::database_name,
                    db::set_database_name,
                ),
            )
            .into_element()
        } else {
            ui::field(
                "Database name",
                ui::text_field(
                    "db-database-name",
                    "app",
                    db::database_name,
                    db::set_database_name,
                ),
            )
            .into_element()
        })
        .child(if redis {
            view().into_element()
        } else {
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Create database",
                    Tone::Default,
                    "db-create-database",
                    db::operations_busy,
                    move || db::create_database(&id_create),
                ))
                .child(ui::action_button(
                    "Drop database",
                    Tone::Danger,
                    "db-drop-database",
                    || db::operations_busy() || is_protected_database(&db::database_name()),
                    move || db::request_drop_database(&id_drop),
                ))
                .into_element()
        })
        .child(when(!redis, ui::hairline))
        .child(when(!redis, || {
            text("User")
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(!redis, move || {
            dynamic(move || {
                let users = catalog_users.get();
                (!users.is_empty()).then(|| {
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap(tokens::space(1.5))
                        .child(users.into_iter().map(|entry| {
                            let press = entry.clone();
                            let selected = entry.clone();
                            ui::chip(
                                entry.clone(),
                                move || db::database_user() == selected,
                                format!("db-user-{entry}"),
                                false,
                                move || {
                                    let (user, database) = press
                                        .split_once('@')
                                        .map(|(user, db)| (user.to_string(), Some(db.to_string())))
                                        .unwrap_or((press.clone(), None));
                                    db::set_database_user(user);
                                    if let Some(database) = database
                                        && !database.is_empty()
                                    {
                                        db::set_database_name(database);
                                    }
                                },
                            )
                            .into_element()
                        }))
                })
            })
        }))
        .child(when(!redis, || {
            view()
                .flex_row()
                .gap(tokens::space(3.0))
                .flex_wrap()
                .child(view().flex_1().min_w(px(160.0)).child(ui::field(
                    "Database user",
                    ui::text_field(
                        "db-database-user",
                        "app_user",
                        db::database_user,
                        db::set_database_user,
                    ),
                )))
                .child(view().flex_1().min_w(px(160.0)).child(ui::field(
                    "New user password",
                    ui::text_field(
                        "db-database-password",
                        "",
                        db::database_password,
                        db::set_database_password,
                    ),
                )))
        }))
        .child(when(!redis, move || {
            let id_create_user = id_create_user.clone();
            let id_drop_user = id_drop_user.clone();
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Create database user",
                    Tone::Default,
                    "db-create-user",
                    db::operations_busy,
                    move || db::create_database_user(&id_create_user),
                ))
                .child(ui::action_button(
                    "Drop user",
                    Tone::Danger,
                    "db-drop-user",
                    || db::operations_busy() || is_protected_database_user(&db::database_user()),
                    move || db::request_drop_user(&id_drop_user),
                ))
        }))
}

// --- Backups -----------------------------------------------------------------

fn backups_card() -> impl IntoElement {
    let key = computed(|| db::selected_instance().map(|instance| (instance.id, instance.kind)));
    ui::card(
        "Backups",
        dynamic(move || {
            Some(
                key.get()
                    .map(|(id, kind)| backups_body(id, kind).into_element())
                    .unwrap_or_else(|| empty().into_element()),
            )
        }),
    )
}

fn backups_body(id: String, kind: String) -> impl IntoElement {
    let redis = kind == "redis";
    let id_schedule = id.clone();
    let id_backup = id.clone();
    let id_rows = id.clone();
    let catalog_databases = computed(|| db::catalog().unwrap_or_default().databases);
    let backups_error = computed(db::backups_error);
    let drag_over = signal(false);

    view()
        .flex_col()
        .gap(tokens::space(2.5))
        .child(auto_backup_row(id_schedule))
        .child(ui::hairline())
        .child(dynamic(move || {
            let names = catalog_databases.get();
            (!names.is_empty()).then(|| {
                view()
                    .flex_col()
                    .gap(tokens::space(1.0))
                    .child(
                        text("Target database")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .flex_wrap()
                            .gap(tokens::space(1.5))
                            .child(names.into_iter().map(|name| {
                                let press = name.clone();
                                let selected = name.clone();
                                ui::chip(
                                    name.clone(),
                                    move || db::database_name() == selected,
                                    format!("backup-target-{name}"),
                                    false,
                                    move || db::set_database_name(press.clone()),
                                )
                                .into_element()
                            })),
                    )
            })
        }))
        .child(ui::action_button(
            "Back up database",
            Tone::Default,
            "db-backup",
            db::operations_busy,
            move || db::backup(&id_backup),
        ))
        .child(dynamic(move || {
            backups_error
                .get()
                .map(|message| note(icons::warn(), message, true))
        }))
        .child(dynamic(move || Some(backups_list(&id_rows).into_element())))
        .child(ui::hairline())
        .child(
            view()
                .test_id("db-restore-dropzone")
                .flex_col()
                .gap(tokens::space(2.5))
                .p(tokens::space(3.0))
                .rounded(move || theme().radius)
                .bg(move || {
                    if drag_over.get() {
                        theme().primary.opacity(0.08)
                    } else {
                        tokens::inset()
                    }
                })
                .border_1()
                .border_color(move || {
                    if drag_over.get() {
                        theme().ring
                    } else {
                        tokens::hairline()
                    }
                })
                .on_file_drag_enter(move |_: &FileDragEvent| {
                    drag_over.set(true);
                })
                .on_file_drag_leave(move || {
                    drag_over.set(false);
                })
                .on_file_drop(move |event: &FileDropEvent| {
                    drag_over.set(false);
                    if let Some(first) = event.uris.first() {
                        let path = decode_file_uri(first);
                        if !path.is_empty() {
                            db::set_backup_path(path);
                        }
                    }
                })
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(tokens::space(2.0))
                        .child(
                            text("Restore")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(
                            text(move || {
                                if drag_over.get() {
                                    "Drop backup file"
                                } else {
                                    "Drop file here"
                                }
                            })
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || {
                                if drag_over.get() {
                                    theme().foreground
                                } else {
                                    theme().muted_foreground
                                }
                            }),
                        ),
                )
                .child(ui::field(
                    "Backup file",
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.5))
                        .child(view().flex_1().min_w(px(0.0)).child(ui::text_field(
                            "db-backup-path",
                            "Select a backup file",
                            db::backup_path,
                            db::set_backup_path,
                        )))
                        .child(ui::action_button(
                            "Choose backup file…",
                            Tone::Default,
                            "db-choose-backup",
                            false,
                            db::choose_backup_file,
                        )),
                ))
                .child(ui::field(
                    if redis {
                        "Restore Redis database (0–15)"
                    } else {
                        "Restore target database"
                    },
                    ui::text_field(
                        "db-restore-target",
                        if redis { "1" } else { "app_restored" },
                        db::restore_database,
                        db::set_restore_database,
                    ),
                ))
                .child(ui::action_button(
                    "Restore into new database",
                    Tone::Default,
                    "db-restore",
                    db::operations_busy,
                    move || db::request_restore(&id),
                )),
        )
}

fn auto_backup_row(id: String) -> impl IntoElement {
    let id_switch_read = id.clone();
    let id_switch_write = id.clone();
    let id_daily_read = id.clone();
    let id_daily_write = id.clone();
    let id_weekly_read = id.clone();
    let id_weekly_write = id.clone();
    let id_keep_3_read = id.clone();
    let id_keep_3_write = id.clone();
    let id_keep_5_read = id.clone();
    let id_keep_5_write = id.clone();
    let id_keep_10_read = id.clone();
    let id_keep_10_write = id.clone();
    let id_last = id;

    view()
        .test_id("db-auto-backup-row")
        .flex_col()
        .gap(tokens::space(1.5))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(2.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.5))
                        .child(ui::switch(
                            "db-auto-backup-switch",
                            move || db::backup_schedule(&id_switch_read).enabled,
                            db::operations_busy,
                            "Automatic backups",
                            move |next| db::set_backup_schedule_enabled(&id_switch_write, next),
                        ))
                        .child(
                            text("Automatic backups")
                                .text_size(px(tokens::TEXT_SMALL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.0))
                        .child(ui::chip(
                            "Daily",
                            move || {
                                db::backup_schedule(&id_daily_read).cadence == BackupCadence::Daily
                            },
                            "db-backup-cadence-daily",
                            db::operations_busy,
                            move || {
                                db::set_backup_schedule_cadence(
                                    &id_daily_write,
                                    BackupCadence::Daily,
                                )
                            },
                        ))
                        .child(ui::chip(
                            "Weekly",
                            move || {
                                db::backup_schedule(&id_weekly_read).cadence
                                    == BackupCadence::Weekly
                            },
                            "db-backup-cadence-weekly",
                            db::operations_busy,
                            move || {
                                db::set_backup_schedule_cadence(
                                    &id_weekly_write,
                                    BackupCadence::Weekly,
                                )
                            },
                        )),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.0))
                        .child(ui::chip(
                            "Keep 3",
                            move || db::backup_schedule(&id_keep_3_read).keep_last == 3,
                            "db-backup-keep-3",
                            db::operations_busy,
                            move || db::set_backup_schedule_keep_last(&id_keep_3_write, 3),
                        ))
                        .child(ui::chip(
                            "Keep 5",
                            move || db::backup_schedule(&id_keep_5_read).keep_last == 5,
                            "db-backup-keep-5",
                            db::operations_busy,
                            move || db::set_backup_schedule_keep_last(&id_keep_5_write, 5),
                        ))
                        .child(ui::chip(
                            "Keep 10",
                            move || db::backup_schedule(&id_keep_10_read).keep_last == 10,
                            "db-backup-keep-10",
                            db::operations_busy,
                            move || db::set_backup_schedule_keep_last(&id_keep_10_write, 10),
                        )),
                ),
        )
        .child(dynamic(move || {
            db::backup_schedule(&id_last).last_run_at.map(|ts| {
                text(format!("Last automatic backup: {ts}"))
                    .text_size(px(tokens::TEXT_CAPTION))
                    .tabular_nums()
                    .text_color(move || theme().muted_foreground)
            })
        }))
}

/// Decodes a `file://` URI (with percent-encoded bytes) into a local path;
/// plain filesystem paths pass through unchanged.
fn decode_file_uri(uri: &str) -> String {
    let raw = uri
        .strip_prefix("file://localhost")
        .or_else(|| uri.strip_prefix("file://"))
        .unwrap_or(uri);
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| raw.to_string())
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn backups_list(id: &str) -> impl IntoElement {
    let rows = db::backups();
    let mut list: Vec<AnyElement> = Vec::new();
    if db::backups_loading() && rows.is_empty() {
        list.push(
            text("Reading backups…")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .into_element(),
        );
    } else if rows.is_empty() {
        list.push(
            text("No backups")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .into_element(),
        );
    } else {
        for row in rows {
            list.push(backup_row(id, row).into_element());
        }
    }
    view().flex_col().gap(tokens::space(1.0)).child(list)
}

fn backup_row(id: &str, row: BackupFile) -> impl IntoElement {
    let name = row.name.clone();
    let label = format!(
        "{} · {} · {}",
        row.database,
        db::format_modified(row.modified),
        db::format_size(row.size)
    );
    let reveal = row.path.clone();
    let use_row = row.clone();
    let delete_row = row.clone();
    let delete_id = id.to_string();
    view()
        .test_id(&format!("db-backup-row-{name}"))
        .flex_row()
        .items_center()
        .gap(tokens::space(1.5))
        .min_w(px(0.0))
        .py(tokens::space(0.5))
        .child(
            text(label)
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(tokens::TEXT_SMALL))
                .tabular_nums()
                .text_color(move || theme().muted_foreground)
                .truncate(),
        )
        .child(ui::action_button(
            "Use for restore",
            Tone::Default,
            format!("db-use-backup-{name}"),
            false,
            move || db::use_backup_for_restore(use_row.clone()),
        ))
        .child(ui::row_icon_button(
            icons::folder(),
            crate::platform::show_in_files_label(&name),
            format!("db-reveal-{name}"),
            false,
            move || db::reveal(&reveal),
        ))
        .child(ui::action_button(
            "Delete",
            Tone::Danger,
            format!("db-delete-backup-{name}"),
            db::operations_busy,
            move || db::request_delete_backup(&delete_id, delete_row.clone()),
        ))
}

// --- Remove ------------------------------------------------------------------

fn remove_card() -> impl IntoElement {
    let key = computed(|| {
        db::selected_instance().map(|instance| (instance.id, !instance.data_directory.is_empty()))
    });
    ui::danger_card(
        "Remove",
        dynamic(move || {
            let (id, external) = key.get().unwrap_or_default();
            Some(remove_body(id, external).into_element())
        }),
    )
}

fn remove_body(id: String, external: bool) -> impl IntoElement {
    let remove_id = id.clone();
    let preview_id = id;
    view()
        .flex_col()
        .gap(tokens::space(2.5))
        .child(
            text(if external {
                "External data is kept on disk."
            } else {
                "Removing keeps data files on disk."
            })
            .text_size(px(tokens::TEXT_CAPTION))
            .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Remove instance (keep data files)",
                    Tone::Danger,
                    "db-remove",
                    db::operations_busy,
                    move || db::remove_instance(&remove_id),
                ))
                .child(ui::action_button(
                    "Review permanent data deletion",
                    Tone::Danger,
                    "db-review-deletion",
                    db::operations_busy,
                    move || db::open_deletion_preview(&preview_id),
                )),
        )
}
