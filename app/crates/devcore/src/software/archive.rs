//! Safe archive extraction, ported from the Go `extractContext` in
//! `packages.go:457-618`.
//!
//! Rejections are the security contract: backslashes, absolute paths, `..`
//! traversal, symlinks that are absolute or resolve outside the root, special
//! entries, and expanded size above 8 GiB. Symlinks are created last, so no
//! earlier entry can write through one, and every link is re-resolved after
//! creation to reject a chain that escapes.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;

use super::{MAX_EXPANDED, SoftwareError, canonical_inside};
use crate::fs::{clean_path, safe_relative};

/// Extract `src` into `root`, applying `strip` leading path components.
pub fn extract(
    src: &Path,
    root: &Path,
    format: &str,
    strip: i64,
    cancelled: super::Cancelled<'_>,
) -> Result<(), SoftwareError> {
    if cancelled() {
        return Err(cancelled_error());
    }
    let mut writer = Extractor {
        root: root.to_path_buf(),
        strip: strip.max(0) as usize,
        total: 0,
        links: Vec::new(),
        cancelled,
    };
    match format {
        "zip" => writer.extract_zip(src)?,
        _ => writer.extract_tar_gz(src)?,
    }
    for (path, target) in &writer.links {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::os::unix::fs::symlink(target, path)?;
    }
    for (path, _) in &writer.links {
        let Ok(real) = std::fs::canonicalize(path) else {
            return Err(SoftwareError::invalid("invalid archive symlink chain"));
        };
        if !canonical_inside(root, &real) {
            return Err(SoftwareError::invalid("invalid archive symlink chain"));
        }
    }
    Ok(())
}

fn cancelled_error() -> SoftwareError {
    SoftwareError::State(crate::state::StateError::Cancelled)
}

struct Extractor<'a> {
    root: PathBuf,
    strip: usize,
    total: i64,
    links: Vec<(PathBuf, String)>,
    cancelled: super::Cancelled<'a>,
}

