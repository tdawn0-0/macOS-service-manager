//! Devbase's Rust UI: its root component, which `nsh run`, `nsh package`
//! and `cargo run` mount. Screens are Rust components ([`pages`]), the shell
//! and its parts are in [`components`], the selected section is [`store`], and
//! [`theme`] names the design tokens from the Go app's `theme.go`.

pub mod command_palette;
mod components;
pub mod dev_logs;
pub mod editor;
mod guide;
pub mod launch;
mod logs;
mod menubar;
mod meta;
mod pages;
mod shell;
mod store;
mod theme;
pub mod toast;

use std::collections::BTreeMap;

use nsh_base::theme;
use nsh_core::{
    desktop::{ProgressState, TitleBarOptions, TitleBarStyle, WindowAttributes, WindowProgress},
    element::desktop::main_window,
    notifications::{Notification, NotificationPermission},
    platform::Os,
    prelude::*,
};

/// The seams a headless test uses: a launchd provider of its own per source,
/// a software provider instead of devcore's facade, and a development
/// provider instead of the shell facade. Call the `use_fake_*` function and
/// the matching `install_provider` before the surface starts.
pub mod testing {
    pub use crate::command_palette::{
        close as command_palette_close, is_open as command_palette_is_open,
        open as command_palette_open, toggle as command_palette_toggle,
    };
    pub use crate::dev_logs::{
        DevLogsProvider, EmptyState as DevLogsEmptyState, KnownFile as DevLogsKnownFile,
        LogRead as DevLogsRead, LogSnapshot as DevLogsSnapshot, Stream as DevLogsStream,
        Target as DevLogsTarget, drawer as dev_logs_drawer,
        install_provider as install_dev_logs_provider, use_fake_provider as use_fake_dev_logs,
    };
    pub use crate::editor::{
        CommandMode as EditorCommandMode, EditorProvider, RestartPolicy as EditorRestartPolicy,
        SaveResult as EditorSaveResult, SavedEvent as EditorSavedEvent,
        ScheduleMode as EditorScheduleMode, clear_saved_callbacks as clear_editor_saved_callbacks,
        close as editor_close, editor_panel, editor_window,
        install_provider as install_editor_provider, is_open as editor_is_open,
        last_saved as editor_last_saved, notice as editor_notice, on_saved as editor_on_saved,
        open_edit as editor_open_edit, open_edit_with_apply as editor_open_edit_with_apply,
        open_new as editor_open_new, revision as editor_revision,
        use_fake_provider as use_fake_editor,
    };
    pub use crate::guide::{
        Access as GuideAccess, Page as GuidePage, Persistence as GuidePersistence,
        close as guide_close, install_access as install_guide_access,
        install_persistence as install_guide_persistence, open as guide_open,
        overlay as guide_overlay, start as guide_start, use_fake_provider as use_fake_guide,
        visible as guide_visible,
    };
    pub use crate::menubar::{
        Action as MenubarAction, GLOBAL_SHORTCUT as MENUBAR_GLOBAL_SHORTCUT,
        GLOBAL_SHORTCUT_LABEL as MENUBAR_GLOBAL_SHORTCUT_LABEL, InstanceRow as MenubarInstance,
        MenuEntry as MenubarMenuEntry, Panel as MenubarPanel, PanelInput as MenubarInput,
        Row as MenubarRow, RowKind as MenubarRowKind, ServiceRow as MenubarService,
        anchor_position_in as menubar_anchor_position_in, close_popover as menubar_close_popover,
        dispatch as menubar_dispatch, load as menubar_load, menu_entries as menubar_menu_entries,
        panel as menubar_panel, popover_content as menubar_popover,
        popover_open as menubar_popover_open,
        should_reopen_after_blur as menubar_should_reopen_after_blur,
        title_runs as menubar_title_runs,
        toggle_popover_shortcut as menubar_toggle_popover_shortcut, use_fake_menubar,
    };
    pub use crate::store::databases::{
        CancelToken, Provider as DatabasesProvider, Snapshot as DatabasesSnapshot,
        install_provider as install_databases_provider, use_fake_provider as use_fake_databases,
    };
    pub use crate::store::development::{
        JavaVersion as DevelopmentJava, Mode as DevelopmentMode, NodeVersion as DevelopmentNode,
        Provider as DevelopmentProvider, PythonVersion as DevelopmentPython,
        ReleaseList as DevelopmentReleases, Selection as DevelopmentSelection,
        ShellRow as DevelopmentShell, Snapshot as DevelopmentSnapshot, Tab as DevelopmentTab,
        install_provider as install_development_provider,
        use_fake_provider as use_fake_development,
    };
    pub use crate::store::proxy::{
        AuthorityAction, Provider as ProxyProvider, RuleDraft as ProxyRuleDraft,
        SavedCloudflare as ProxySavedCloudflare, Snapshot as ProxySnapshot,
        Workers as ProxyWorkers, install_provider as install_proxy_provider,
        use_fake_provider as use_fake_proxy,
    };
    pub use crate::store::services::{
        install_login_item_adder, install_provider, use_fake_providers,
    };

