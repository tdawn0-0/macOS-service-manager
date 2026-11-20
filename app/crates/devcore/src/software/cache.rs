//! Download cache inspection and clearing, ported from the Go `cache.go:19-131`.
//!
//! Only regular files directly in the download directory are counted or
//! removed; directories and symlinks are ignored. Clearing refuses while any
//! package lease is exclusively held (an install, repair or uninstall), so an
//! archive can never disappear underneath an active operation.

use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use serde::Serialize;

use super::{Cancelled, Software, SoftwareError};
use crate::fs::{safe_relative, sync_directory};
use crate::state::{PackageBusyError, StateError};

/// Cache contents: the directory, number of archives, and total bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadCacheInfo {
    pub directory: PathBuf,
    pub files: i64,
    pub bytes: i64,
}

impl Software {
    /// Report cached installation archives.
    pub fn download_cache_info(&self) -> Result<DownloadCacheInfo, SoftwareError> {
        let directory = self.store().download_directory();
        let mut info = DownloadCacheInfo {
            directory: directory.clone(),
            ..DownloadCacheInfo::default()
        };
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(info),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            info.files += 1;
            info.bytes += metadata.len() as i64;
        }
        Ok(info)
    }

    /// Delete cached archives. Installed packages and databases are untouched.
    pub fn clear_download_cache(
        &self,
        cancelled: Cancelled<'_>,
    ) -> Result<DownloadCacheInfo, SoftwareError> {
        if cancelled() {
            return Err(SoftwareError::State(StateError::Cancelled));
        }
        ensure_no_active_package_lock(self.store().root())?;
        let directory = self.store().download_directory();
        let mut removed = DownloadCacheInfo {
            directory: directory.clone(),
            ..DownloadCacheInfo::default()
        };
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(removed),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            if cancelled() {
                return Err(SoftwareError::State(StateError::Cancelled));
            }
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !safe_relative(name) {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
            removed.files += 1;
            removed.bytes += metadata.len() as i64;
        }
        sync_directory(&directory)?;
        Ok(removed)
    }
}

fn ensure_no_active_package_lock(root: &std::path::Path) -> Result<(), SoftwareError> {
    let directory = root.join("package-locks");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if entry.file_type()?.is_dir() || !safe_relative(name) {
            continue;
        }
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(entry.path())
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !file.metadata()?.is_file() {
            continue;
        }
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
        if result == 0 {
            unsafe {
                libc::flock(file.as_raw_fd(), libc::LOCK_UN);
            }
            continue;
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(SoftwareError::State(StateError::PackageBusy(
                PackageBusyError {
                    package_id: name.to_string(),
                },
            )));
        }
        return Err(error.into());
    }
    Ok(())
}
