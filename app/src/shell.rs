//! The app shell: the navigation rail beside the workspace, plus the native
//! macOS application menu and Liquid Glass state. It holds no page logic; the
//! store says which section shows and each page owns its content.

use nsh_core::{
    desktop::{MenuRole, WindowMode},
    element::desktop::{MenuItem, set_application_menu, window_state},
    platform::Os,
    prelude::*,
};

use crate::{
    components::sidebar,
    meta, pages,
    store::{self, Section},
    theme as tokens,
};

/// Whether the left sidebar renders over macOS 26 Liquid Glass rather than
/// its solid fallback fill. Turns off automatically in full screen and off
/// macOS (including default headless tests).
pub(crate) fn sidebar_glass_active() -> bool {
    os() == Os::MacOs
        && !matches!(
            window_state().map(|state| state.mode),
            Some(WindowMode::FullScreen)
        )
}

/// Refreshes whichever section is currently active in the workspace.
pub(crate) fn refresh_active_section() {
    match store::active_section() {
        Section::Services => store::services::refresh_active(),
        Section::Development => {
            store::development::refresh();
            store::software::refresh();
        }
        Section::Databases => store::databases::refresh(),
        Section::HttpsProxy => store::proxy::refresh(),
        Section::Diagnostics => store::system::refresh(),
        Section::Settings => store::system::refresh_settings(),
    }
}

/// Installs the native macOS application menu bar and keeps the active section
/// checkmark in sync with the workspace.
pub(crate) fn install_application_menu() {
    if !os().is_desktop() {
        return;
    }
    effect(|| {
        let active = store::active_section();
        let section_item = |section: Section, shortcut: &str| {
            MenuItem::new(section.label())
                .shortcut(shortcut)
                .checked(active == section)
                .on_select(move || store::select_section(section))
        };
        set_application_menu(vec![
            MenuItem::submenu(
                meta::NAME,
                [
                    MenuItem::role(MenuRole::About),
                    MenuItem::separator(),
                    MenuItem::new("Settings…")
                        .shortcut("mod+,")
                        .on_select(|| store::select_section(Section::Settings)),
                    MenuItem::separator(),
                    MenuItem::role(MenuRole::Hide),
                    MenuItem::role(MenuRole::HideOthers),
                    MenuItem::role(MenuRole::ShowAll),
                    MenuItem::separator(),
                    MenuItem::role(MenuRole::Quit),
                ],
            ),
            MenuItem::submenu(
                "Edit",
                [
                    MenuItem::role(MenuRole::Undo),
                    MenuItem::role(MenuRole::Redo),
                    MenuItem::separator(),
                    MenuItem::role(MenuRole::Cut),
                    MenuItem::role(MenuRole::Copy),
                    MenuItem::role(MenuRole::Paste),
                    MenuItem::role(MenuRole::SelectAll),
                ],
            ),
            MenuItem::submenu(
                "View",
                [
                    MenuItem::new("Refresh")
                        .shortcut("mod+r")
                        .on_select(refresh_active_section),
                    MenuItem::new("Toggle Logs")
                        .shortcut("mod+l")
                        .checked(crate::dev_logs::is_open())
                        .on_select(crate::dev_logs::toggle),
                    MenuItem::separator(),
                    section_item(Section::Services, "mod+1"),
                    section_item(Section::Development, "mod+2"),
                    section_item(Section::Databases, "mod+3"),
                    section_item(Section::HttpsProxy, "mod+4"),
                    section_item(Section::Diagnostics, "mod+5"),
                    section_item(Section::Settings, "mod+6"),
                    MenuItem::separator(),
                    MenuItem::role(MenuRole::ToggleFullScreen),
                ],
            ),
            MenuItem::submenu(
                "Window",
                [
                    MenuItem::role(MenuRole::Minimize),
                    MenuItem::role(MenuRole::Zoom),
                    MenuItem::separator(),
                    MenuItem::role(MenuRole::Close),
                ],
            ),
        ]);
    });
}

pub(crate) fn shell() -> impl IntoElement {
    view()
        .flex_1()
        .flex_row()
        .child(sidebar())
        .child(view().w(px(1.0)).flex_none().bg(tokens::hairline))
        .child(workspace())
}

/// The workspace: one opaque sheet, one page at a time, chosen by the rail,
/// plus the collapsible live log drawer at the bottom.
fn workspace() -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .bg(tokens::sheet)
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_col()
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
                )),
        )
        .child(when(crate::dev_logs::is_open, crate::dev_logs::drawer))
}
