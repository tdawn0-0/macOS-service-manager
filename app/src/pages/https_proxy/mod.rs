//! HTTPS reverse proxy, local CA and Cloudflare tunnel (Phase 3), ported from
//! `view_https_proxy.go`, `view_proxy_activation.go` and `view_authority.go`.
//!
//! The page reads [`crate::store::proxy`] and never calls devproxy itself:
//! saving rules, applying the proxy, Cloudflare settings, trust actions and
//! activation checks are all tasks that write the store when they answer.

mod authority;
mod icons;
mod proxy;

use nsh_core::prelude::*;

use crate::store::proxy as state;

pub(crate) fn https_proxy() -> impl IntoElement {
    // The page is made once; reading the development home is a task, not a
    // binding or render work.
    state::load();

    view()
        .flex_1()
        .min_w(px(0.0))
        .child(when(move || !state::authority_open(), proxy::rules_page))
        .child(when(state::authority_open, authority::authority_page))
}
