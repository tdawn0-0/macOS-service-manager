//! The new-instance form: name, engine, the version from the software
//! catalog, a free local port and its preflight, and where the data lives
//! (`view_databases.go` `devNewInstance`).

use devcore::model::Package;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{icons as app_icons, note},
    store::{databases as db, software::SoftwareKind},
    theme as tokens,
};

use super::{
    icons, rail,
    tasks::tasks_section,
    ui::{self, TagTone, Tone},
};

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
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(5.0))
                    .pb(tokens::space(5.0))
                    .child(tasks_section())
                    .child(instance_card())
                    .child(storage_card())
                    .child(create_row()),
            ),
        )
}

fn header() -> impl IntoElement {
    let kind = db::draft_kind();
    let title = rail::instance_title(kind);
    view()
        .h(px(60.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
        .pt(tokens::space(2.0))
        .child(
            icons::engine(kind)
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
                    text("New instance")
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(
                    text(title)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

fn notes() -> impl IntoElement {
    view()
        .flex_col()
        .flex_none()
        .child(dynamic(move || {
            db::action_error().map(|message| note(app_icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            db::notice().map(|message| note(icons::check(), message, false))
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
            Some(version_row(kind, db::packages(kind)))
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
        .child(dynamic(persistence_row))
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

fn persistence_row() -> Option<impl IntoElement> {
    if db::draft_kind() != "redis" {
        return None;
    }
    Some(
        view()
            .flex_row()
            .items_center()
            .gap(tokens::space(1.5))
            .child(
                nsh_base::button()
                    .test_id("db-persistent")
                    .size(px(16.0))
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(tokens::hairline)
                    .bg(move || {
                        if db::draft_persistent() {
                            theme().primary
                        } else {
                            AdaptiveColor::TRANSPARENT
                        }
                    })
                    .on_press(db::toggle_persistent)
                    .child(when(db::draft_persistent(), || {
                        icons::check()
                            .size(px(10.0))
                            .text_color(move || theme().primary_foreground)
                    })),
            )
            .child(
                text("Persist database contents")
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().foreground),
            ),
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
                let detail = inspection.detail.clone();
                let listeners = inspection.listeners.clone();
                let port = inspection.port;
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
                                    .text_color(move || theme().foreground),
                            )
                            .child(ui::tag(inspection.status.clone(), tone)),
                    )
                    .child(listeners.into_iter().map(|listener| {
                        text(format!(
                            "PID {} · {} · {}",
                            listener.pid, listener.name, listener.address
                        ))
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .selectable(true)
                    }))
                    .child(when(!detail.is_empty(), move || {
                        text(detail.clone())
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    }))
                    .child(
                        text(format!("This checks port {port}; it does not reserve it."))
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground),
                    )
            })
        }))
        .child(dynamic(move || {
            db::port_loading().then(|| {
                text("Checking the port…")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
            })
        }))
}

fn storage_card() -> impl IntoElement {
    ui::card("Storage", storage_body())
}

fn storage_body() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(2.5))
        .child(ui::field(
            "Data location",
            ui::text_field(
                "db-data-directory",
                "Blank for app storage, or a new absolute directory",
                db::draft_data_directory,
                db::set_draft_data_directory,
            ),
        ))
        .child(dynamic(|| {
            Some(
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(1.5))
                    .child(ui::action_button(
                        "Choose parent folder…",
                        Tone::Default,
                        "db-choose-parent",
                        false,
                        db::choose_data_parent,
                    ))
                    .child(when(!db::draft_data_directory().is_empty(), || {
                        ui::action_button(
                            "Use app storage",
                            Tone::Default,
                            "db-use-app-storage",
                            false,
                            db::use_app_storage,
                        )
                    })),
            )
        }))
        .child(
            text("A chosen folder becomes a new subfolder for this instance. External data is retained when the instance is removed.")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .number_of_lines(2),
        )
}

fn create_row() -> impl IntoElement {
    dynamic(|| {
        Some(ui::action_button(
            "Create instance",
            Tone::Primary,
            "db-create",
            db::operations_busy(),
            db::create_instance,
        ))
    })
}
