//! Systemd unit management, `systemctl` parsing, `journalctl` log reading, and
//! [`service::Provider`](crate::service::Provider) / [`service::Batcher`](crate::service::Batcher)
//! implementations for Linux user and system services.

pub mod journal;
pub mod parse;
pub mod provider;
pub mod unit;

pub use journal::{DEFAULT_JOURNAL_LINES, read_journal_logs};
pub use parse::{
    UnitListEntry, UnitShow, map_autostart, map_unit_state, parse_list_unit_files,
    parse_list_units, parse_show, parse_show_job_state, show_to_job_state,
};
pub use provider::{
    DefaultSystemdHelper, SystemdConfig, SystemdHelper, SystemdProvider, SystemdUnitRemove,
    SystemdUnitSave, UnavailableSystemdHelper,
};
pub use unit::{
    SystemdError, UnitSpec, normalize_service_unit_name, parse_unit, quote_systemd_arg,
    split_systemd_words, validate_any_unit_name,
};
