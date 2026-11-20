//! Devbase's Rust UI: its root component, which `nsh run`, `nsh package`
//! and `cargo run` mount. Screens are Rust components ([`pages`]), the shell
//! and its parts are in [`components`], the selected section is [`store`], and
//! [`theme`] names the design tokens from the Go app's `theme.go`.

mod components;
mod logs;
mod meta;
mod pages;
mod shell;
mod store;
mod theme;

/// The seams a headless test uses: a launchd provider of its own per source,
/// and a software provider instead of devcore's facade. Call
/// [`use_fake_providers`] / [`use_fake_software`] and the matching
/// `install_provider` before the surface starts.
pub mod testing {
    pub use crate::store::databases::{
        CancelToken, Provider as DatabasesProvider, Snapshot as DatabasesSnapshot,
        install_provider as install_databases_provider, use_fake_provider as use_fake_databases,
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
    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(shell::shell())
}
