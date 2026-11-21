//! Devbase's Rust UI: its root component, which `nsh run`, `nsh package`
//! and `cargo run` mount. Screens are Rust components ([`pages`]), the shell
//! and its parts are in [`components`], the selected section is [`store`], and
//! [`theme`] names the design tokens from the Go app's `theme.go`.

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

/// The seams a headless test uses: a launchd provider of its own per source,
/// a software provider instead of devcore's facade, and a development
/// provider instead of the shell facade. Call the `use_fake_*` function and
/// the matching `install_provider` before the surface starts.
pub mod testing {
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
        install_persistence as install_guide_persistence, overlay as guide_overlay,
        start as guide_start, use_fake_provider as use_fake_guide, visible as guide_visible,
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
    pub use devcore::databases::ListeningPort as ProxyListeningPort;
    pub use devcore::model::ProxyRule;
    pub use devcore::service::Source;
    pub use devproxy::ca::material::{AuthorityInfo, TrustState};
    pub use devproxy::ca::service::AuthorityStatus;
    pub use devproxy::proxy::activation::ProxyHttpResult;
}

use nsh_base::theme;
use nsh_core::{
    desktop::{TitleBarOptions, TitleBarStyle, WindowAttributes},
    element::desktop::main_window,
    platform::Os,
    prelude::*,
};

/// The app's UI set (`nsh.json` `"rust"`).
#[native_module("devbase/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
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

/// The app's root: the whole surface, on the theme's background. The runner
/// mounts it in the app's main window, whose title, size and minimum size come
/// from `nsh.json`'s `"window"`.
#[component]
pub fn App() -> impl IntoElement {
    let desktop = os().is_desktop();
    store::system::install_quit_handler();
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
        .child(when(store::system::quit_open, store::system::quit_dialog))
        .child(editor::editor_window())
        .child(when(desktop, menubar::popover_window))
        .child(when(desktop, guide::guide_window))
        .child(when(move || !desktop && guide::visible(), guide::overlay))
}
