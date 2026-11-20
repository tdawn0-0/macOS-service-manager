//! Quit preview and shutdown, ported from the Go `shutdown.go`.
//!
//! The preview binds the exact state document to the workspace root with a
//! SHA-256 fingerprint. Shutdown refuses a stale plan, leases every configured
//! database instance (real try-only flocks), stops each `db-<id>` job, and
//! verifies the launchd records are gone. Proxy, tunnel and log-maintenance
//! jobs are deliberately outside this scope.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::engine::Engine;
use crate::fs::clean_path;
use crate::model::State;
use crate::state::{ResourceKey, StateError, Store, Tx};

/// One database resource in the quit preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownResource {
    pub id: String,
    pub name: String,
}

/// The reviewed shutdown scope and its state fingerprint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShutdownPlan {
    pub instances: Vec<ShutdownResource>,
    pub fingerprint: String,
}

/// Reserved for future shutdown options; the Go struct is empty for now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShutdownOptions {}

/// Fingerprint of the state bytes bound to the root, exactly as the Go code
/// builds it: `sha256(clean(root) + "\0" + json(state))`.
pub fn shutdown_fingerprint(root: &Path, state: &State) -> String {
    use std::os::unix::ffi::OsStrExt;
    let bytes = serde_json::to_vec(state).unwrap_or_default();
    let root = clean_path(root);
    let mut raw = root.as_os_str().as_bytes().to_vec();
    raw.push(0);
    raw.extend_from_slice(&bytes);
    crate::fs::encode_hex(&Sha256::digest(&raw))
}

/// A shutdown over one store and engine.
pub struct Shutdown<'a> {
    store: &'a Store,
    engine: &'a Engine,
}

impl<'a> Shutdown<'a> {
    pub fn new(store: &'a Store, engine: &'a Engine) -> Self {
        Shutdown { store, engine }
    }

    /// List every configured database resource, including stopped ones. A
    /// client may start one before shutdown acquires the state lock.
    pub fn preview(&self, cancelled: &dyn Fn() -> bool) -> Result<ShutdownPlan, StateError> {
        if cancelled() {
            return Err(StateError::Cancelled);
        }
        let state = self.store.read()?;
        let fingerprint = shutdown_fingerprint(self.store.root(), &state);
        Ok(ShutdownPlan {
            fingerprint,
            instances: state
                .instances
                .iter()
                .map(|(id, instance)| ShutdownResource {
                    id: id.clone(),
                    name: instance.kind.clone(),
                })
                .collect(),
        })
    }

    /// Stop the reviewed scope. The fingerprint is checked again under the
    /// state lock, then every instance lease is held while its job stops.
    pub fn execute(
        &self,
        plan: &ShutdownPlan,
        _options: &ShutdownOptions,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        self.store
            .with_state_transaction_cancellable(cancelled, |state| {
                if plan.fingerprint.is_empty()
                    || plan.fingerprint != shutdown_fingerprint(self.store.root(), state)
                {
                    return Err(StateError::invalid(
                        "workspace changed after the quit preview; review the updated scope and retry",
                    ));
                }
                let keys: Vec<ResourceKey> = state
                    .instances
                    .keys()
                    .map(|id| ResourceKey::new("database instance", id.clone()))
                    .collect();
                let _lease = self
                    .store
                    .resource_leases_cancellable(&keys, cancelled)?;
                for id in state.instances.keys() {
                    self.store.emit(&format!("Stopping database {id}"));
                    self.engine
                        .stop_job(&format!("db-{id}"), cancelled)
                        .map_err(|error| {
                            StateError::invalid(format!("stop database {id}: {error}"))
                        })?;
                }
                // A failed launchctl read is not proof that a job is absent.
                // Verify the whole chosen scope before the GUI may finish
                // quitting.
                for id in state.instances.keys() {
                    let target = format!("{}/{}", self.engine.domain(), self.engine.label(&format!("db-{id}")));
                    self.verify_target(&target, cancelled)?;
                }
                Ok(Tx::new(()))
            })
    }

    /// Check that a managed job is no longer registered. A launchd read that
    /// fails for any reason other than "not found" is not proof.
    pub fn verify_target(
        &self,
        target: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        if cancelled() {
            return Err(StateError::Cancelled);
        }
        let result = self.engine.run_with_timeout(
            "launchctl",
            &["print", target],
            std::time::Duration::from_secs(1),
        );
        match result {
            Ok(_) => Err(StateError::invalid(format!(
                "managed service is still registered: {target}"
            ))),
            Err(error) if error.to_string().contains("context deadline exceeded") => Err(
                StateError::invalid(format!("could not verify stopped service: {error}")),
            ),
            Err(error) => {
                let message = error.to_string();
                if message.contains("Could not find service")
                    || message.contains("Could not find specified service")
                {
                    Ok(())
                } else {
                    Err(StateError::invalid(format!(
                        "could not verify stopped service {target}: {error}"
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "shutdown_tests.rs"]
mod tests;
