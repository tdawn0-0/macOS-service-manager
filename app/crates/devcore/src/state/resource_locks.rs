//! Try-only resource leases, ported from the Go `resource_locks.go`.
//!
//! A resource is any `(kind, id)` pair an operation reserves for its lifetime,
//! for example `database instance:blog` or `TCP port:5432`. Keys are sorted by
//! the lowercased `kind:id` string and acquired in that order, so two
//! operations reserving overlapping sets cannot deadlock. Acquisition never
//! waits: a held resource reports [`ResourceBusyError`].

use std::collections::HashSet;
use std::os::unix::fs::OpenOptionsExt;

use super::Store;
use super::error::{ResourceBusyError, StateError};
use super::lock::{FileLease, try_flock};
use crate::fs::{PRIVATE_FILE, create_private_dir_all, digest_bytes};

/// One reserved resource. `kind` is a human-readable category such as
/// `"database instance"` or `"TCP port"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceKey {
    pub kind: String,
    pub id: String,
}

impl ResourceKey {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        ResourceKey {
            kind: kind.into(),
            id: id.into(),
        }
    }

    fn value(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }

    fn sort_key(&self) -> String {
        format!("{}:{}", self.kind.to_lowercase(), self.id)
    }
}

/// A held set of resource leases. Dropping it releases every flock.
#[derive(Debug)]
pub struct ResourceLease {
    _leases: Vec<FileLease>,
}

impl Store {
    /// Reserve all `keys`, or fail with the first busy resource.
    pub fn resource_leases(&self, keys: &[ResourceKey]) -> Result<ResourceLease, StateError> {
        self.resource_leases_cancellable(keys, &|| false)
    }

    /// Reserve all `keys`, stopping when `cancelled` returns true.
    pub fn resource_leases_cancellable(
        &self,
        keys: &[ResourceKey],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ResourceLease, StateError> {
        if !self.root().is_absolute() {
            return Err(StateError::invalid("storage root must be absolute"));
        }
        let dir = self.root().join("resource-locks");
        match std::fs::symlink_metadata(&dir) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(StateError::invalid(
                    "resource lock store must be a plain directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        create_private_dir_all(&dir)?;

        let mut sorted: Vec<&ResourceKey> = keys.iter().collect();
        sorted.sort_by_key(|key| key.sort_key());
        let mut leases = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for key in sorted {
            if cancelled() {
                return Err(StateError::Cancelled);
            }
            let value = key.value();
            if !seen.insert(value.clone()) {
                continue;
            }
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .mode(PRIVATE_FILE)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(dir.join(digest_bytes(value.as_bytes())))?;
            if !file.metadata()?.is_file() {
                return Err(StateError::invalid("resource lease must be a regular file"));
            }
            if !try_flock(&file, libc::LOCK_EX)? {
                return Err(StateError::ResourceBusy(ResourceBusyError {
                    kind: key.kind.clone(),
                    id: key.id.clone(),
                }));
            }
            leases.push(FileLease::new(file));
            if key.kind == "database instance"
                && let Some(check) = &self.database_work_check
            {
                check(&key.id)?;
            }
        }
        Ok(ResourceLease { _leases: leases })
    }
}
