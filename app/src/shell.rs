//! The app shell: the navigation rail beside the workspace. It holds no page
//! logic; the store says which section shows and each page owns its content.

use nsh_core::prelude::*;

use crate::{
    components::sidebar,
    pages,
    store::{self, Section},
    theme as tokens,
};

pub(crate) fn shell() -> impl IntoElement {
    view()
        .flex_1()
        .flex_row()
        .child(sidebar())
        .child(workspace())
}

/// The workspace: one opaque sheet, one page at a time, chosen by the rail.
fn workspace() -> impl IntoElement {
    view()
        .flex_1()
        .flex_col()
        .bg(move || tokens::sheet())
        .child(when(
            move || store::active_section() == Section::Services,
            pages::services,
        ))
        .child(when(
            move || store::active_section() == Section::Development,
            pages::development,
        ))
        .child(when(
            move || store::active_section() == Section::Databases,
            pages::databases,
        ))
        .child(when(
            move || store::active_section() == Section::HttpsProxy,
            pages::https_proxy,
        ))
        .child(when(
            move || store::active_section() == Section::Diagnostics,
            pages::diagnostics,
        ))
        .child(when(
            move || store::active_section() == Section::Settings,
            pages::settings,
        ))
}