    /// Triggers a refresh of `source` in the services store during headless tests.
    pub fn refresh_services(source: Source) {
        crate::store::services::refresh(source);
    }
    pub use crate::store::software::{
        Provider as SoftwareProvider, Snapshot as SoftwareSnapshot, SoftwareKind,
        install_provider as install_software_provider, use_fake_provider as use_fake_software,
    };
    pub use crate::store::system::{
        CacheClear as SystemCacheClear, CacheInfo as SystemCacheInfo, CliChange as SystemCliChange,
        CliStatus as SystemCliStatus, DiagnosticsProvider as SystemDiagnosticsProvider,
        HelperChange as SystemHelperChange, HelperStatus as SystemHelperStatus,
        LoginChange as SystemLoginChange, OperationsProvider as SystemOperationsProvider,
        PortsProvider as SystemPortsProvider, SettingsProvider as SystemSettingsProvider,
        SettingsSnapshot as SystemSettingsSnapshot,
        install_diagnostics_provider as install_system_diagnostics_provider,
        install_operations_provider as install_system_operations_provider,
        install_ports_provider as install_system_ports_provider,
        install_settings_provider as install_system_settings_provider,
        use_fake_provider as use_fake_system,
    };
    pub use crate::store::system::{quit_open, request_quit as system_request_quit};
    pub use crate::toast::{
        dismiss_all as toast_dismiss_all, error as toast_error, info as toast_info,
        success as toast_success,
    };
    pub use devcore::databases::ListeningPort as ProxyListeningPort;
    pub use devcore::model::ProxyRule;
    pub use devcore::service::Source;
    pub use devproxy::ca::material::{AuthorityInfo, TrustState};
    pub use devproxy::ca::service::AuthorityStatus;
    pub use devproxy::proxy::activation::ProxyHttpResult;
}

/// The app's UI set (`nsh.json` `"rust"`).
#[native_module("devbase/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// Maximum remembered notification-id → section routes per surface.
const MAX_NOTIFICATION_ROUTES: usize = 64;

#[derive(Debug, Clone, Default)]
struct NotificationRoutes(BTreeMap<String, store::Section>);

/// Whether OS-level desktop effects (Dock progress, Dock badge, Dock attention,
/// and macOS Notification Center alerts) should be emitted. Always `false` off
/// desktops or when fake-mode test providers are installed.
fn desktop_effects_enabled() -> bool {
    os().is_desktop() && !store::system::is_fake_mode() && !menubar::is_fake_mode()
}

/// Records a task notification and, on desktop outside fake/headless test mode,
/// posts a native macOS notification and bounces the Dock icon when the main
/// window is unfocused. Pressing the notification focuses the window and
/// navigates to `section`.
pub(crate) fn notify_task(
    id: &str,
    title: &str,
    body: &str,
    section: store::Section,
    failed: bool,
) {
    if let Some(routes) = try_global::<NotificationRoutes>() {
        routes.update(|map| {
            if map.0.len() >= MAX_NOTIFICATION_ROUTES
                && !map.0.contains_key(id)
                && let Some(oldest) = map.0.keys().next().cloned()
            {
                map.0.remove(&oldest);
            }
            map.0.insert(id.to_string(), section);
        });
    }

    if !desktop_effects_enabled() {
        return;
    }

    let main = main_window();
    let focused = main.state().is_some_and(|state| state.focused);
    if !focused {
        main.request_attention(failed);
    }

    let id = id.to_string();
    let title = title.to_string();
    let body = body.to_string();
    Owner::detached().run(move || {
        spawn(async move {
            let permission = match notification_permission().await {
                NotificationPermission::Default => request_notification_permission().await,
                other => other,
            };
            if permission == NotificationPermission::Granted {
                let _ = show_notification(Notification::new(id, title, body)).await;
            }
        });
    });
}

