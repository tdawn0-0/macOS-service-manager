//! The operation list and the two unfinished-work screens
//! (`view_database_tasks.go`, `view_database_work.go`): what is running or
//! just finished, the database-task reservations left behind, and the
//! interrupted preparations.

use devcore::databases::{DatabaseWorkInfo, InstanceInitializationInfo};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{icons as app_icons, note},
    store::databases as db,
    theme as tokens,
};

use super::{
    icons, rail,
    ui::{self, Tone},
};

/// The running and finished operations, as the Go `devDatabaseTasks` drew
/// them above the instance.
pub(crate) fn tasks_section() -> impl IntoElement {
    view().child(dynamic(|| Some(tasks_body())))
}

fn tasks_body() -> impl IntoElement {
    let tasks = db::tasks();
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(tasks.into_iter().map(|(id, kind, progress, result, running)| {
            let instance_id = id.clone();
            let cancel_id = id.clone();
            let dismiss_id = id.clone();
            let label = if kind == "initialize" {
                "Initialization".to_string()
            } else {
                format!("Database · {kind}")
            };
            let cancel_label = if kind == "initialize" {
                format!("Cancel initialization {instance_id}")
            } else {
                format!("Cancel database operation {instance_id}")
            };
            let dismiss_label = if kind == "initialize" {
                format!("Dismiss initialization result {instance_id}")
            } else {
                format!("Dismiss database result {instance_id}")
            };
            view()
                .test_id(&format!("db-task-{id}"))
                .flex_col()
                .gap(tokens::space(1.0))
                .p(tokens::space(2.0))
                .rounded(move || theme().radius)
                .bg(tokens::inset)
                .child(ui::hairline())
                .child(
                    text(format!("{label} · {id}"))
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(if running {
                        progress.clone()
                    } else {
                        result.clone().unwrap_or_default()
                    })
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || {
                        if running {
                            tokens::success()
                        } else if result.as_deref().is_some_and(|line| line.contains("Error:")) {
                            theme().destructive
                        } else {
                            theme().foreground
                        }
                    })
                    .selectable(true),
                )
                .child(when(running, move || {
                    let cancel_label = cancel_label.clone();
                    let cancel_id = cancel_id.clone();
                    view()
                        .flex_col()
                        .gap(tokens::space(1.0))
                        .child(ui::action_button(
                            cancel_label,
                            Tone::Default,
                            format!("db-task-cancel-{cancel_id}"),
                            false,
                            move || db::cancel_task(&cancel_id),
                        ))
                        .child(
                            text("Canceling does not undo data already written. Review the target before retrying.")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground),
                        )
                }))
                .child(when(!running, move || {
                    let dismiss_label = dismiss_label.clone();
                    let dismiss_id = dismiss_id.clone();
                    ui::action_button(
                        dismiss_label,
                        Tone::Default,
                        format!("db-task-dismiss-{dismiss_id}"),
                        false,
                        move || db::dismiss_task(&dismiss_id),
                    )
                }))
        }))
}

pub(crate) fn database_work() -> impl IntoElement {
    view()
        .test_id("db-work-page")
        .flex_1()
        .min_w(px(0.0))
        .child(dynamic(|| Some(work_body())))
}

fn work_body() -> AnyElement {
    let rows = db::database_work();
    let empty = rows.is_empty();
    view()
        .flex_col()
        .flex_1()
        .child(heading(
            "Unfinished database tasks",
            "Refresh database tasks",
            "db-work-refresh",
            db::refresh_work,
        ))
        .child(notes(
            db::pending_deletions_error(),
            db::database_work_error(),
        ))
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(5.0))
                    .pb(tokens::space(5.0))
                    .child(when(empty, empty_line))
                    .child(rows.into_iter().map(work_row)),
            ),
        )
        .into_element()
}

