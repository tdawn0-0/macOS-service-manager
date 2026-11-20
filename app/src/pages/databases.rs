//! Database instances: lifecycle, ports, connections and backups (Phase 2).

use nsh_core::prelude::*;

use crate::{components::placeholder, store::Section};

pub(crate) fn databases() -> impl IntoElement {
    placeholder(Section::Databases)
}
