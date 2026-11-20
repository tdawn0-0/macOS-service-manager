//! What the shell and its pages share.

mod controls;
pub(crate) mod icons;
mod sidebar;
mod status_dot;

pub(crate) use controls::{icon_button, labeled_button, note};
pub(crate) use sidebar::sidebar;
pub(crate) use status_dot::{state_color, status_dot};
