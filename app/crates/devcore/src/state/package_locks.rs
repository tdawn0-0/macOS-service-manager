//! Per-package advisory leases, ported from the Go `package_locks.go`.
//!
//! Work that uses an installed package (starting or preparing an instance,
//! database work, checking files) holds a shared lease; replacement, repair
//! and uninstall take an exclusive one. Exclusive acquisition is try-only and
//! reports [`PackageBusyError`]; shared acquisition waits. A state-lock holder
//! may only try exclusive leases, never wait, so publication cannot deadlock
//! against uninstall.

use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use super::error::{PackageBusyError, StateError};
use super::lock::{FileLease, try_flock};
use super::{LOCK_POLL, Store};
use crate::fs::{PRIVATE_FILE, create_private_dir_all, safe_relative};

const WAIT: &str = "Waiting for package ";

/// A held package lease. Dropping it releases the flock.
#[derive(Debug)]
pub struct PackageLease {
    _lease: FileLease,
}

impl Store {
    /// Take a package lease. Shared leases wait; exclusive leases are
    /// try-only.
    pub fn package_lease(&self, id: &str, exclusive: bool) -> Result<PackageLease, StateError> {
        self.package_lease_cancellable(id, exclusive, &|| false)
    }

    /// Take a package lease, abandoning a waiting shared acquisition when
    /// `cancelled` returns true.
    pub fn package_lease_cancellable(
        &self,
        id: &str,
        exclusive: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<PackageLease, StateError> {
        if !self.root().is_absolute() {
            return Err(StateError::invalid("storage root must be absolute"));
        }
        if !safe_relative(id)
            || Path::new(id).file_name() != Some(std::ffi::OsStr::new(id))
            || !id.contains('@')
        {
            return Err(StateError::invalid(
                "invalid package identity for operation",
            ));
        }
        let dir = self.root().join("package-locks");
        match std::fs::symlink_metadata(&dir) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(StateError::invalid(
                    "package lock store must be a plain directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        create_private_dir_all(&dir)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(PRIVATE_FILE)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(dir.join(id))?;
        if !file.metadata()?.is_file() {
            return Err(StateError::invalid("package lease is not a regular file"));
        }
        let mode = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        };
        let mut announced = false;
        loop {
            if cancelled() {
                return Err(StateError::Cancelled);
            }
            if try_flock(&file, mode)? {
                return Ok(PackageLease {
                    _lease: FileLease::new(file),
                });
            }
            if exclusive {
                return Err(StateError::PackageBusy(PackageBusyError {
                    package_id: id.to_string(),
                }));
            }
            if !announced {
                self.emit(&format!("{WAIT}{id}"));
                announced = true;
            }
            std::thread::sleep(LOCK_POLL);
        }
    }
}
