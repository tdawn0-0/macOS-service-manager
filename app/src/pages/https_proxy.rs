//! HTTPS reverse proxy, local CA and Cloudflare tunnel (Phase 3).

use nsh_core::prelude::*;

use crate::{components::placeholder, store::Section};

pub(crate) fn https_proxy() -> impl IntoElement {
    placeholder(Section::HttpsProxy)
}