fn handle_notification_press(id: &str) {
    let section = try_global::<NotificationRoutes>()
        .and_then(|routes| routes.with_untracked(|map| map.0.get(id).copied()));
    if let Some(target) = section {
        store::select_section(target);
    }
    let main = main_window();
    main.show();
    main.focus();
}

/// Configures the main desktop window's native chrome: hidden title bar with
/// 52pt traffic-light strip, hide-to-menubar on close, and macOS 26 Liquid
/// Glass transparency that falls back to opaque in full screen.
fn configure_main_window() {
    let mut bar = TitleBarOptions::default();
    bar.style = TitleBarStyle::Hidden;
    bar.height = Some(px(theme::TITLE_BAR_HEIGHT));

    let mut attrs = WindowAttributes::default();
    attrs.title_bar = Some(bar);
    attrs.close_on_press = Some(false);
    if os() == Os::MacOs {
        attrs.transparent = Some(shell::sidebar_glass_active());
    }
    let main = main_window();
    main.set(attrs);
    main.on_close_press(|_| {
        main_window().hide();
    });

    if os() == Os::MacOs {
        effect(|| {
            let mut update = WindowAttributes::default();
            update.transparent = Some(shell::sidebar_glass_active());
            main_window().set(update);
        });
    }
}

/// Installs desktop-only Dock progress, Dock failure badge, and macOS
/// notification-press routing. Strictly skipped in fake/headless test mode.
fn install_desktop_effects() {
    if !desktop_effects_enabled() {
        return;
    }

    let last_busy = std::cell::Cell::new(false);
    effect(move || {
        let busy = store::software::operation_busy() || store::databases::operations_busy();
        if busy != last_busy.get() {
            last_busy.set(busy);
            let mut update = WindowAttributes::default();
            update.progress = if busy {
                let mut progress = WindowProgress::default();
                progress.state = ProgressState::Indeterminate;
                Some(Some(progress))
            } else {
                Some(None)
            };
            main_window().set(update);
        }
    });

    let last_badge = std::cell::Cell::new(0u32);
    effect(move || {
        let failed = menubar::failed_count();
        if failed != last_badge.get() {
            last_badge.set(failed);
            set_badge(failed);
        }
    });

    spawn(async move {
        loop {
            let id = next_notification_press().await;
            handle_notification_press(&id);
        }
    });
}

/// The app's root: the whole surface, on the theme's background. The runner
/// mounts it in the app's main window, whose title, size and minimum size come
/// from `nsh.json`'s `"window"`.
#[component]
pub fn App() -> impl IntoElement {
    let desktop = os().is_desktop();
    store::system::install_quit_handler();
    on_key("mod+k", |_| command_palette::toggle());
    {
        let seen_editor_rev = std::cell::Cell::new(untracked(editor::revision));
        effect(move || {
            let rev = editor::revision();
            if rev != seen_editor_rev.get() {
                seen_editor_rev.set(rev);
                if let Some(saved) = untracked(editor::last_saved) {
                    store::services::handle_editor_saved(saved);
                } else {
                    store::services::refresh_active();
                }
            }
        });
    }
    if desktop {
        configure_main_window();
        shell::install_application_menu();
        menubar::install();
        guide::start();
        install_desktop_effects();
    }
    view()
        .flex_1()
        .relative()
        .bg(move || {
            if shell::sidebar_glass_active() {
                AdaptiveColor::TRANSPARENT
            } else {
                theme().background
            }
        })
        .text_color(move || theme().foreground)
        .child(shell::shell())
        .child(toast::stack())
        .child(when(command_palette::is_open, command_palette::overlay))
        .child(when(store::system::quit_open, store::system::quit_dialog))
        .child(editor::editor_window())
        .child(when(desktop, menubar::popover_window))
        .child(when(desktop, guide::guide_window))
        .child(when(move || !desktop && guide::visible(), guide::overlay))
}
