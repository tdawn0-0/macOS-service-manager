//! Devbase's system core, ported from the Go `internal/service`,
//! `internal/launchd` and `internal/devplatform` packages.
//!
//! [`service`] holds the model every source of startup items shares: a
//! Homebrew service, a launch agent or daemon, a login item. [`launchd`] is
//! the first provider, managing the three launchd directories. [`state`] is
//! the file-locked persistence layer: the schema-2 state document, its
//! validators, atomic writes, and the package/resource leases that keep the
//! GUI and CLI from colliding.
//!
//! All operating-system interaction that tests need to fake is injected:
//! [`launchd::Context`] carries a [`launchd::ProcessRunner`], a
//! [`launchd::PrivilegedHelper`], and a [`launchd::Host`]. Production code
//! uses [`launchd::SystemRunner`], [`launchd::SystemHost`] and
//! [`launchd::UnavailableHelper`] (the real privileged helper is a later
//! phase). Locking is never faked: [`state::Store`] uses real `flock(2)` calls.
//!
//! [`operations`] is the cross-process operation journal, [`engine`] the
//! launchd job engine, [`health`] the job-state/readiness layer, and
//! [`shutdown`] the quit preview and database stop plan. [`databases`] holds
//! the instance foundation: data locations, port preflight, staged
//! initialization, the lifecycle, deletion staging and credential files.
//! [`helper`] is the on-demand privileged helper: its HTTP/1.1 server over a
//! launchd-activated Unix socket, the two-layer peer authorization, the
//! install/uninstall client with the single administrator prompt, the helper
//! and proxy-socket property lists, and an in-process helper for tests.

pub mod databases;
pub mod diagnostics;
pub mod engine;
pub mod fs;
pub mod health;
pub mod helper;
pub mod launchd;
pub mod model;
pub mod operations;
pub mod service;
pub mod shell;
pub mod shutdown;
pub mod software;
pub mod state;

#[cfg(test)]
pub(crate) mod test_support_engine;
