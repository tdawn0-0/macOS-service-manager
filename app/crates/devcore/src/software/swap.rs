//! Atomic publication primitives, ported from the Go
//! `package_swap_darwin.go:12-30`.
//!
//! `renamex_np` is the only way package directories are replaced. `RENAME_SWAP`
//! keeps the installation path present for the whole exchange; `RENAME_EXCL`
//! refuses a destination created after the caller's existence check. Both
//! directories are staged on the same volume. There is deliberately no
//! non-atomic fallback.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::SoftwareError;

const RENAME_SWAP: libc::c_uint = 2;
const RENAME_EXCL: libc::c_uint = 4;

/// Exchange two existing directories atomically.
pub fn exchange_package_directories(a: &Path, b: &Path) -> Result<(), SoftwareError> {
    rename(a, b, RENAME_SWAP, || {
        "filesystem could not atomically exchange the package directories; the original installation was kept"
            .to_string()
    })
}

/// Publish a new directory without replacing anything already at `b`.
pub fn publish_new_package_directory(a: &Path, b: &Path) -> Result<(), SoftwareError> {
    rename(a, b, RENAME_EXCL, || {
        format!(
            "installation destination already exists or cannot be published exclusively; existing files were kept: {}",
            b.display()
        )
    })
}

fn rename(
    a: &Path,
    b: &Path,
    flags: libc::c_uint,
    failure: impl Fn() -> String,
) -> Result<(), SoftwareError> {
    let from = CString::new(a.as_os_str().as_bytes())
        .map_err(|_| SoftwareError::invalid("package path contains a NUL byte"))?;
    let to = CString::new(b.as_os_str().as_bytes())
        .map_err(|_| SoftwareError::invalid("package path contains a NUL byte"))?;
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), flags) };
    if result == 0 {
        Ok(())
    } else {
        Err(SoftwareError::invalid(failure()))
    }
}
