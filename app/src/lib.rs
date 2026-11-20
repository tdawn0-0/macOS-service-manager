//! Devbase's Rust UI: its root component, which `nsh run`, `nsh package`
//! and `cargo run` mount. Screens are Rust components ([`pages`]), the shell
//! and its parts are in [`components`], the selected section is [`store`], and
//! [`theme`] names the design tokens from the Go app's `theme.go`.

mod components;
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
    pub use crate::guide::{
        Access as GuideAccess, Page as GuidePage, Persistence as GuidePersistence,
        close as guide_close, install_access as install_guide_access,
        install_persistence as install_guide_persistence, overlay as guide_overlay,
        start as guide_start, use_fake_provider as use_fake_guide, visible as guide_visible,
    };
    pub use crate::menubar::{
        Action as MenubarAction, InstanceRow as MenubarInstance, Panel as MenubarPanel,
        PanelInput as MenubarInput, Row as MenubarRow, RowKind as MenubarRowKind,
        dispatch as menubar_dispatch, load as menubar_load, panel as menubar_panel,
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
    pub use crate::store::services::{install_provider, use_fake_providers};
    pub use crate::store::software::{
        Provider as SoftwareProvider, Snapshot as SoftwareSnapshot, SoftwareKind,
        install_provider as install_software_provider, use_fake_provider as use_fake_software,
    };
    pub use crate::store::system::quit_open;
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
    pub use devcore::databases::ListeningPort as ProxyListeningPort;
    pub use devcore::model::ProxyRule;
    pub use devcore::service::Source;
    pub use devproxy::ca::material::{AuthorityInfo, TrustState};
    pub use devproxy::ca::service::AuthorityStatus;
    pub use devproxy::proxy::activation::ProxyHttpResult;
}

use nsh_base::theme;
use nsh_core::prelude::*;

/// The app's UI set (`nsh.json` `"rust"`).
#[native_module("devbase/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// The app's root: the whole surface, on the theme's background. The runner
/// mounts it in the app's main window, whose title, size and minimum size come
/// from `nsh.json`'s `"window"`.
#[component]
pub fn App() -> impl IntoElement {
    store::system::install_quit_handler();
    if os().is_desktop() {
        menubar::install();
        guide::start();
    }
    view()
        .flex_1()
        .relative()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(shell::shell())
        .child(when(
            || store::system::quit_open(),
            store::system::quit_dialog,
        ))
        .child(when(guide::visible, guide::overlay))
}
