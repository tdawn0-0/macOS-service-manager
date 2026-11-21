//! Native LaunchAgent / LaunchDaemon property-list editor, ported from Go's
//! `editor.go` and `view_editor.go`.
//!
//! Callers open the editor with [`open_new`] or [`open_edit`], mount
//! [`editor_window`] (or [`editor_panel`] directly in headless tests), and
//! react to saves via [`revision`] / [`last_saved`] or [`on_saved`].
//!
//! All disk and launchd work runs off the UI thread through [`EditorProvider`],
//! which tests replace using [`use_fake_provider`] and [`install_provider`].

pub mod store;
pub mod systemd;
pub mod view;

pub use store::{
    CommandMode, EditorProvider, JOB_KINDS, RestartPolicy, SaveResult, SavedEvent, ScheduleMode,
    clear_saved_callbacks, close, install_provider, is_open, last_saved, notice, on_saved,
    open_edit, open_edit_with_apply, open_new, revision, use_fake_provider,
};
pub use systemd::{
    DefaultSystemdEditorProvider, SYSTEMD_KINDS, SystemdEditorProvider, SystemdFormSnapshot,
    install_provider as install_systemd_provider, unit_editor_panel,
    use_fake_provider as use_fake_systemd_provider,
};
pub use view::{editor_panel, editor_window};