impl Extractor<'_> {
    fn extract_tar_gz(&mut self, src: &Path) -> Result<(), SoftwareError> {
        let file = File::open(src)?;
        let decoder = GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let kind = entry.header().entry_type();
            let size = entry.size();
            let mode = entry.header().mode().unwrap_or(0o644);
            let path = entry
                .path()
                .map_err(|_| SoftwareError::invalid("archive entry path is not valid UTF-8"))?
                .into_owned();
            let name = path
                .to_str()
                .ok_or_else(|| SoftwareError::invalid("archive entry path is not valid UTF-8"))?;
            if kind.is_file() {
                self.write_regular(name, mode, size, &mut entry)?;
            } else if kind.is_dir() {
                self.write_directory(name)?;
            } else if kind.is_symlink() {
                let target = entry
                    .link_name()?
                    .ok_or_else(|| SoftwareError::invalid("unsafe archive symlink"))?
                    .into_owned();
                let target = target
                    .to_str()
                    .ok_or_else(|| SoftwareError::invalid("unsafe archive symlink"))?;
                self.write_symlink(name, target)?;
            } else {
                return Err(SoftwareError::invalid(format!(
                    "unsupported tar entry {}",
                    name
                )));
            }
        }
        Ok(())
    }

    fn extract_zip(&mut self, src: &Path) -> Result<(), SoftwareError> {
        let file = File::open(src)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|error| SoftwareError::invalid(format!("read zip archive: {error}")))?;
        for index in 0..archive.len() {
            if (self.cancelled)() {
                return Err(cancelled_error());
            }
            let mut entry = archive
                .by_index(index)
                .map_err(|error| SoftwareError::invalid(format!("read zip entry: {error}")))?;
            let name = entry
                .name()
                .map_err(|error| SoftwareError::invalid(format!("read zip entry: {error}")))?
                .into_owned();
            let mode = entry.unix_mode().unwrap_or(0o644);
            if entry.is_dir() {
                self.write_directory(&name)?;
            } else if entry.is_symlink() {
                let mut target = Vec::new();
                (&mut entry)
                    .take(4097)
                    .read_to_end(&mut target)
                    .map_err(SoftwareError::Io)?;
                if target.len() > 4096 {
                    return Err(SoftwareError::invalid("invalid symlink"));
                }
                let target = String::from_utf8(target)
                    .map_err(|_| SoftwareError::invalid("invalid symlink"))?;
                self.write_symlink(&name, &target)?;
            } else if entry.is_file() {
                self.write_regular(&name, mode, entry.size(), &mut entry)?;
            } else {
                return Err(SoftwareError::invalid("unsupported archive entry"));
            }
        }
        Ok(())
    }

    fn write_directory(&mut self, name: &str) -> Result<(), SoftwareError> {
        if (self.cancelled)() {
            return Err(cancelled_error());
        }
        let Some(path) = self.archive_path(name)? else {
            return Ok(());
        };
        std::fs::create_dir_all(path)?;
        Ok(())
    }

    fn write_regular(
        &mut self,
        name: &str,
        mode: u32,
        size: u64,
        reader: &mut dyn Read,
    ) -> Result<(), SoftwareError> {
        if (self.cancelled)() {
            return Err(cancelled_error());
        }
        let Some(path) = self.archive_path(name)? else {
            return Ok(());
        };
        let size = i64::try_from(size).unwrap_or(i64::MAX);
        self.total = self.total.saturating_add(size);
        if size < 0 || self.total > MAX_EXPANDED {
            return Err(SoftwareError::invalid("expanded archive exceeds limit"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode & 0o777)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        let copied = copy_limited(reader, &mut file, size as u64, self.cancelled)?;
        if copied != size as u64 {
            return Err(SoftwareError::invalid("archive entry size mismatch"));
        }
        Ok(())
    }

    fn write_symlink(&mut self, name: &str, target: &str) -> Result<(), SoftwareError> {
        if (self.cancelled)() {
            return Err(cancelled_error());
        }
        let Some(path) = self.archive_path(name)? else {
            return Ok(());
        };
        if Path::new(target).is_absolute() {
            return Err(SoftwareError::invalid("unsafe archive symlink"));
        }
        let joined = path.parent().unwrap_or_else(|| Path::new(".")).join(target);
        if !canonical_inside(&self.root, &joined) {
            return Err(SoftwareError::invalid("unsafe archive symlink"));
        }
        self.links.push((path, target.to_string()));
        Ok(())
    }

    /// Apply `strip`, reject unsafe names, and prove the joined path is inside
    /// the extraction root. `None` means the entry is removed by stripping.
    fn archive_path(&self, name: &str) -> Result<Option<PathBuf>, SoftwareError> {
        if name.contains('\\') || !safe_relative(name) {
            return Err(SoftwareError::invalid(format!(
                "unsafe archive path {name:?}"
            )));
        }
        let cleaned = clean_path(Path::new(name));
        let parts: Vec<&std::ffi::OsStr> = cleaned
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part),
                _ => None,
            })
            .collect();
        if parts.len() <= self.strip {
            return Ok(None);
        }
        let mut path = self.root.clone();
        for part in &parts[self.strip..] {
            path.push(part);
        }
        if !crate::fs::inside(&self.root, &path) {
            return Err(SoftwareError::invalid("archive path escapes root"));
        }
        Ok(Some(path))
    }
}

fn copy_limited(
    reader: &mut dyn Read,
    writer: &mut dyn io::Write,
    limit: u64,
    cancelled: super::Cancelled<'_>,
) -> Result<u64, SoftwareError> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut copied = 0u64;
    loop {
        if cancelled() {
            return Err(cancelled_error());
        }
        let remaining = limit.saturating_sub(copied);
        let want = buffer.len().min((remaining + 1) as usize);
        let read = reader.read(&mut buffer[..want])?;
        if read == 0 {
            break;
        }
        copied += read as u64;
        if copied > limit {
            break;
        }
        writer.write_all(&buffer[..read])?;
    }
    Ok(copied)
}
