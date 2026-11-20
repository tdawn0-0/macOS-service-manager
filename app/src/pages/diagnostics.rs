//! Diagnostics: environment, ports, logs and operations.

use nsh_core::prelude::*;

use crate::{components::placeholder, store::Section};

pub(crate) fn diagnostics() -> impl IntoElement {
    placeholder(Section::Diagnostics)
}
