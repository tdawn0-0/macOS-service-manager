//! The database rail: every registered instance with its engine, port and
//! state, then the new-instance form and the unfinished-work lists, in the
//! Go development tab's order.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::icons as app_icons,
    store::{self, Section, databases as db},
    theme as tokens,
};

use super::icons;

pub(crate) fn rail() -> impl IntoElement {
    view()
        .w(px(208.0))
        .flex_none()
        .flex_col()
        .bg(move || theme().background)
        .border_r_1()
        .border_color(tokens::hairline)
        .px(tokens::space(2.0))
        .py(tokens::space(3.0))
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .px(tokens::space(2.0))
                .pb(tokens::space(3.0))
                .child(
                    text("Databases")
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(
                    text(Section::Databases.summary())
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .number_of_lines(2),
                ),
        )
        .child(
            text("Instances")
                .px(tokens::space(2.0))
                .pb(tokens::space(1.0))
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(dynamic(|| Some(rows())))
        .child(
            view()
                .flex_col()
                .gap(tokens::space(1.0))
                .pt(tokens::space(2.0))
                .child(nav_row(
                    "New instance",
                    "db-new",
                    None,
                    db::mode() == db::Mode::New,
                    db::open_new,
                )),
        )
        .child(view().flex_1())
        .child(
            view()
                .flex_col()
                .gap(tokens::space(1.0))
                .pt(tokens::space(3.0))
                .child(section_label("Unfinished"))
                .child(nav_row(
                    "Database tasks",
                    "db-work",
                    Some(format!("{}", db::database_work().len())),
                    db::mode() == db::Mode::Work,
                    db::open_work,
                ))
                .child(nav_row(
                    "Initializations",
                    "db-initializations",
                    Some(format!("{}", db::pending_initializations().len())),
                    db::mode() == db::Mode::Initializations,
                    db::open_initializations,
                ))
                .child(nav_row(
                    "Pending deletions",
                    "db-deletions",
                    Some(format!("{}", db::pending_deletions().len())),
                    db::mode() == db::Mode::Deletions,
                    db::open_deletions,
                )),
        )
        .child(
            view().px(tokens::space(2.0)).pt(tokens::space(3.0)).child(
                text("Removing an instance keeps its data files")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
                    .number_of_lines(2),
            ),
        )
}

fn section_label(title: &'static str) -> impl IntoElement {
    text(title)
        .px(tokens::space(2.0))
        .pb(tokens::space(1.0))
        .text_size(px(tokens::TEXT_CAPTION))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(move || theme().muted_foreground)
}

/// The registered instances, read from the store each time they change.
fn rows() -> impl IntoElement {
    let items = db::instances();
    let empty = items.is_empty();
    view()
        .flex_col()
        .gap(tokens::space(0.5))
        .child(when(empty, || {
            text("None yet")
                .px(tokens::space(2.0))
                .py(tokens::space(1.0))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
        .child(items.into_iter().map(instance_row))
}

fn instance_row(instance: devcore::model::Instance) -> impl IntoElement {
    let id = instance.id.clone();
    let selected = db::mode() == db::Mode::Instance && db::selected_id().as_deref() == Some(&id);
    let press_id = id.clone();
    let title = instance_title(&instance.kind);
    let meta = format!("{}:{}", title, instance.port);
    let working = db::status(&id);
    let running = matches!(working.as_str(), "Running" | "Healthy");
    let active = matches!(working.as_str(), "Starting" | "Restarting" | "Stopping");
    nsh_base::button()
        .test_id(&format!("db-instance-{id}"))
        .w_full()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.0))
        .py(tokens::space(1.5))
        .rounded(move || theme().radius)
        .bg(move || {
            if selected {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_press(move || db::open_instance(&press_id))
        .child(
            icons::engine(&instance.kind)
                .size(px(14.0))
                .flex_none()
                .text_color(move || {
                    if selected {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .child(
            view()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text(id.clone())
                        .text_sm()
                        .text_color(move || theme().sidebar_foreground)
                        .truncate(),
                )
                .child(
                    text(meta)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .truncate(),
                ),
        )
        .child(
            view()
                .size(px(8.0))
                .flex_none()
                .rounded(px(4.0))
                .bg(move || {
                    if running {
                        tokens::success()
                    } else if active {
                        tokens::warning()
                    } else {
                        tokens::hairline()
                    }
                }),
        )
}

pub(crate) fn instance_title(kind: &str) -> &'static str {
    match kind {
        "mysql" => "MySQL",
        "postgresql" => "PostgreSQL",
        "redis" => "Redis",
        "mongodb" => "MongoDB",
        _ => "Database",
    }
}

fn nav_row(
    label: &'static str,
    test_id: &'static str,
    trailing: Option<String>,
    selected: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let has_trailing = trailing.is_some();
    nsh_base::button()
        .test_id(test_id)
        .w_full()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.0))
        .py(tokens::space(1.5))
        .rounded(move || theme().radius)
        .bg(move || {
            if selected {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_press(on_press)
        .child(
            app_icons::external()
                .size(px(13.0))
                .flex_none()
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(label)
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .text_color(move || theme().sidebar_foreground)
                .truncate(),
        )
        .child(when(has_trailing, move || {
            text(trailing.clone().unwrap_or_default())
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
}

/// Moves the workspace to the software screen of an engine, as the Go
/// "Open Software" button does.
pub(crate) fn open_software(kind: &str) {
    if let Some(software) = crate::store::software::SoftwareKind::from_key(kind) {
        crate::store::software::select_software(software);
    }
    store::select_section(Section::Development);
}
