//! Go `path/filepath` semantics the helper validation relies on.
//!
//! The Go server compares `filepath.Clean(path)` against the request path and
//! matches parent directories with `filepath.Dir`, so the port needs the exact
//! same answers (`//x` cleans to `/x`, `a/` to `a`, and so on).

use std::path::{Path, PathBuf};

use super::admin::shell_quote;

/// `filepath.Clean` on Unix.
pub fn clean_path(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => match parts.last() {
                Some(last) if *last != ".." => {
                    parts.pop();
                }
                None if !absolute => parts.push(".."),
                Some(_) if !absolute => parts.push(".."),
                _ => {}
            },
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// `filepath.Dir` on Unix.
pub fn dir_path(path: &str) -> String {
    match path.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(index) => clean_path(&path[..index]),
    }
}

/// `filepath.Base` on Unix. Unlike `clean_path` it keeps `.` and `..`
/// elements, because Go's `Base` only strips trailing separators first.
pub fn base_name(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let mut end = path.len();
    while end > 0 && path.as_bytes()[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return "/".to_string();
    }
    let trimmed = &path[..end];
    match trimmed.rfind('/') {
        Some(index) => trimmed[index + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

/// Whether every byte is an ASCII digit; the Go `isDigits` helper.
pub fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Rejects an existing symlink at `path`, succeeding when it is absent.
pub fn reject_symlink_if_exists(path: &str) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!("refusing symlink {path:?}"));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(lstat_error(path, &error)),
    }
}

/// `lstat <path>: <reason>`, the shape Go's `*PathError` prints.
pub fn lstat_error(path: &str, error: &std::io::Error) -> String {
    format!("lstat {path}: {}", io_reason(error))
}

/// The bare reason text of an I/O error, without Rust's `(os error N)` and
/// with Go's lowercase first letter (`no such file or directory`).
pub fn io_reason(error: &std::io::Error) -> String {
    let text = error.to_string();
    let stripped = match error.raw_os_error() {
        Some(code) => text
            .strip_suffix(&format!(" (os error {code})"))
            .unwrap_or(&text),
        None => text.as_str(),
    };
    let mut chars = stripped.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Checks that `path` is an absolute, normalized file directly inside one of
/// `allowed_dirs`, ending in `suffix`, and that neither it nor its parent is a
/// symlink. This is Go's `validateCleanTargetFile`.
pub fn validate_clean_target_file(
    path: &str,
    allowed_dirs: &[String],
    suffix: &str,
) -> Result<String, String> {
    if path.is_empty() || !Path::new(path).is_absolute() {
        return Err(format!("path {path:?} must be absolute"));
    }
    if path.contains('\0') {
        return Err("path contains NUL byte".to_string());
    }
    let clean = clean_path(path);
    if clean != path {
        return Err(format!("path {path:?} is not normalized"));
    }
    if !suffix.is_empty() && !clean.ends_with(suffix) {
        return Err(format!("path {clean:?} must end with {suffix}"));
    }
    let parent = dir_path(&clean);
    if !allowed_dirs.iter().any(|dir| clean_path(dir) == parent) {
        return Err(format!("directory {parent:?} is not permitted"));
    }
    reject_symlink_if_exists(&parent)?;
    reject_symlink_if_exists(&clean)?;
    Ok(clean)
}

/// Converts a validated path string into a [`PathBuf`].
pub fn to_path(path: &str) -> PathBuf {
    PathBuf::from(path)
}

/// Quotes one path as a single shell word, as `system.ShellQuote` does.
pub fn shell_quote_path(path: &Path) -> String {
    shell_quote(&path.to_string_lossy())
}
