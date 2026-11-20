//! The permanent-deletion screens (`view_instance_deletion.go`): the review
//! of one instance's data and the staged deletions an interrupted erasure
//! left behind. The preview names the exact path, says what is covered, and
//! refuses external data: it is retained, never deleted.

use devcore::databases::InstanceDeletionInfo;
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

pub(crate) fn deletion_preview() -> impl IntoElement {
    view()
        .test_id("db-deletion-preview")
        .flex_1()
        .min_w(px(0.0))
        .child(preview_body())
}

fn preview_body() -> AnyElement {
    let Some(instance) = db::selected_instance() else {
        return view().into_element();
    };
    let id = instance.id.clone();
    let title = rail::instance_title(&instance.kind);
    view()
        .flex_col()
        .flex_1()
        .child(
            view()
                .h(px(60.0))
                .flex_none()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .px(tokens::space(5.0))
                .pt(tokens::space(2.0))
                .child(
                    view()
                        .flex_col()
                        .gap(tokens::space(0.5))
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            text(format!("Delete {id}"))
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
                .child(ui::action_button(
                    "Refresh deletion preview",
                    Tone::Default,
                    "deletion-refresh",
                    db::operations_busy(),
                    db::refresh_deletion_preview,
                ))
                .child(ui::action_button(
                    "Cancel deletion preview",
                    Tone::Default,
                    "deletion-cancel",
                    db::operations_busy(),
                    db::cancel_deletion_preview,
                )),
        )
        .child(
            view()
                .flex_col()
                .flex_none()
                .child(dynamic(move || {
                    db::action_error().map(|message| note(app_icons::warn(), message, true))
                }))
                .child(dynamic(move || {
                    db::notice().map(|message| note(icons::check(), message, false))
                })),
        )
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .flex_col()
                    .gap(tokens::space(3.0))
                    .px(tokens::space(5.0))
                    .pb(tokens::space(5.0))
                    .child(dynamic(move || {
                        db::deletion_preview_error()
                            .map(|message| note(app_icons::warn(), message, true))
                    }))
                    .child(dynamic(|| {
                        db::deletion_preview_loading().then(|| {
                            text("Reviewing the data path…")
                                .text_size(px(tokens::TEXT_SMALL))
                                .text_color(move || theme().muted_foreground)
                        })
                    }))
                    .child(dynamic(|| preview_card().map(|card| card.into_element())))
                    .child(ui::field(
                        "Confirm instance ID",
                        ui::text_field(
                            "deletion-confirm",
                            "Instance ID",
                            db::confirmation,
                            db::set_confirmation,
                        ),
                    ))
                    .child(dynamic(|| {
                        Some(ui::action_button(
                            "Permanently delete instance data",
                            Tone::Danger,
                            "deletion-delete",
                            !db::can_delete(),
                            db::delete_instance_data,
                        ))
                    })),
            ),
        )
        .into_element()
}

fn preview_card() -> Option<impl IntoElement> {
    let preview = db::deletion_preview()?;
    let entries = preview.entries;
    let mebibytes = preview.bytes as f64 / (1u64 << 20) as f64;
    let detail = preview.detail.clone();
    let can_delete = preview.can_delete;
    let path = preview.path.display().to_string();
    Some(
        ui::card(
            "Permanent deletion",
            view()
                .flex_col()
                .gap(tokens::space(2.5))
                .child(ui::field(
                    "Data path to delete",
                    ui::read_only_value(path),
                ))
                .child(
                    text(format!(
                        "{entries} entries · {mebibytes:.2} MiB logical file size"
                    ))
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().muted_foreground),
                )
                .child(
                    text("Covers every database, configuration and credential in this instance. No backup is made; backups elsewhere are kept.")
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().muted_foreground)
                        .number_of_lines(2),
                )
                .child(
                    text(detail)
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || {
                            if can_delete {
                                theme().destructive
                            } else {
                                tokens::warning()
                            }
                        })
                        .selectable(true),
                ),
        ),
    )
}

