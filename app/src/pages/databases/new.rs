//! The new-instance form: name, engine, the version from the software
//! catalog, a free local port and its preflight, and where the data lives
//! (`view_databases.go` `devNewInstance`).

use devcore::model::Package;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{
        controls::{self as ui, TagTone, Tone},
        icons, note,
    },
    store::{databases as db, software::SoftwareKind},
    theme as tokens,
};

use super::{rail, tasks::tasks_section};

pub(crate) fn new_pane() -> impl IntoElement {
    view()
        .test_id("db-create-form")
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
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
                            .child(instance_card())
                            .child(storage_card())
                            .child(create_row()),
                    ),
            ),
        )
}

fn header() -> impl IntoElement {
    let kind = db::draft_kind();
    ui::page_header_with_leading(
        Some(
            icons::engine(kind)
                .size(px(16.0))
                .flex_none()
                .text_color(move || theme().muted_foreground),
        ),
        "New instance",
        None::<&str>,
        None::<AnyElement>,
    )
}

fn notes() -> impl IntoElement {
    view().flex_col().flex_none().child(dynamic(move || {
        db::action_error().map(|message| note(icons::warn(), message, true))
    }))
}

fn instance_card() -> impl IntoElement {
    ui::card("Instance", instance_body())
}

fn instance_body() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(3.0))
        .child(dynamic(|| Some(install_warning())))
        .child(ui::field(
            "Instance name",
            ui::text_field("db-name", "app-db", db::draft_name, db::set_draft_name),
        ))
        .child(dynamic(|| Some(engine_row())))
        .child(dynamic(|| {
            let kind = db::draft_kind();
            let packages = db::packages(kind);
            (!packages.is_empty()).then(|| version_row(kind, packages))
        }))
        .child(
            view()
                .flex_row()
                .gap(tokens::space(3.0))
                .flex_wrap()
                .child(view().w(px(180.0)).flex_none().child(ui::field(
                    "Local port",
                    ui::text_field("db-port", "55432", db::draft_port, db::set_draft_port),
                )))
                .child(
                    view()
                        .flex_row()
                        .items_end()
                        .gap(tokens::space(1.5))
                        .child(ui::action_button(
                            "Check port",
                            Tone::Default,
                            "db-check-port",
                            false,
                            db::check_port,
                        ))
                        .child(ui::action_button(
                            "Find a free port",
                            Tone::Default,
                            "db-find-port",
                            false,
                            db::find_free_port,
                        )),
                ),
        )
        .child(port_preview())
        .child(when(move || db::draft_kind() == "redis", persistence_row))
}

fn install_warning() -> impl IntoElement {
    let kind = db::draft_kind();
    let title = rail::instance_title(kind);
    let missing = db::packages(kind).is_empty();
    view().flex_col().child(when(missing, move || {
        view()
            .flex_row()
            .items_center()
            .gap(tokens::space(2.0))
            .child(
                text(format!("Install {title} first, then create the instance."))
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(tokens::warning),
            )
            .child(ui::action_button(
                "Open Software",
                Tone::Default,
                "db-open-software",
                false,
                move || rail::open_software(kind),
            ))
    }))
}

fn persistence_row() -> impl IntoElement {
    nsh_base::checkbox()
        .test_id("db-persistent")
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .rounded(move || theme().radius)
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .checked(db::draft_persistent)
        .on_change(|next: &bool| db::set_draft_persistent(*next))
        .indicator_style(|style| {
            style
                .size(px(16.0))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .border_1()
                .border_color(move || {
                    if db::draft_persistent() {
                        theme().primary
                    } else {
                        tokens::hairline()
                    }
                })
                .bg(move || {
                    if db::draft_persistent() {
                        theme().primary
                    } else {
                        tokens::control()
                    }
                })
        })
        .indicator(view().child(when(db::draft_persistent, || {
            icons::check()
                .size(px(10.0))
                .text_color(move || theme().primary_foreground)
        })))
        .child(
            text("Persist database contents")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
        )
}

fn engine_row() -> impl IntoElement {
    let active = db::draft_kind();
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .child(
            text("Database engine")
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(SoftwareKind::DATABASES.into_iter().map(move |kind| {
                    ui::chip(
                        kind.title(),
                        active == kind.key(),
                        format!("db-engine-{}", kind.key()),
                        false,
                        move || db::set_draft_kind(kind.key()),
                    )
                    .into_element()
                })),
        )
}

