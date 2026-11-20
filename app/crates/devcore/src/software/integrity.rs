//! Installation tree indexing and verification, ported from the Go
//! `package_integrity.go:54-267`.
//!
//! The index records kinds, modes, sizes, SHA-256s and symlink targets, never
//! timestamps or xattrs. Its own digest is anchored in `state.json` outside the
//! package, so editing the index cannot make a damaged tree verify.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    Cancelled, MAX_EXPANDED, MAX_INTEGRITY_INDEX, Software, SoftwareError, canonical_inside,
    integrity_path, utc_now_rfc3339,
};
use crate::fs::{PRIVATE_FILE, read_regular_file};
use crate::model::{Package, State};

/// One filesystem entry in an installation index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageFile {
    pub kind: String,
    pub mode: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bytes: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// The schema-1 installation index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageTree {
    pub schema: i64,
    #[serde(rename = "packageID")]
    pub package_id: String,
    #[serde(rename = "archiveSHA256")]
    pub archive_sha256: String,
    pub files: BTreeMap<String, PackageFile>,
}

/// The outcome of checking an installed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageIntegrityResult {
    /// `Verified`, `Needs repair`, `Not recorded` or `Homebrew managed`.
    pub status: String,
    pub checked_at: String,
    pub files: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub baseline: String,
}

impl PackageIntegrityResult {
    pub fn text(&self) -> String {
        let mut text = format!("{} · {} entries checked", self.status, self.files);
        for issue in &self.issues {
            text.push('\n');
            text.push_str(issue);
        }
        text
    }
}

impl Software {
    /// Verify a package under a shared lease.
    pub fn verify_package(
        &self,
        id: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<PackageIntegrityResult, SoftwareError> {
        let _lease = self
            .store()
            .package_lease_cancellable(id, false, cancelled)?;
        let state = self.store().read()?;
        let package = state
            .packages
            .get(id)
            .ok_or_else(|| SoftwareError::invalid("package is not installed"))?
            .clone();
        if package.id() != id {
            return Err(SoftwareError::invalid(
                "installed package identity mismatch",
            ));
        }
        self.verify_package_in_state(cancelled, &state, &package)
    }

    /// Verify against an already-read state (the install path uses this).
    pub fn verify_package_in_state(
        &self,
        cancelled: Cancelled<'_>,
        state: &State,
        package: &Package,
    ) -> Result<PackageIntegrityResult, SoftwareError> {
        let mut result = PackageIntegrityResult {
            status: "Needs repair".into(),
            checked_at: utc_now_rfc3339(),
            files: 0,
            issues: Vec::new(),
            baseline: state
                .package_integrity
                .get(&package.id())
                .cloned()
                .unwrap_or_default(),
        };
        if cancelled() {
            return Err(cancelled_error());
        }
        package.validate()?;
        let mut names: Vec<&String> = package.binaries.keys().collect();
        names.sort();
        for name in names {
            if self.binary(state, &package.id(), name).is_err() {
                result
                    .issues
                    .push(format!("Required executable is missing or invalid: {name}"));
            }
            if !package.formula.is_empty() {
                result.files += 1;
            }
        }
        if !package.formula.is_empty() {
            if result.issues.is_empty() {
                result.status = "Homebrew managed".into();
            }
            return Ok(result);
        }
        if result.baseline.is_empty() {
            if result.issues.is_empty() {
                result.status = "Not recorded".into();
            }
            result.issues.push(
                "No trusted installation index is recorded. Repair from the verified archive to establish one."
                    .into(),
            );
            return Ok(result);
        }
        let path = integrity_path(self.store(), &package.id());
        let expected_bytes = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                match read_regular_file(
                    path.parent().unwrap_or_else(|| Path::new(".")),
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default(),
                    (MAX_INTEGRITY_INDEX as i64) + 1,
                ) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        result
                            .issues
                            .push("Installation integrity index could not be read.".into());
                        return Ok(result);
                    }
                }
            }
            _ => {
                result.issues.push(
                    "Installation integrity index is missing or is not a regular file.".into(),
                );
                return Ok(result);
            }
        };
        if cancelled() {
            return Err(cancelled_error());
        }
        let digest = crate::fs::digest_bytes(&expected_bytes);
        if expected_bytes.len() > MAX_INTEGRITY_INDEX || digest != result.baseline {
            result
                .issues
                .push("Installation integrity index is damaged or has changed.".into());
            return Ok(result);
        }
        let expected: PackageTree = match serde_json::from_slice(&expected_bytes) {
            Ok(tree) => tree,
            Err(_) => {
                result
                    .issues
                    .push("Installation integrity index does not match this package.".into());
                return Ok(result);
            }
        };
        if expected.schema != 1
            || expected.package_id != package.id()
            || expected.archive_sha256 != package.sha256
        {
            result
                .issues
                .push("Installation integrity index does not match this package.".into());
            return Ok(result);
        }
        self.store()
            .emit(&format!("Checking installed files for {}", package.id()));
        let actual = scan_package_tree(
            self,
            cancelled,
            &self.store().package_directory(package),
            package,
        );
        let actual = match actual {
            Ok(tree) => tree,
            Err(error) => {
                result.issues.push(error.to_string());
                return Ok(result);
            }
        };
        result.files = actual.files.len() as i64;
        if cancelled() {
            return Err(cancelled_error());
        }
        let mut changes = Vec::new();
        for (path, want) in &expected.files {
            match actual.files.get(path) {
                None => changes.push(format!("Missing: {path}")),
                Some(got) if got != want => changes.push(format!("Changed: {path}")),
                Some(_) => {}
            }
        }
        for path in actual.files.keys() {
            if !expected.files.contains_key(path) {
                changes.push(format!("Unexpected: {path}"));
            }
        }
        changes.sort();
        if changes.len() > 50 {
            let more = changes.len() - 50;
            changes.truncate(50);
            changes.push(format!("{more} more differences"));
        }
        result.issues.extend(changes);
        if result.issues.is_empty() {
            result.status = "Verified".into();
        }
        Ok(result)
    }
}

