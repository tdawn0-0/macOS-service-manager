//! Private executable snapshots under `helpers/`.
//!
//! launchd jobs must not point into a movable `.app` bundle, so `serve` and
//! `tunnel` run copies with mode 0700. Ported from the helper half of
//! `network.go`.

use std::io;
use std::path::{Path, PathBuf};

use devcore::fs::{
    PRIVATE_DIR, atomic_write, create_private_dir_all, digest_bytes, sync_directory,
};
use sha2::{Digest, Sha256};

/// The helper size ceiling from the Go reader (128 MiB plus one byte).
const MAX_HELPER_SIZE: u64 = 128 << 20;

/// Copy `source` to `<root>/helpers/<name>` unless the same bytes are already
/// there. Returns the stable helper path.
pub fn helper_executable(root: &Path, name: &str, source: &Path) -> io::Result<PathBuf> {
    if !source.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "application executable must be absolute",
        ));
    }
    let directory = root.join("helpers");
    create_private_dir_all(&directory)?;
    let destination = directory.join(name);
    if source == destination {
        return Ok(destination);
    }
    let metadata = std::fs::metadata(source)?;
    if metadata.len() > MAX_HELPER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "helper executable exceeds size limit",
        ));
    }
    let bytes = std::fs::read(source)?;
    if bytes.len() as u64 > MAX_HELPER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "helper executable exceeds size limit",
        ));
    }
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if existing_digest(&destination).as_deref() == Some(digest.as_slice()) {
        return Ok(destination);
    }
    atomic_write(&destination, &bytes, PRIVATE_DIR)?;
    sync_directory(&directory)?;
    Ok(destination)
}

fn existing_digest(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() as u64 > MAX_HELPER_SIZE {
        return None;
    }
    Some(Sha256::digest(&bytes).to_vec())
}

/// The digest text used by tests and diagnostics.
pub fn helper_digest(bytes: &[u8]) -> String {
    digest_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn helper_is_copied_privately_and_reused() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source");
        std::fs::write(&source, b"binary payload").unwrap();
        let root = dir.path().join("root");
        let first = helper_executable(&root, "proxy", &source).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"binary payload");
        assert_eq!(
            std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let modified = std::fs::metadata(&first).unwrap().modified().unwrap();
        let second = helper_executable(&root, "proxy", &source).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            std::fs::metadata(&second).unwrap().modified().unwrap(),
            modified,
            "identical bytes are not rewritten"
        );
        std::fs::write(&source, b"changed").unwrap();
        helper_executable(&root, "proxy", &source).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"changed");
    }

    #[test]
    fn relative_sources_are_rejected() {
        let dir = TempDir::new().unwrap();
        assert!(helper_executable(dir.path(), "proxy", Path::new("relative")).is_err());
    }
}
