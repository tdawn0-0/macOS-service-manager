//! The app shell: the navigation rail beside the workspace, plus the native
//! macOS application menu, Liquid Glass state, and resizable bottom log drawer.
//! It holds no page logic; the store says which section shows and each page
//! owns its content.

use devcore::service::Source;
use nsh_base::{resizable_panel, theme, v_resizable};
use nsh_core::{
    desktop::{MenuRole, WindowMode},
    element::desktop::{MenuItem, set_application_menu, window_state},
    platform::Os,
    prelude::*,
};

use crate::{
    command_palette,
    components::sidebar,
    dev_logs, editor, guide, meta, pages,
    store::{self, Section},
    theme as tokens,
};

/// Minimum, default, and maximum heights for the resizable live log drawer.
const LOG_DRAWER_MIN_HEIGHT: f32 = 140.0;
const LOG_DRAWER_DEFAULT_HEIGHT: f32 = 260.0;
const LOG_DRAWER_MAX_HEIGHT: f32 = 560.0;

#[derive(Debug, Clone)]
struct LogDrawerHeight(Points);

impl Default for LogDrawerHeight {
    fn default() -> Self {
        Self(px(LOG_DRAWER_DEFAULT_HEIGHT))
    }
}

#[derive(Debug, Clone, Default)]
struct FindTrigger(u64);

/// Triggers `Edit -> Find…` across the shell. Screens with a local search input
/// can react to [`find_revision`]; elsewhere this opens the Command Palette.
pub(crate) fn trigger_find() {
    global::<FindTrigger>().update(|trigger| {
        trigger.0 = trigger.0.wrapping_add(1);
    });
    if store::active_section() != Section::Services && !dev_logs::is_open() {
        command_palette::open();
    }
}

/// Monotonic revision bumped whenever `Edit -> Find…` is invoked.
pub(crate) fn find_revision() -> u64 {
    global::<FindTrigger>().with(|trigger| trigger.0)
}

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
/// checkmark and logs toggle in sync with the workspace.
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
                "File",
                [
                    MenuItem::new("New Job…")
                        .shortcut("mod+n")
                        .on_select(|| editor::open_new(Source::UserAgent)),
                    MenuItem::new("New Database Instance…")
                        .shortcut("mod+shift+n")
                        .on_select(|| {
                            store::select_section(Section::Databases);
                            store::databases::open_new();
                        }),
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
                    MenuItem::separator(),
                    MenuItem::new("Find…")
                        .shortcut("mod+f")
                        .on_select(trigger_find),
                ],
            ),
            MenuItem::submenu(
                "View",
                [
                    MenuItem::new("Command Palette…")
                        .shortcut("mod+k")
                        .on_select(command_palette::toggle),
                    MenuItem::separator(),
                    MenuItem::new("Refresh")
                        .shortcut("mod+r")
                        .on_select(refresh_active_section),
                    MenuItem::new("Toggle Logs")
                        .shortcut("mod+l")
                        .checked(dev_logs::is_open())
                        .on_select(dev_logs::toggle),
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
            MenuItem::submenu(
                "Help",
                [
                    MenuItem::new("Full Disk Access Guide").on_select(guide::open),
                    MenuItem::new("Run Diagnostics").on_select(|| {
                        store::select_section(Section::Diagnostics);
                        store::system::run_diagnostics();
                    }),
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
        .child(
            view()
                .w(px(1.0))
                .flex_none()
                .bg(move || theme().sidebar_border),
        )
        .child(workspace())
}

fn active_page() -> impl IntoElement {
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
        ))
}

/// The workspace: one opaque sheet, one page at a time, chosen by the rail,
/// above the resizable live log drawer.
fn workspace() -> impl IntoElement {
    let drawer_height = global::<LogDrawerHeight>();
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .bg(tokens::sheet)
        .child(
            v_resizable()
                .flex_1()
                .min_h(px(0.0))
                .handle_accessibility_label("Resize live log drawer")
                .handle_style(|s| {
                    s.w_full()
                        .h(px(1.0))
                        .bg(tokens::hairline)
                        .group_hover(|g| g.h(px(2.0)).bg(move || theme().primary.opacity(0.65)))
                        .group_active(|g| g.h(px(2.0)).bg(move || theme().primary))
                })
                .handle_thumb_style(|s| {
                    s.h(px(28.0))
                        .w(px(3.0))
                        .rounded_full()
                        .bg(move || theme().muted_foreground)
                        .opacity(0.0)
                        .group_hover(|g| g.opacity(0.45))
                        .group_active(|g| g.opacity(0.85))
                })
                .child(
                    resizable_panel()
                        .flex_col()
                        .min_size(px(180.0))
                        .child(active_page()),
                )
                .child(
                    resizable_panel()
                        .test_id("log-drawer-panel")
                        .flex_col()
                        .overflow_hidden()
                        .visible(dev_logs::is_open)
                        .fixed(true)
                        .size(move || Some(drawer_height.get().0))
                        .min_size(px(LOG_DRAWER_MIN_HEIGHT))
                        .max_size(Some(px(LOG_DRAWER_MAX_HEIGHT)))
                        .on_resize(move |next: &Points| {
                            drawer_height.set(LogDrawerHeight(px(next
                                .0
                                .clamp(LOG_DRAWER_MIN_HEIGHT, LOG_DRAWER_MAX_HEIGHT))));
                        })
                        .child(when(dev_logs::is_open, dev_logs::drawer)),
                ),
        )
}