fn work_row(row: DatabaseWorkInfo) -> impl IntoElement {
    let status = row.status.clone();
    let detail = row.detail.clone();
    let output = row.output.clone();
    let instance_id = row.instance_id.clone();
    let kind = row.kind.clone();
    let database = row.database.clone();
    let id = row.id.clone();
    let revision = row.revision.clone();
    let prefix = db::short_id(&id);
    let running = row.running;
    let can_resolve = row.can_resolve;
    let busy = db::operations_busy();
    let resolve_id = id.clone();
    let label = if running {
        format!("Stop database task {prefix}")
    } else {
        format!("Finish database task {prefix}")
    };
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(ui::hairline())
        .child(
            text(format!("{instance_id} · {kind}"))
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            text(status.clone())
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || {
                    if status == "Succeeded" {
                        tokens::success()
                    } else {
                        tokens::warning()
                    }
                }),
        )
        .child(
            text(detail)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground)
                .selectable(true),
        )
        .child(
            text(format!("Task {id} · Target {database}"))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
        .child(when(!output.is_empty(), {
            let output = output.clone();
            move || ui::field(format!("Output {id}"), ui::read_only_value(output.clone()))
        }))
        .child(ui::action_button(
            label,
            Tone::Primary,
            format!("work-resolve-{prefix}"),
            busy || !can_resolve,
            move || db::resolve_work(&resolve_id, &revision),
        ))
        .child(
            text(
                "Does not undo changes already written. Database files and backup output are kept.",
            )
            .text_size(px(tokens::TEXT_CAPTION))
            .text_color(move || theme().muted_foreground)
            .number_of_lines(2),
        )
}

pub(crate) fn initializations() -> impl IntoElement {
    view()
        .test_id("db-initializations-page")
        .flex_1()
        .min_w(px(0.0))
        .child(dynamic(|| Some(initializations_body())))
}

fn initializations_body() -> AnyElement {
    let rows = db::pending_initializations();
    let empty = rows.is_empty();
    view()
        .flex_col()
        .flex_1()
        .child(heading(
            "Unfinished initializations",
            "Refresh initializations",
            "db-initializations-refresh",
            db::refresh_initializations,
        ))
        .child(notes(
            db::database_work_error(),
            db::pending_initializations_error(),
        ))
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(5.0))
                    .pb(tokens::space(5.0))
                    .child(when(empty, empty_line))
                    .child(rows.into_iter().map(initialization_row)),
            ),
        )
        .into_element()
}

fn initialization_row(row: InstanceInitializationInfo) -> impl IntoElement {
    let status = row.status.clone();
    let detail = row.detail.clone();
    let id = row.id.clone();
    let instance_id = row.instance_id.clone();
    let kind = row.kind.clone();
    let port = row.port;
    let path = row.path.display().to_string();
    let revision = row.revision.clone();
    let prefix = db::short_id(&id);
    let busy = row.busy;
    let can_resolve = row.can_resolve;
    let label = match status.as_str() {
        "Published" => format!("Finish registration {prefix}"),
        "Archived" => format!("Finish archive {prefix}"),
        _ => format!("Stop and archive data {prefix}"),
    };
    let working = db::operations_busy();
    let resolve_id = id.clone();
    let _ = &instance_id;
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .child(ui::hairline())
        .child(
            text(format!("{instance_id} · {status}"))
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            text(format!("{} · port {port}", rail::instance_title(&kind)))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(detail)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .selectable(true),
        )
        .child(ui::field(
            format!("Prepared data {id}"),
            ui::read_only_value(path),
        ))
        .child(ui::action_button(
            label,
            Tone::Primary,
            format!("initialization-resolve-{prefix}"),
            working || busy || !can_resolve,
            move || db::resolve_initialization(&resolve_id, &revision),
        ))
}

fn heading(
    title: &'static str,
    refresh: &'static str,
    refresh_id: &'static str,
    on_refresh: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .h(px(60.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
        .pt(tokens::space(2.0))
        .child(
            text(title)
                .flex_1()
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(ui::action_button(
            refresh,
            Tone::Default,
            refresh_id,
            db::operations_busy(),
            on_refresh,
        ))
        .child(ui::action_button(
            "Back to database settings",
            Tone::Default,
            "list-back",
            db::operations_busy(),
            db::back_to_settings,
        ))
}

fn notes(error: Option<String>, second: Option<String>) -> impl IntoElement {
    view()
        .flex_col()
        .flex_none()
        .child(dynamic(move || {
            db::action_error().map(|message| note(app_icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            error
                .clone()
                .map(|message| note(app_icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            second
                .clone()
                .map(|message| note(app_icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            db::notice().map(|message| note(icons::check(), message, false))
        }))
}

fn empty_line() -> impl IntoElement {
    text("None")
        .text_size(px(tokens::TEXT_SMALL))
        .text_color(move || theme().muted_foreground)
}
