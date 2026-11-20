//! One instance's detail: its state and lifecycle actions, the connection
//! panel, the endpoint and storage facts, its databases and users, its
//! backups and restores, and the two ways to remove it
//! (`view_databases.go`, `view_database_connections.go`).

use devcore::{
    databases::BackupFile,
    databases::inventory::{is_protected_database, is_protected_database_user},
};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{icons as app_icons, note},
    store::databases as db,
    theme as tokens,
};

use super::{
    icons, rail,
    tasks::tasks_section,
    ui::{self, Tone},
};

pub(crate) fn detail_pane() -> impl IntoElement {
    view()
        .test_id("database-detail")
        .flex_1()
        .min_w(px(0.0))
        .child(dynamic(|| {
            db::selected_instance().map(|_| {
                view()
                    .flex_col()
                    .flex_1()
                    .child(header())
                    .child(notes())
                    .child(
                        scroll().flex_1().min_h(px(0.0)).child(
                            view()
                                .flex_col()
                                .gap(tokens::space(4.0))
                                .px(tokens::space(5.0))
                                .pb(tokens::space(5.0))
                                .child(tasks_section())
                                .child(connections_card())
                                .child(endpoint_card())
                                .child(databases_card())
                                .child(backups_card())
                                .child(remove_card()),
                        ),
                    )
            })
        }))
}

