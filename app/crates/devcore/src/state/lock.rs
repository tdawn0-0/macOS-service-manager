//! `flock(2)` wrappers. Locks are always real file locks: the GUI and the CLI
//! are separate processes and an in-process mutex would not exclude them.

use std::fs::File;
use std::io;
use std::os::unix::io::AsRawFd;

/// A held flock that is released (and the descriptor closed) on drop.
#[derive(Debug)]
pub(crate) struct FileLease {
    file: File,
}

impl FileLease {
    pub(crate) fn new(file: File) -> Self {
        FileLease { file }
    }
}

impl Drop for FileLease {
    fn drop(&mut self) {
        // Closing the descriptor would release the lock anyway; unlocking
        // first keeps the intent explicit and matches the Go defer.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Try to take a lock without blocking.
///
/// `Ok(true)` means the lock was acquired, `Ok(false)` means another open
/// file description holds a conflicting lock (`EWOULDBLOCK`).
pub(crate) fn try_flock(file: &File, operation: libc::c_int) -> io::Result<bool> {
    let result = unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(false),
        _ => Err(error),
    }
}