/// Scan an installation tree into a schema-1 index.
pub fn scan_package_tree(
    software: &Software,
    cancelled: Cancelled<'_>,
    root: &Path,
    package: &Package,
) -> Result<PackageTree, SoftwareError> {
    let mut tree = PackageTree {
        schema: 1,
        package_id: package.id(),
        archive_sha256: package.sha256.clone(),
        files: BTreeMap::new(),
    };
    let info = std::fs::symlink_metadata(root)?;
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(SoftwareError::invalid(
            "installation root must be a directory, not a symlink",
        ));
    }
    tree.files.insert(
        ".".into(),
        PackageFile {
            kind: "directory".into(),
            mode: integrity_mode(&info),
            ..PackageFile::default()
        },
    );
    let mut total = 0i64;
    let mut last = std::time::Instant::now();
    walk(
        software, cancelled, root, root, &mut tree, &mut total, &mut last, package,
    )?;
    Ok(tree)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    software: &Software,
    cancelled: Cancelled<'_>,
    root: &Path,
    directory: &Path,
    tree: &mut PackageTree,
    total: &mut i64,
    last: &mut std::time::Instant,
    package: &Package,
) -> Result<(), SoftwareError> {
    if cancelled() {
        return Err(cancelled_error());
    }
    let mut entries: Vec<_> = std::fs::read_dir(directory)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if cancelled() {
            return Err(cancelled_error());
        }
        let path = entry.path();
        if tree.files.len() >= 100_000 {
            return Err(SoftwareError::invalid(
                "package contains too many filesystem entries",
            ));
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| SoftwareError::invalid("package path is outside the installation"))?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        if !crate::fs::safe_relative(&rel) {
            return Err(SoftwareError::invalid(
                "package path is outside the installation",
            ));
        }
        let info = std::fs::symlink_metadata(&path)?;
        let mut record = PackageFile {
            mode: integrity_mode(&info),
            ..PackageFile::default()
        };
        if info.is_dir() {
            record.kind = "directory".into();
        } else if info.file_type().is_symlink() {
            record.kind = "symlink".into();
            record.target = std::fs::read_link(&path)?
                .to_str()
                .ok_or_else(|| SoftwareError::invalid("symlink target is not UTF-8"))?
                .to_string();
            if Path::new(&record.target).is_absolute()
                || !canonical_inside(
                    root,
                    &path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(&record.target),
                )
            {
                return Err(SoftwareError::invalid(format!(
                    "symlink escapes installation: {rel}"
                )));
            }
        } else if info.is_file() {
            record.kind = "file".into();
            record.bytes = info.size() as i64;
            *total = (*total).saturating_add(record.bytes);
            if *total > MAX_EXPANDED {
                return Err(SoftwareError::invalid(
                    "installed files exceed the supported size",
                ));
            }
            if !crate::fs::inside(root, &path) {
                return Err(SoftwareError::invalid("file escapes installation"));
            }
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)?;
            let before = file.metadata()?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0u8; 64 * 1024];
            let mut read_total: u64 = 0;
            loop {
                if cancelled() {
                    return Err(cancelled_error());
                }
                use std::io::Read;
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                read_total += read as u64;
                if read_total > (MAX_EXPANDED as u64) + 1 {
                    return Err(SoftwareError::invalid(
                        "installed files exceed the supported size",
                    ));
                }
                hasher.update(&buffer[..read]);
            }
            let after = file.metadata()?;
            if read_total != info.size()
                || before.dev() != after.dev()
                || before.ino() != after.ino()
                || before.len() != after.len()
                || before.mode() != after.mode()
                || before.mtime() != after.mtime()
                || before.mtime_nsec() != after.mtime_nsec()
            {
                return Err(SoftwareError::invalid(format!(
                    "file changed while checking: {rel}"
                )));
            }
            record.sha256 = crate::fs::encode_hex(&hasher.finalize());
        } else {
            return Err(SoftwareError::invalid(format!(
                "unsupported filesystem entry: {rel}"
            )));
        }
        tree.files.insert(rel, record);
        if last.elapsed() > std::time::Duration::from_millis(500) {
            software.store().emit(&format!(
                "Checking {} · {} entries",
                package.id(),
                tree.files.len()
            ));
            *last = std::time::Instant::now();
        }
        if info.is_dir() && !info.file_type().is_symlink() {
            walk(software, cancelled, root, &path, tree, total, last, package)?;
        }
    }
    Ok(())
}

fn integrity_mode(metadata: &std::fs::Metadata) -> u32 {
    metadata.mode() & 0o7777
}

/// Encode an index exactly as the Go `encodePackageTree` does (two-space
/// indentation + trailing newline) and return its bytes and SHA-256.
pub fn encode_package_tree(tree: &PackageTree) -> Result<(Vec<u8>, String), SoftwareError> {
    let mut bytes = serde_json::to_vec_pretty(tree)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_INTEGRITY_INDEX {
        return Err(SoftwareError::invalid(
            "package integrity index exceeds limit",
        ));
    }
    let digest = crate::fs::digest_bytes(&bytes);
    Ok((bytes, digest))
}

/// Write an installation index with mode 0600.
pub fn write_index(path: &Path, bytes: &[u8]) -> Result<(), SoftwareError> {
    crate::fs::atomic_write(path, bytes, PRIVATE_FILE)?;
    Ok(())
}

fn cancelled_error() -> SoftwareError {
    SoftwareError::State(crate::state::StateError::Cancelled)
}

/// The integrity index path for a package (UI/report helper).
pub fn package_index_path(software: &Software, id: &str) -> PathBuf {
    integrity_path(software.store(), id)
}
