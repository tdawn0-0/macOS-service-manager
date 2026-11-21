//! Workspace live log drawer, ported from `view_dev_logs.go`.
//!
//! Exposes a self-contained bottom drawer ([`drawer`]) and reactive store
//! commands ([`open`], [`open_default`], [`toggle`], [`close`], [`is_open`],
//! [`select_target`], [`set_stream`], [`set_paused`], [`refresh`], [`tick`],
//! [`copy_logs`], [`open_in_console`]) ready to wire into the workspace shell
//! and toolbar buttons.

mod store;
mod view;

pub use store::{
    DEFAULT_JOB_ID, DevLogsProvider, EmptyState, FilteredLine, INVALID_TARGET_ERROR, KnownFile,
    LogRead, LogSnapshot, Stream, SystemDevLogsProvider, Target, active_job_id, active_path,
    active_stream, active_title, can_copy, close, copy_logs, empty_state, error, filter_lines,
    filtered_lines, install_provider, is_open, is_paused, is_valid_job_id, lines, notice, open,
    open_default, open_in_console, query, refresh, select_target, set_paused, set_query,
    set_stream, target_title, targets, tick, toggle, use_fake_provider,
};
pub use view::drawer;
