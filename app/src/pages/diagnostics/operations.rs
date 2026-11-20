//! The task journal card, ported from `view_operations.go`: every record
//! newest first, its phase, result and lineage, with retry and dismiss where
//! devcore's journal offers them.

use devcore::operations::{OperationRecord, operation_can_retry};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{store::system, theme as tokens};

use super::ui;

pub(crate) fn operations_card() -> impl IntoElement {
    ui::card(
        "Task history",
        dynamic(|| Some(operations_body().into_element())),
    )
}

fn operations_body() -> impl IntoElement {
    let records = system::operations();
    let loading = system::operations_loading();
    let error = system::operations_error();
    let action_error = system::operation_action_error();
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .children(error.map(|error| ui::note(error, true)))
        .children(action_error.map(|error| ui::note(error, true)))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(
                    text("Tasks")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().muted_foreground),
                )
                .child(view().flex_1())
                .child(ui::action_button(
                    "Refresh tasks",
                    ui::Tone::Default,
                    "operations-refresh",
                    loading,
                    system::refresh_operations,
                )),
        )
        .child(when(loading, || {
            text("Reading task history…")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(records.is_empty() && !loading, || {
            text("No tasks")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .children(records.into_iter().map(record))
}

fn record(record: OperationRecord) -> impl IntoElement {
    let id = record.id.clone();
    let busy = system::operation_busy(&id) || record.busy;
    let retryable = operation_can_retry(&record);
    let mut details = Vec::new();
    if !record.started_at.is_empty() {
        details.push(("Started", record.started_at.clone()));
    }
    if !record.updated_at.is_empty() {
        details.push(("Updated", record.updated_at.clone()));
    }
    if !record.request.database.is_empty() {
        details.push(("Database", record.request.database.clone()));
    }
    if !record.followup.is_empty() {
        details.push(("Follow-up", record.followup.clone()));
    }
    if !record.retry_of.is_empty() {
        details.push(("Retry of", record.retry_of.clone()));
    }
    if !record.artifact.is_empty() && record.request.kind == "backup" {
        let name = std::path::Path::new(&record.artifact)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| record.artifact.clone());
        details.push(("Output", name));
    }
    let retry_label = if record.busy {
        "Retry unavailable: another caller owns this task".to_string()
    } else if !retryable {
        "Retry unavailable for this task".to_string()
    } else {
        "Retry operation".to_string()
    };
    let title = format!("{} · {}", record.request.kind, record.request.resource);
    let result = record.result.clone();
    let retry_id = id.clone();
    let dismiss_id = id.clone();
    let test_suffix = id.clone();
    ui::inset(
        view()
            .test_id(&format!("operation-{test_suffix}"))
            .flex_col()
            .gap(tokens::space(1.5))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(2.0))
                    .child(ui::status_tag(record.status.clone()))
                    .child(
                        text(title)
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(tokens::TEXT_SMALL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(move || theme().foreground)
                            .truncate(),
                    )
                    .child(
                        text(format!("Task {}", record.id))
                            .font_family("monospace")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground),
                    ),
            )
            .child(
                view().flex_col().gap(tokens::space(0.5)).children(
                    details
                        .into_iter()
                        .map(|(label, value)| ui::kv(label, value)),
                ),
            )
            .child(when(!record.phase.is_empty(), {
                let phase = format!("{} · {}", record.status, record.phase);
                move || {
                    text(phase.clone())
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                }
            }))
            .child(when(!result.is_empty(), move || {
                text(result.clone())
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().foreground)
                    .selectable(true)
            }))
            .child(when(record.busy, || {
                ui::note("Another caller owns this task.", false)
            }))
            .child(when(
                !record.artifact.is_empty()
                    && record.request.kind == "backup"
                    && record.status != "Succeeded",
                || {
                    ui::note(
                        "This output may be incomplete. Do not use it as a verified backup.",
                        false,
                    )
                },
            ))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(1.5))
                    .child(ui::action_button(
                        retry_label,
                        ui::Tone::Primary,
                        format!("operation-retry-{test_suffix}"),
                        busy || !retryable,
                        move || system::retry_operation(&retry_id),
                    ))
                    .child(ui::action_button(
                        "Dismiss record (keep all files)",
                        ui::Tone::Default,
                        format!("operation-dismiss-{test_suffix}"),
                        busy,
                        move || system::dismiss_operation(&dismiss_id),
                    )),
            ),
    )
}
