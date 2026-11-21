//! Freedesktop XDG autostart provider (`~/.config/autostart` and `/etc/xdg/autostart`).

pub mod desktop;
pub mod provider;

pub use desktop::{
    AutostartError, DesktopEntry, parse_desktop, quote_exec_arg, update_desktop_enabled,
    validate_desktop_filename,
};
pub use provider::{AutostartConfig, AutostartProvider};
