//! The launchd provider, ported from the Go `internal/launchd` package.
//!
//! [`LaunchdProvider`] manages one directory of launch agents or daemons;
//! [`Launchd`] holds the three providers the app registers. All process and
//! filesystem interaction the tests need is injected through [`Context`].
//!
//! ```no_run
//! use std::sync::Arc;
//! use devcore::launchd::{
//!     Context, Launchd, PrivilegedHelper, SystemHost, SystemRunner, UnavailableHelper,
//! };
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let ctx = Arc::new(Context::new(
//!     Arc::new(SystemRunner::new()),
//!     Arc::new(UnavailableHelper) as Arc<dyn PrivilegedHelper>,
//!     Arc::new(SystemHost::new()),
//! ));
//! let launchd = Launchd::new(ctx);
//! for (source, items) in launchd.list_all() {
//!     println!("{source}: {} items", items.map(|items| items.len()).unwrap_or(0));
//! }
//! # Ok(())
//! # }
//! ```

mod calendar;
mod error;
mod host;
mod login_agent;
mod parse;
mod plist_file;
mod provider;
mod spec;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_calendar;
#[cfg(test)]
mod tests_edit;
#[cfg(test)]
mod tests_login;
#[cfg(test)]
mod tests_provider;
#[cfg(test)]
mod tests_root;

pub use calendar::{Calendar, CalendarEntry, MAX_CALENDAR_ENTRIES, calendar_from};
pub use error::{ErrorCode, LaunchdError};
pub use host::{
    CommandError, HelperApi, Host, LaunchdRemoveRequest, LaunchdSaveRequest, LaunchdSaveResponse,
    PrivilegedHelper, ProcessRunner, RUN_TIMEOUT, RUN_TIMEOUT_LABEL, SystemHost, SystemRunner,
    UnavailableHelper, WAIT_DELAY,
};
pub use login_agent::{
    LOGIN_AGENT_LABEL, disable_launch_at_login, enable_launch_at_login, launch_at_login_enabled,
    login_agent_plist_path, login_agent_target, resolve_login_executable,
};
pub use parse::{DomainState, Job, parse_print};
pub use provider::{Config, Context, INVALID_KIND, Launchd, LaunchdProvider, SaveOutcome};
pub use spec::{
    Spec, format_env, load_spec, parse_env, parse_lines, parse_list, save_file, shell_args,
    shell_command,
};

// The model the UI consumes is re-exported for convenience; the canonical
// definitions live in `crate::service`.
pub use crate::service::{
    Action, Autostart, Batcher, Detail, Item, LogFile, Provider, Row, Source, State,
};
