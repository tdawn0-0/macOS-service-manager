//! The task journal card: every record newest first, its phase, result and
//! lineage, with retry and dismiss where devcore's journal offers them.

use devcore::operations::{OperationRecord, operation_can_retry};
use nsh_base::theme;
use nsh_core::prelude::*;

use super::{inset_hairline, section_card};
use crate::{
    components::controls::{self as ui, Tone},
    store::system,
    theme as tokens,
};

pub(crate) fn operations_card() -> impl IntoElement {
    section_card(
        "Task history",
        None,
        dynamic(|| Some(operations_body().into_element())),
    )
}

fn operations_body() -> impl IntoElement {
    let records = system::operations();
    let loading = system::operations_loading();
    let error = system::operations_error();
    let action_error = system::operation_action_error();
    let mut rows = view()
        .w_full()
        .flex_col()
        .children(error.map(|error| {
            view()
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .child(ui::plain_note(error, true))
        }))
        .children(action_error.map(|error| {
            view()
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
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
        }));

    for (index, item) in records.into_iter().enumerate() {
        if index > 0 {
            rows = rows.child(inset_hairline());
        }
        rows = rows.child(record(item));
    }

    rows
}

fn format_label_token(raw: &str) -> String {
    let trimmed = raw.trim();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::with_capacity(trimmed.len());
    for ch in first.to_uppercase() {
        out.push(ch);
    }
    out.push_str(chars.as_str());
    out
}

fn operation_title(kind: &str, resource: &str) -> Option<String> {
    let kind = kind.trim();
    let resource = resource.trim();
    match (kind.is_empty(), resource.is_empty()) {
        (true, true) => None,
        (false, true) => Some(format_label_token(kind)),
        (true, false) => Some(resource.to_string()),
        (false, false) => Some(format!("{} · {}", format_label_token(kind), resource)),
    }
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
    let title = operation_title(&record.request.kind, &record.request.resource);
    let phase = format_label_token(&record.phase);
    let has_phase = !phase.is_empty();
    let result = record.result.clone();
    let retry_tip = if record.busy {
        "Another caller owns this task"
    } else {
        "Retry operation"
    };
    let retry_id = id.clone();
    let dismiss_id = id.clone();
    let test_suffix = id.clone();
    let retry_suffix = test_suffix.clone();
    let has_details = !details.is_empty();

    view()
        .test_id(&format!("operation-{test_suffix}"))
        .w_full()
        .flex_col()
        .gap(tokens::space(2.0))
        .px(tokens::space(4.0))
        .py(tokens::space(3.0))
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(ui::status_tag(record.status.clone()))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_row()
                        .items_center()
                        .children(title.map(|title| {
                            text(title)
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(tokens::TEXT_SMALL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground)
                                .truncate()
                        })),
                )
                .child(
                    view()
                        .flex_none()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.5))
                        .child(when(has_phase, move || {
                            view()
                                .flex_none()
                                .flex_row()
                                .items_center()
                                .gap(tokens::space(1.5))
                                .child(
                                    text(phase.clone())
                                        .text_size(px(tokens::TEXT_CAPTION))
                                        .text_color(move || theme().muted_foreground),
                                )
                                .child(
                                    text("·")
                                        .text_size(px(tokens::TEXT_CAPTION))
                                        .text_color(move || theme().muted_foreground),
                                )
                        }))
                        .child(
                            text(format!("Task {}", record.id))
                                .font_family("monospace")
                                .tabular_nums()
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground),
                        ),
                ),
        )
        .child(when(has_details, move || {
            view().flex_col().gap(tokens::space(1.0)).children(
                details
                    .clone()
                    .into_iter()
                    .map(|(label, value)| ui::kv(label, value)),
            )
        }))
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
                .child(when(retryable, move || {
                    let retry_id = retry_id.clone();
                    let retry_suffix = retry_suffix.clone();
                    view()
                        .flex_none()
                        .tooltip(ui::tooltip(retry_tip))
                        .child(ui::action_button(
                            "Retry",
                            Tone::Primary,
                            format!("operation-retry-{retry_suffix}"),
                            busy,
                            move || system::retry_operation(&retry_id),
                        ))
                }))
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
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_title_omits_stray_middot_when_metadata_is_empty() {
        assert_eq!(operation_title("", ""), None);
        assert_eq!(operation_title("  ", " "), None);
        assert_eq!(operation_title("backup", ""), Some("Backup".to_string()));
        assert_eq!(operation_title("", "pg-main"), Some("pg-main".to_string()));
        assert_eq!(
            operation_title("backup", "pg-main"),
            Some("Backup · pg-main".to_string())
        );
        assert_eq!(format_label_token("published"), "Published");
    }
}