fn version_row(kind: &'static str, packages: Vec<Package>) -> impl IntoElement {
    let selected = db::draft_package();
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .child(
            text("Database version")
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(packages.into_iter().map(move |package| {
                    let id = package.id();
                    let press = id.clone();
                    let label = format!("{} · {}", rail::instance_title(kind), package.version);
                    ui::chip(
                        label,
                        selected == id,
                        format!("db-version-{id}"),
                        false,
                        move || db::set_draft_package(press.clone()),
                    )
                    .into_element()
                })),
        )
}

fn port_preview() -> impl IntoElement {
    view()
        .flex_col()
        .child(dynamic(move || {
            db::port_error().map(|message| {
                text(message)
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().destructive)
                    .selectable(true)
            })
        }))
        .child(dynamic(move || {
            db::port_inspection().map(|inspection| {
                let tone = match inspection.status.as_str() {
                    "Available" => TagTone::Success,
                    "In use" => TagTone::Warning,
                    "Devbase proxy" => TagTone::Neutral,
                    _ => TagTone::Warning,
                };
                let label = format!("Port {}", inspection.port);
                let conflict = db::format_port_conflict(&inspection);
                let suggested = db::port_conflict_suggestion();
                let (self_pid, self_uid) = crate::store::ports::current_identity();
                let stoppable_listener = inspection
                    .listeners
                    .iter()
                    .find(|l| !l.own && l.pid > 1 && l.pid != self_pid && l.uid == self_uid)
                    .cloned();
                let inspect_port_num = inspection.port;
                let detail = inspection.detail.clone();
                let listeners = inspection.listeners.clone();
                view()
                    .test_id("db-port-preview")
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
                            .child(stoppable_listener.map(|listener| {
                                let name = listener.name.clone();
                                let pid = listener.pid;
                                let label = if name.is_empty() {
                                    format!("Stop PID {pid}")
                                } else {
                                    format!("Stop {name}")
                                };
                                ui::action_button(
                                    label,
                                    Tone::Danger,
                                    "db-stop-conflict-port",
                                    false,
                                    move || {
                                        crate::store::ports::stop_conflict_pid(
                                            inspect_port_num,
                                            pid,
                                            name.clone(),
                                            db::check_port,
                                        );
                                    },
                                )
                            }))
                            .child(suggested.map(|next_port| {
                                ui::action_button(
                                    format!("Use :{next_port}"),
                                    Tone::Primary,
                                    "db-use-suggested-port",
                                    false,
                                    move || db::apply_suggested_port(next_port),
                                )
                            }))
                    }))
                    .child(listeners.into_iter().map(|listener| {
                        text(format!(
                            "PID {} · {} · {}",
                            listener.pid, listener.name, listener.address
                        ))
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .tabular_nums()
                        .text_color(move || theme().muted_foreground)
                        .selectable(true)
                    }))
                    .child(when(!detail.is_empty(), move || {
                        text(detail.clone())
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    }))
            })
        }))
        .child(dynamic(move || {
            db::port_loading().then(|| {
                text("Checking…")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
            })
        }))
}

fn storage_card() -> impl IntoElement {
    ui::card("Storage", storage_body())
}

fn storage_body() -> impl IntoElement {
    view().flex_col().gap(tokens::space(2.0)).child(ui::field(
        "Data location",
        view()
            .flex_row()
            .items_center()
            .gap(tokens::space(1.5))
            .child(view().flex_1().min_w(px(0.0)).child(ui::text_field(
                "db-data-directory",
                "App storage (default) or absolute path",
                db::draft_data_directory,
                db::set_draft_data_directory,
            )))
            .child(ui::action_button(
                "Choose parent folder…",
                Tone::Default,
                "db-choose-parent",
                false,
                db::choose_data_parent,
            ))
            .child(when(
                move || !db::draft_data_directory().is_empty(),
                || {
                    ui::action_button(
                        "Use app storage",
                        Tone::Default,
                        "db-use-app-storage",
                        false,
                        db::use_app_storage,
                    )
                },
            )),
    ))
}

fn create_row() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .justify_end()
        .child(ui::action_button(
            "Create instance",
            Tone::Primary,
            "db-create",
            db::operations_busy,
            db::create_instance,
        ))
}
