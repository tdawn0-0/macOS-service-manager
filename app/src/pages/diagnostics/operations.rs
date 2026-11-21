//! The task journal card: every record newest first, its phase, result and
//! lineage, with retry and dismiss where devcore's journal offers them.

use devcore::operations::{OperationRecord, operation_can_retry};
use nsh_base::theme;
use nsh_core::prelude::*;

use super::section_card;
use crate::{
    components::controls::{self as ui, Tone},
    store::system,
    theme as tokens,
};

pub(crate) fn operations_card() -> impl IntoElement {
    section_card(
        "Task history",
        Some(
            ui::action_button(
                "Refresh",
                Tone::Default,
                "operations-refresh",
                system::operations_loading,
                system::refresh_operations,
            )
            .into_element(),
        ),
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
        .children(error.map(|error| {
            view()
                .px(tokens::space(4.0))
                .py(tokens::space(2.5))
                .child(ui::plain_note(error, true))
        }))
        .children(action_error.map(|error| {
            view()
                .px(tokens::space(4.0))
                .py(tokens::space(2.5))
                .child(ui::plain_note(error, true))
        }))
        .child(when(loading, || {
            text("Loading…")
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(records.is_empty() && !loading, || {
            text("No tasks")
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .children(
            records
                .into_iter()
                .enumerate()
                .map(|(index, item)| record(item, index == 0)),
        )
}

fn record(record: OperationRecord, is_first: bool) -> impl IntoElement {
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
    let retry_tip = if record.busy {
        "Another caller owns this task"
    } else if !retryable {
        "Retry unavailable"
    } else {
        "Retry operation"
    };
    let title = format!("{} · {}", record.request.kind, record.request.resource);
    let phase = record.phase.clone();
    let result = record.result.clone();
    let retry_id = id.clone();
    let dismiss_id = id.clone();
    let test_suffix = id.clone();

    let row =
        view()
            .test_id(&format!("operation-{test_suffix}"))
            .flex_col()
            .gap(tokens::space(1.5))
            .px(tokens::space(4.0))
            .py(tokens::space(3.0))
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
                    .child(when(!phase.is_empty(), move || {
                        text(phase.clone())
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    }))
                    .child(
                        text(format!("Task {}", record.id))
                            .font_family("monospace")
                            .tabular_nums()
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
            .child(when(!result.is_empty(), move || {
                text(result.clone())
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().foreground)
                    .selectable(true)
            }))
            .child(when(record.busy, || {
                ui::plain_note("Another caller owns this task.", false)
            }))
            .child(when(
                !record.artifact.is_empty()
                    && record.request.kind == "backup"
                    && record.status != "Succeeded",
                || {
                    ui::plain_note(
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
                    .pt(tokens::space(0.5))
                    .child(view().flex_none().tooltip(ui::tooltip(retry_tip)).child(
                        ui::action_button(
                            "Retry",
                            Tone::Primary,
                            format!("operation-retry-{test_suffix}"),
                            busy || !retryable,
                            move || system::retry_operation(&retry_id),
                        ),
                    ))
                    .child(
                        view()
                            .flex_none()
                            .tooltip(ui::tooltip("Dismiss record (files are kept)"))
                            .child(ui::action_button(
                                "Dismiss",
                                Tone::Default,
                                format!("operation-dismiss-{test_suffix}"),
                                busy,
                                move || system::dismiss_operation(&dismiss_id),
                            )),
                    ),
            );
    if is_first {
        row
    } else {
        row.border_t_1().border_color(tokens::hairline)
    }
}