fn header() -> AnyElement {
    let Some(instance) = db::selected_instance() else {
        return view().h(px(60.0)).flex_none().into_element();
    };
    let id = instance.id.clone();
    let status = db::status(&id);
    let running = matches!(status.as_str(), "Running" | "Healthy");
    let busy = db::operations_busy();
    let meta = format!(
        "{} {} · 127.0.0.1:{}",
        rail::instance_title(&instance.kind),
        db::package_version(&instance.package),
        instance.port
    );
    let start_id = id.clone();
    let restart_id = id.clone();
    let stop_id = id.clone();

    view()
        .h(px(68.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
        .pt(tokens::space(2.0))
        .child(
            icons::engine(&instance.kind)
                .size(px(16.0))
                .flex_none()
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text(id.clone())
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground)
                        .truncate(),
                )
                .child(
                    text(meta)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .truncate(),
                ),
        )
        .child(ui::status_tag(&status))
        .child(ui::action_button(
            "Start",
            if running {
                Tone::Default
            } else {
                Tone::Primary
            },
            "db-start",
            busy,
            move || db::start_instance(&start_id),
        ))
        .child(ui::action_button(
            "Restart",
            Tone::Default,
            "db-restart",
            busy,
            move || db::restart_instance(&restart_id),
        ))
        .child(ui::action_button(
            "Stop",
            Tone::Default,
            "db-stop",
            busy,
            move || db::stop_instance(&stop_id),
        ))
        .child(
            nsh_base::button()
                .test_id("db-refresh")
                .size(px(26.0))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(move || theme().radius)
                .hover(move |style| style.bg(tokens::row_hover()))
                .accessibility_label("Refresh databases")
                .on_press(db::refresh)
                .child(
                    icons::rotate()
                        .size(px(14.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .into_element()
}

fn notes() -> impl IntoElement {
    view()
        .flex_col()
        .flex_none()
        .child(dynamic(move || {
            db::load_error().map(|message| note(app_icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            db::action_error().map(|message| note(app_icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            db::notice().map(|message| note(icons::check(), message, false))
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
    ui::card(
        "Connections",
        dynamic(|| {
            Some(
                connections_body()
                    .map(|body| body.into_element())
                    .unwrap_or_else(|| empty().into_element()),
            )
        }),
    )
}

fn connections_body() -> Option<impl IntoElement> {
    let instance = db::selected_instance()?;
    let selected = db::selected_profile();
    let busy = db::operations_busy();
    let dirty = db::profile_dirty();
    let pending = db::pending_switch();
    let kind = instance.kind.clone();
    let id = instance.id.clone();
    let profiles = db::profiles(&id);
    let can_copy = selected != "new";
    let admin = selected.is_empty();
    let jdbc = kind == "postgresql" || kind == "mysql";
    let cli = kind != "mongodb";

    let id_uri = id.clone();
    let id_env = id.clone();
    let id_jdbc = id.clone();
    let id_password = id.clone();
    let id_client = id.clone();
    let id_cli = id.clone();
    let id_details = id.clone();
    let id_save = id.clone();
    let id_delete = id.clone();
    let id_add = id.clone();

    let chips = view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(tokens::space(1.5))
        .child(ui::chip(
            "Administrator",
            selected.is_empty(),
            "profile-admin",
            move || db::select_profile(""),
        ))
        .child(profiles.into_iter().map(|profile| {
            let press = profile.id.clone();
            ui::chip(
                profile.name.clone(),
                selected == profile.id,
                format!("profile-{}", profile.id),
                move || db::select_profile(&press),
            )
            .into_element()
        }))
        .child(ui::chip(
            "Add profile",
            selected == "new",
            "profile-new",
            move || db::add_profile(&id_add),
        ));

    let mut editor: Vec<AnyElement> = Vec::new();
    if !selected.is_empty() {
        editor
            .push(editor_panel(&kind, busy, id_save, id_delete, selected == "new").into_element());
    }

    Some(
        view()
            .flex_col()
            .gap(tokens::space(3.0))
            .child(chips)
            .child(when(pending, || pending_switch_notice()))
            .child(editor)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
                    .child(ui::action_button(
                        "Copy connection URI",
                        Tone::Default,
                        "connection-uri-copy",
                        busy || !can_copy || dirty,
                        move || db::copy_uri(&id_uri),
                    ))
                    .child(ui::action_button(
                        "Copy .env",
                        Tone::Default,
                        "connection-env-copy",
                        busy || !can_copy || dirty,
                        move || db::copy_env(&id_env),
                    ))
                    .child(jdbc.then(|| {
                        ui::action_button(
                            "Copy JDBC URL",
                            Tone::Default,
                            "connection-jdbc-copy",
                            busy || !can_copy || dirty,
                            move || db::copy_jdbc(&id_jdbc),
                        )
                    }))
                    .child(admin.then(|| {
                        ui::action_button(
                            "Copy database password",
                            Tone::Default,
                            "connection-password-copy",
                            busy,
                            move || db::copy_password(&id_password),
                        )
                    }))
                    .child(ui::action_button(
                        "Open in client",
                        Tone::Default,
                        "connection-client",
                        busy || !can_copy || dirty,
                        move || db::open_client(&id_client),
                    ))
                    .child(cli.then(|| {
                        ui::action_button(
                            "Open CLI in Terminal",
                            Tone::Default,
                            "connection-cli",
                            busy || !can_copy || dirty,
                            move || db::open_cli(&id_cli),
                        )
                    }))
                    .child(admin.then(|| {
                        ui::action_button(
                            "Details",
                            Tone::Default,
                            "connection-details",
                            busy,
                            move || db::show_connection_details(&id_details),
                        )
                    })),
            )
            .child(dynamic(move || {
                db::connection_details().map(|block| {
                    view()
                        .flex_col()
                        .gap(tokens::space(1.0))
                        .p(tokens::space(2.0))
                        .rounded(move || theme().radius)
                        .bg(move || tokens::inset())
                        .child(
                            text(block)
                                .font_family("monospace")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground)
                                .selectable(true),
                        )
                })
            }))
            .child(
                text("Profiles never store the password. The Terminal launcher reads the credential file inside the shell.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
                    .number_of_lines(2),
            ),
    )
}

fn pending_switch_notice() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(
            text("Unsaved connection changes. Discard or cancel the switch.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || tokens::warning()),
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

fn editor_panel(
    kind: &str,
    busy: bool,
    save_id: String,
    delete_id: String,
    is_new: bool,
) -> impl IntoElement {
    let redis = kind == "redis";
    let mongo = kind == "mongodb";
    view()
        .flex_col()
        .gap(tokens::space(2.5))
        .p(tokens::space(3.0))
        .rounded(move || theme().radius)
        .bg(move || tokens::inset())
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
                    busy,
                    move || db::save_profile(&save_id),
                ))
                .child(when(!is_new, move || {
                    let delete_id = delete_id.clone();
                    ui::action_button(
                        "Delete connection profile",
                        Tone::Danger,
                        "profile-delete",
                        busy,
                        move || db::delete_profile(&delete_id),
                    )
                })),
        )
}

// --- Endpoint and storage ----------------------------------------------------

fn endpoint_card() -> impl IntoElement {
    ui::card(
        "Endpoint & storage",
        dynamic(|| {
            Some(
                endpoint_body()
                    .map(|body| body.into_element())
                    .unwrap_or_else(|| empty().into_element()),
            )
        }),
    )
}

fn endpoint_body() -> Option<impl IntoElement> {
    let instance = db::selected_instance()?;
    let id = instance.id.clone();
    let status = db::status(&id);
    let running = matches!(status.as_str(), "Running" | "Healthy");
    let target = db::upgrade_target(&id);
    let version = db::package_version(&instance.package);
    let data = if instance.data_directory.is_empty() {
        format!("instances/{}", instance.id)
    } else {
        instance.data_directory.clone()
    };
    let endpoint = format!("127.0.0.1:{}", instance.port);
    let upgrade_version = target.clone().map(|target| db::package_version(&target));
    let upgrade_id = id.clone();

    Some(
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(kv("Version", version))
            .child(upgrade_version.map(|version| {
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(1.5))
                    .child(ui::action_button(
                        format!("Upgrade to {version}…"),
                        Tone::Default,
                        "db-upgrade",
                        db::operations_busy() || running,
                        move || db::request_upgrade(&upgrade_id),
                    ))
                    .child(
                        text("Only the server version changes; data, port and credentials are kept.")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground),
                    )
            }))
            .child(kv("Endpoint", endpoint))
            .child(kv("Data", data))
            .child(when(!instance.data_directory.is_empty(), || {
                text("External data is retained when the instance is removed; permanent erasure is not available for it.")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || tokens::warning())
                    .number_of_lines(2)
            })),
    )
}

fn kv(label: &'static str, value: String) -> impl IntoElement {
    view()
        .flex_row()
        .items_start()
        .gap(tokens::space(2.0))
        .child(
            text(label)
                .w(px(84.0))
                .flex_none()
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(value)
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground)
                .selectable(true),
        )
}

// --- Databases and users -----------------------------------------------------

fn databases_card() -> impl IntoElement {
    let title = db::selected_instance()
        .filter(|instance| instance.kind == "redis")
        .map(|_| "Database")
        .unwrap_or("Databases & users");
    ui::card(
        title,
        dynamic(|| {
            Some(
                databases_body()
                    .map(|body| body.into_element())
                    .unwrap_or_else(|| empty().into_element()),
            )
        }),
    )
}

fn databases_body() -> Option<impl IntoElement> {
    let instance = db::selected_instance()?;
    let id = instance.id.clone();
    let redis = instance.kind == "redis";
    let busy = db::operations_busy();
    let running = db::is_running(&id);
    let catalog = db::catalog().unwrap_or_default();
    let name = db::database_name();
    let user = db::database_user();
    let password = db::database_password();

    let id_refresh = id.clone();
    let id_create = id.clone();
    let id_drop = id.clone();
    let id_create_user = id.clone();
    let id_drop_user = id.clone();

    Some(
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
                        busy || !running,
                        move || db::refresh_catalog(&id_refresh),
                    ))
                    .child(when(!running, || {
                        text("Start the instance to read its databases.")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    }))
                    .child(when(db::catalog_loading(), || {
                        text("Reading…")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    })),
            )
            .child(dynamic(move || {
                db::catalog_error().map(|message| note(app_icons::warn(), message, true))
            }))
            .child(when(!catalog.databases.is_empty(), {
                let names = catalog.databases.clone();
                let selected = name.clone();
                move || {
                    let names = names.clone();
                    let selected = selected.clone();
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap(tokens::space(1.5))
                        .child(names.into_iter().map(|database| {
                            let press = database.clone();
                            let selected = selected.clone();
                            ui::chip(
                                database.clone(),
                                selected == database,
                                format!("db-name-{database}"),
                                move || db::set_database_name(press.clone()),
                            )
                            .into_element()
                        }))
                }
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
                        busy,
                        move || db::create_database(&id_create),
                    ))
                    .child(ui::action_button(
                        "Drop database",
                        Tone::Danger,
                        "db-drop-database",
                        busy || is_protected_database(&name),
                        move || db::request_drop_database(&id_drop),
                    ))
                    .into_element()
            })
            .child(when(!redis, || ui::hairline()))
            .child(when(!redis, || {
                text("User")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(move || theme().muted_foreground)
            }))
            .child(when(!redis && !catalog.users.is_empty(), {
                let users = catalog.users.clone();
                let selected = user.clone();
                move || {
                    let users = users.clone();
                    let selected = selected.clone();
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap(tokens::space(1.5))
                        .child(users.into_iter().map(|entry| {
                            let press = entry.clone();
                            let chosen = selected.clone();
                            ui::chip(
                                entry.clone(),
                                chosen == entry,
                                format!("db-user-{entry}"),
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
                }
            }))
            .child(when(!redis, {
                let user = user.clone();
                let password = password.clone();
                move || {
                    view()
                        .flex_row()
                        .gap(tokens::space(3.0))
                        .flex_wrap()
                        .child(view().flex_1().min_w(px(160.0)).child(ui::field(
                            "Database user",
                            ui::text_field(
                                "db-database-user",
                                "app_user",
                                {
                                    let user = user.clone();
                                    move || user.clone()
                                },
                                db::set_database_user,
                            ),
                        )))
                        .child(view().flex_1().min_w(px(160.0)).child(ui::field(
                            "New user password",
                            ui::text_field(
                                "db-database-password",
                                "",
                                {
                                    let password = password.clone();
                                    move || password.clone()
                                },
                                db::set_database_password,
                            ),
                        )))
                }
            }))
            .child(when(!redis, {
                let user = user.clone();
                move || {
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
                            busy,
                            move || db::create_database_user(&id_create_user),
                        ))
                        .child(ui::action_button(
                            "Drop user",
                            Tone::Danger,
                            "db-drop-user",
                            busy || is_protected_database_user(&user),
                            move || db::request_drop_user(&id_drop_user),
                        ))
                }
            })),
    )
}

// --- Backups -----------------------------------------------------------------

fn backups_card() -> impl IntoElement {
    ui::card(
        "Backups",
        dynamic(|| {
            Some(
                backups_body()
                    .map(|body| body.into_element())
                    .unwrap_or_else(|| empty().into_element()),
            )
        }),
    )
}

fn backups_body() -> Option<impl IntoElement> {
    let instance = db::selected_instance()?;
    let id = instance.id.clone();
    let redis = instance.kind == "redis";
    let busy = db::operations_busy();
    let rows = db::backups();
    let catalog = db::catalog().unwrap_or_default();
    let database = db::database_name();
    let id_backup = id.clone();

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
            list.push(backup_row(&id, row, busy).into_element());
        }
    }

    Some(
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(when(!catalog.databases.is_empty(), {
                let names = catalog.databases.clone();
                let selected = database.clone();
                move || {
                    let names = names.clone();
                    let selected = selected.clone();
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
                                    let selected = selected.clone();
                                    ui::chip(
                                        name.clone(),
                                        selected == name,
                                        format!("backup-target-{name}"),
                                        move || db::set_database_name(press.clone()),
                                    )
                                    .into_element()
                                })),
                        )
                }
            }))
            .child(ui::action_button(
                "Back up database",
                Tone::Default,
                "db-backup",
                busy,
                move || db::backup(&id_backup),
            ))
            .child(dynamic(move || {
                db::backups_error().map(|message| note(app_icons::warn(), message, true))
            }))
            .child(view().flex_col().gap(tokens::space(1.0)).child(list))
            .child(ui::hairline())
            .child(
                text("Restore")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(move || theme().muted_foreground),
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
                busy,
                move || db::request_restore(&id),
            )),
    )
}

