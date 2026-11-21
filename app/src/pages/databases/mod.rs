//! Database instances: the managed engines, their lifecycle, connection
//! profiles, backups, permanent deletion and the unfinished work lists.
//!
//! The page reads [`crate::store::databases`] and never calls devcore itself:
//! every read, creation, lifecycle action, backup, restore or deletion is a
//! task that writes the store when it answers.

mod deletion;
mod detail;
mod icons;
mod new;
mod rail;
mod tasks;

use nsh_core::prelude::*;

pub(crate) fn databases() -> impl IntoElement {
    // The page is made once; reading the development home is a task, not a
    // binding or render work.
    crate::store::databases::load();

    view()
        .flex_1()
        .flex_row()
        .min_w(px(0.0))
        .child(rail::rail())
        .child(content())
}

/// One screen at a time, chosen by the store's mode, exactly where the Go
/// `devDatabases` dispatches to its own screens.
fn content() -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .child(when(
            move || crate::store::databases::mode() == crate::store::databases::Mode::Instance,
            detail::detail_pane,
        ))
        .child(when(
            move || crate::store::databases::mode() == crate::store::databases::Mode::New,
            new::new_pane,
        ))
        .child(when(
            move || {
                crate::store::databases::mode() == crate::store::databases::Mode::DeletionPreview
            },
            deletion::deletion_preview,
        ))
        .child(when(
            move || crate::store::databases::mode() == crate::store::databases::Mode::Deletions,
            deletion::pending_deletions,
        ))
        .child(when(
            move || crate::store::databases::mode() == crate::store::databases::Mode::Work,
            tasks::database_work,
        ))
        .child(when(
            move || {
                crate::store::databases::mode() == crate::store::databases::Mode::Initializations
            },
            tasks::initializations,
        ))
}
