//! Atomic publication primitives, ported from the Go
//! `package_swap_darwin.go:12-30` and extended for Linux via `renameat2(2)`.
//!
//! Atomic directory swap/publish is the only way package directories are
//! replaced. On macOS this uses `renamex_np(RENAME_SWAP | RENAME_EXCL)`; on
//! Linux it uses `SYS_renameat2(RENAME_EXCHANGE | RENAME_NOREPLACE)`. Both
//! directories are staged on the same volume. There is deliberately no
//! non-atomic fallback.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::SoftwareError;

#[cfg(target_os = "macos")]
const SWAP_FLAG: libc::c_uint = 2; // RENAME_SWAP
#[cfg(target_os = "macos")]
const EXCL_FLAG: libc::c_uint = 4; // RENAME_EXCL

#[cfg(target_os = "linux")]
const SWAP_FLAG: libc::c_uint = 2; // RENAME_EXCHANGE
#[cfg(target_os = "linux")]
const EXCL_FLAG: libc::c_uint = 1; // RENAME_NOREPLACE

/// Exchange two existing directories atomically.
pub fn exchange_package_directories(a: &Path, b: &Path) -> Result<(), SoftwareError> {
    rename(a, b, SWAP_FLAG, || {
        "filesystem could not atomically exchange the package directories; the original installation was kept"
            .to_string()
    })
}

/// Publish a new directory without replacing anything already at `b`.
pub fn publish_new_package_directory(a: &Path, b: &Path) -> Result<(), SoftwareError> {
    rename(a, b, EXCL_FLAG, || {
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
    let result = rename_with_flags(&from, &to, flags);
    if result == 0 {
        Ok(())
    } else {
        Err(SoftwareError::invalid(failure()))
    }
}

#[cfg(target_os = "macos")]
fn rename_with_flags(from: &CString, to: &CString, flags: libc::c_uint) -> libc::c_int {
    unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), flags) }
}

#[cfg(target_os = "linux")]
fn rename_with_flags(from: &CString, to: &CString, flags: libc::c_uint) -> libc::c_int {
    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            flags,
        )
    };
    if ret == 0 { 0 } else { -1 }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn publish_new_package_directory_moves_once_and_refuses_overwrite() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("staged");
        let dst = dir.path().join("published");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("marker"), b"v1").unwrap();

        publish_new_package_directory(&src, &dst).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read(dst.join("marker")).unwrap(), b"v1");

        // Publishing onto an existing destination must fail without touching dst.
        let src2 = dir.path().join("staged2");
        fs::create_dir(&src2).unwrap();
        fs::write(src2.join("marker"), b"v2").unwrap();
        assert!(publish_new_package_directory(&src2, &dst).is_err());
        assert_eq!(fs::read(dst.join("marker")).unwrap(), b"v1");
        assert_eq!(fs::read(src2.join("marker")).unwrap(), b"v2");
    }

    #[test]
    fn exchange_package_directories_swaps_atomically_and_requires_both() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("ver"), b"new").unwrap();
        fs::write(b.join("ver"), b"old").unwrap();

        exchange_package_directories(&a, &b).unwrap();
        assert_eq!(fs::read(a.join("ver")).unwrap(), b"old");
        assert_eq!(fs::read(b.join("ver")).unwrap(), b"new");

        let missing = dir.path().join("missing");
        assert!(exchange_package_directories(&a, &missing).is_err());
        assert_eq!(fs::read(a.join("ver")).unwrap(), b"old");
    }
}