fn backup_row(id: &str, row: BackupFile, busy: bool) -> impl IntoElement {
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
            format!("Show {name} in Finder"),
            format!("db-reveal-{name}"),
            false,
            move || db::reveal(&reveal),
        ))
        .child(ui::action_button(
            "Delete",
            Tone::Danger,
            format!("db-delete-backup-{name}"),
            busy,
            move || db::request_delete_backup(&delete_id, delete_row.clone()),
        ))
}

// --- Remove ------------------------------------------------------------------

fn remove_card() -> impl IntoElement {
    let id = db::selected_id().unwrap_or_default();
    let instance = db::selected_instance();
    let external = instance
        .as_ref()
        .is_some_and(|instance| !instance.data_directory.is_empty());
    let busy = db::operations_busy();
    let remove_id = id.clone();
    let preview_id = id.clone();
    ui::danger_card(
        "Remove",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(
                text(if external {
                    "Removing the registration keeps the external data directory. Devbase never deletes external data; permanent erasure is not available for it."
                } else {
                    "Removing the registration keeps the data files under instances/<id>. Permanent erasure is a separate, confirmed operation."
                })
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .number_of_lines(3),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(1.5))
                    .child(ui::action_button(
                        "Remove instance (keep data files)",
                        Tone::Danger,
                        "db-remove",
                        busy,
                        move || db::remove_instance(&remove_id),
                    ))
                    .child(ui::action_button(
                        "Review permanent data deletion",
                        Tone::Danger,
                        "db-review-deletion",
                        busy,
                        move || db::open_deletion_preview(&preview_id),
                    )),
            ),
    )
}
