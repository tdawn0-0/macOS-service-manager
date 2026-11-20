//! Small filesystem primitives shared by the persistence modules, ported from
//! the Go `storage.go` helpers.
//!
//! The security-relevant pieces are deliberate: [`read_regular_file`] refuses
//! symlinks and special files (including a FIFO swapped in between `lstat` and
//! `open`), and [`atomic_write`] publishes a complete file with a same-directory
//! rename so readers never observe partial contents.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

/// The directory and file modes the Go code uses for private state.
pub const PRIVATE_DIR: u32 = 0o700;
pub const PRIVATE_FILE: u32 = 0o600;

/// Read a small regular file without ever following a symlink.
///
/// `name` is relative to `dir` and is also the name used in error messages,
/// matching the Go helper. The size check happens both on `fstat` before
/// reading and on the number of bytes actually read, so a file that grows
/// during the read still fails.
pub fn read_regular_file(dir: &Path, name: &str, limit: i64) -> io::Result<Vec<u8>> {
    let path = dir.join(name);
    let info = fs::symlink_metadata(&path)?;
    if !info.file_type().is_file() {
        return Err(invalid_input(format!("{name} must be a regular file")));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.len() > limit as u64 {
        return Err(invalid_input(format!(
            "{name} is not a regular file within its size limit"
        )));
    }
    let mut bytes = Vec::new();
    // Read one byte past the limit so growth during the read is detected.
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() as i64 > limit {
        return Err(invalid_input(format!("{name} exceeds its size limit")));
    }
    Ok(bytes)
}

/// fsync a directory so a rename or unlink inside it is durable.
pub fn sync_directory(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Hex-encoded SHA-256 of `bytes`.
pub fn digest_bytes(bytes: &[u8]) -> String {
    encode_hex(&Sha256::digest(bytes))
}

/// Whether `value` is a hex string that decodes to exactly 32 bytes.
pub fn valid_digest(value: &str) -> bool {
    decode_hex(value).is_some_and(|bytes| bytes.len() == 32)
}

/// Lowercase hex encoding.
pub fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Hex decoding, accepting either case, like `encoding/hex.DecodeString`.
pub fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        out.push(high << 4 | low);
    }
    Some(out)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Lexically clean a path the way `filepath.Clean` does.
pub fn clean_path(path: &Path) -> PathBuf {
    let rooted = path.is_absolute();
    let mut parts: Vec<OsString> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => parts.push(prefix.as_os_str().to_owned()),
            Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(last) if last.as_os_str() != std::ffi::OsStr::new("..") => {
                    parts.pop();
                }
                Some(_) => {
                    if !rooted {
                        parts.push(OsString::from(".."));
                    }
                }
                None => {
                    if !rooted {
                        parts.push(OsString::from(".."));
                    }
                }
            },
            Component::Normal(part) => parts.push(part.to_owned()),
        }
    }
    if parts.is_empty() {
        return if rooted {
            PathBuf::from("/")
        } else {
            PathBuf::from(".")
        };
    }
    let mut out = if rooted {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    for part in parts {
        out.push(part);
    }
    out
}

/// A non-empty relative path that does not escape its root.
pub fn safe_relative(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') {
        return false;
    }
    let cleaned = clean_path(Path::new(path));
    cleaned != Path::new("..") && !cleaned.starts_with("..")
}

/// Whether `path` is `root` or lies inside it, compared lexically after
/// cleaning. Callers that must resolve symlinks have already done so.
pub fn inside(root: &Path, path: &Path) -> bool {
    let root = clean_path(root);
    let path = clean_path(path);
    path == root || path.starts_with(&root)
}

/// Write `bytes` to `path` atomically: a private temp file in the same
/// directory, fsync, then rename over the destination.
pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    create_private_dir_all(parent)?;
    let mut attempt = 0u32;
    loop {
        let candidate = parent.join(temp_name(attempt));
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW);
        match options.open(&candidate) {
            Ok(mut file) => {
                // `mode` is filtered by the process umask on creation; the Go
                // code chmods to the exact mode, so do the same.
                if let Err(error) = file.set_permissions(fs::Permissions::from_mode(mode)) {
                    cleanup(&candidate);
                    return Err(error);
                }
                let result = file.write_all(bytes).and_then(|()| file.sync_all());
                drop(file);
                if let Err(error) = result {
                    cleanup(&candidate);
                    return Err(error);
                }
                if let Err(error) = fs::rename(&candidate, path) {
                    cleanup(&candidate);
                    return Err(error);
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn cleanup(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Create a directory (and parents) with the private mode.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(PRIVATE_DIR)
        .create(path)
}

fn temp_name(attempt: u32) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    format!(
        ".write-{}-{nanos:x}-{counter}-{attempt}",
        std::process::id()
    )
}

fn invalid_input(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_path_matches_filepath_clean() {
        assert_eq!(clean_path(Path::new("")), PathBuf::from("."));
        assert_eq!(clean_path(Path::new("a//b/./c")), PathBuf::from("a/b/c"));
        assert_eq!(clean_path(Path::new("/a/../b")), PathBuf::from("/b"));
        assert_eq!(clean_path(Path::new("a/../../b")), PathBuf::from("../b"));
        assert_eq!(clean_path(Path::new("../x")), PathBuf::from("../x"));
        assert_eq!(clean_path(Path::new("/..")), PathBuf::from("/"));
        assert_eq!(clean_path(Path::new("a/b/..")), PathBuf::from("a"));
    }

    #[test]
    fn safe_relative_rejects_escapes() {
        assert!(safe_relative("bin/java"));
        assert!(safe_relative("a/../b"));
        assert!(!safe_relative(""));
        assert!(!safe_relative("/bin/java"));
        assert!(!safe_relative(".."));
        assert!(!safe_relative("../x"));
        assert!(!safe_relative("a/../../x"));
    }

    #[test]
    fn digest_helpers_round_trip() {
        let digest = digest_bytes(b"hello");
        assert_eq!(
            digest,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert!(valid_digest(&digest));
        assert!(valid_digest(&digest.to_uppercase()));
        assert!(!valid_digest("abc"));
        assert!(!valid_digest(&"0".repeat(63)));
        assert!(!valid_digest(&"z".repeat(64)));
    }

    #[test]
    fn inside_is_lexical() {
        assert!(inside(Path::new("/a/b"), Path::new("/a/b")));
        assert!(inside(Path::new("/a/b"), Path::new("/a/b/c")));
        assert!(!inside(Path::new("/a/b"), Path::new("/a/bc")));
        assert!(inside(Path::new("/a/../a"), Path::new("/a/b")));
    }
}
