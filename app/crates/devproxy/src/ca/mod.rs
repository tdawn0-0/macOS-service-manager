//! The generational local certificate authority.
//!
//! Ported from `authority.go` and the CA parts of `proxy.go`. Storage lives in
//! [`store`], certificate material in [`material`], keychain trust in
//! [`trust`], and the prepare/trust/activate/discard/restore state machine in
//! [`service`]. Tests use temp directories and a fake `ProcessRunner`; nothing
//! here invokes the real keychain unless production code asks it to.

pub mod material;
pub mod service;
pub mod store;
pub mod trust;

use std::io;

pub use material::{
    AUTHORITY_NAME, AuthorityInfo, AuthorityMaterial, AuthorityRecord, LeafCertificate, TrustState,
    new_authority, new_leaf, parse_authority, sha1_fingerprint,
};
pub use service::{
    AlwaysStopped, AuthorityService, AuthorityStatus, FixedProxyGate, ProxyStopCheck,
};
pub use store::{
    ALIASES, AuthorityStore, CURRENT_POINTER, PREPARED_POINTER, PREVIOUS_POINTER, valid_id,
};
pub use trust::{
    LINUX_CA_ANCHOR_NAME, LinuxCaHelper, LinuxTrustStore, SecurityTrust, TrustStore,
    default_trust_store, keychain_item_missing,
};

/// Everything the authority state machine can fail with.
#[derive(Debug, thiserror::Error)]
pub enum CaError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Command(String),
    #[error("{0}")]
    Untrusted(String),
    #[error("cancelled")]
    Cancelled,
}
