//! Settings: helper, CLI, login items and workspace preferences (Phase 4).

use nsh_core::prelude::*;

use crate::{components::placeholder, store::Section};

pub(crate) fn settings() -> impl IntoElement {
    placeholder(Section::Settings)
}
