//! Devbase Phase 3: the local HTTPS proxy, its generational certificate
//! authority, and the Cloudflare Worker tunnel.
//!
//! The three subsystems are independent processes in production, but they
//! share this crate's storage and validator contract. Every operating-system
//! boundary — launchd jobs, TLS, keychain trust, sockets, HTTP — is injected
//! so tests use temp directories and fakes and never touch the network, the
//! keychain, or `launchctl`.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let store = devcore::state::Store::new(devcore::state::default_root());
//! let config = devproxy::proxy::rules::read_published_config(store.root())?;
//! println!("{} proxy routes", config.rules.len());
//! # Ok(())
//! # }
//! ```

pub mod ca;
pub mod helpers;
pub mod proxy;
pub mod secrets;
pub mod tunnel;

use std::sync::Once;

/// Install the process-wide rustls crypto provider once.
///
/// The crate graph enables both rustls providers (reqwest and rcgen pull
/// different ones), so rustls refuses to choose automatically. Every TLS entry
/// point calls this before building a configuration.
pub fn ensure_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