pub(crate) fn pending_deletions() -> impl IntoElement {
    view()
        .test_id("db-pending-deletions")
        .flex_1()
        .min_w(px(0.0))
        .child(dynamic(|| Some(deletions_body())))
}

fn deletions_body() -> impl IntoElement {
    let rows = db::pending_deletions();
    let empty = rows.is_empty();
    view()
        .flex_col()
        .flex_1()
        .child(
            view()
                .h(px(60.0))
                .flex_none()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .px(tokens::space(5.0))
                .pt(tokens::space(2.0))
                .child(
                    text("Pending data deletions")
                        .flex_1()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(ui::action_button(
                    "Refresh pending deletions",
                    Tone::Default,
                    "deletions-refresh",
                    db::operations_busy(),
                    db::refresh_deletions,
                ))
                .child(ui::action_button(
                    "Back to database settings",
                    Tone::Default,
                    "deletions-back",
                    db::operations_busy(),
                    db::back_to_settings,
                )),
        )
        .child(
            view()
                .flex_col()
                .flex_none()
                .child(dynamic(move || {
                    db::action_error().map(|message| note(app_icons::warn(), message, true))
                }))
                .child(dynamic(move || {
                    db::pending_deletions_error()
                        .map(|message| note(app_icons::warn(), message, true))
                }))
                .child(dynamic(move || {
                    db::notice().map(|message| note(icons::check(), message, false))
                })),
        )
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(5.0))
                    .pb(tokens::space(5.0))
                    .child(when(empty, || {
                        text("None")
                            .text_size(px(tokens::TEXT_SMALL))
                            .text_color(move || theme().muted_foreground)
                    }))
                    .child(rows.into_iter().map(deletion_row)),
            ),
        )
        .into_element()
}

fn deletion_row(row: InstanceDeletionInfo) -> impl IntoElement {
    let id = row.id.clone();
    let prefix = db::short_id(&id);
    let instance_id = row.instance_id.clone();
    let status = row.status.clone();
    let detail = row.detail.clone();
    let original = row.original_path.clone();
    let staged = row.staged_path.clone();
    let entries = row.entries;
    let bytes = row.bytes;
    let busy = row.busy;
    let can_restore = row.can_restore;
    let can_erase = row.can_erase;
    let revision = row.revision.clone();
    let restore_id = id.clone();
    let restore_revision = revision.clone();
    let finish_revision = revision.clone();
    let finish_instance = instance_id.clone();
    let restore_prefix = prefix.clone();
    let finish_prefix = prefix.clone();
    let busy_action = db::operations_busy();
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
            text(format!("Deletion {id}"))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(detail)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .selectable(true),
        )
        .child(ui::field(
            format!("Original data path {prefix}"),
            ui::read_only_value(original),
        ))
        .child(when(entries > 0, {
            let staged_label = format!("Staged data path {prefix}");
            move || {
                view()
                    .flex_col()
                    .gap(tokens::space(2.0))
                    .child(ui::field(
                        staged_label.clone(),
                        ui::read_only_value(staged.clone()),
                    ))
                    .child(
                        text(format!(
                            "Remaining: {entries} entries · {:.2} MiB",
                            bytes as f64 / (1u64 << 20) as f64
                        ))
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                    )
            }
        }))
        .child(when(busy, || {
            text("Another caller owns this deletion.")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(tokens::warning)
        }))
        .child(ui::action_button(
            format!("Restore original data {restore_prefix}"),
            Tone::Default,
            format!("deletion-restore-{prefix}"),
            busy_action || busy || !can_restore,
            move || db::restore_deletion(&restore_id, &restore_revision),
        ))
        .child(ui::field(
            format!("Confirm deletion of {instance_id}"),
            ui::text_field(
                format!("deletion-confirm-{prefix}"),
                instance_id.clone(),
                db::confirmation,
                db::set_confirmation,
            ),
        ))
        .child(ui::action_button(
            format!("Finish permanent deletion {finish_prefix}"),
            Tone::Danger,
            format!("deletion-finish-{prefix}"),
            busy_action || busy || !can_erase || db::confirmation().trim() != finish_instance,
            move || db::finish_deletion(&id, &finish_revision),
        ))
}
