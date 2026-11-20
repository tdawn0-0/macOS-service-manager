//! Finished database backups, ported from `backups.go`.
//!
//! A backup is one `<instance>-<database>-<16 hex>.dump` regular file in the
//! workspace `backups/` directory. Listing only trusts that naming convention
//! and refuses names that another instance's longer ID also prefixes. Deleting
//! takes the instance lease first, so it cannot race a running backup.

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use regex::Regex;
use std::sync::LazyLock;

use crate::fs::sync_directory;
use crate::state::ResourceKey;

use super::{Cancelled, DatabaseError, Databases};

static BACKUP_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^.+-[0-9a-f]{16}\.dump$").unwrap());

/// One finished backup in the app's backups folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupFile {
    pub name: String,
    pub path: PathBuf,
    pub database: String,
    pub size: i64,
    pub modified: SystemTime,
}

impl Databases {
    /// List the finished backups of an instance, newest first.
    pub fn backups(
        &self,
        id: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<Vec<BackupFile>, DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        let state = self.store().read()?;
        if !state.instances.contains_key(id) {
            return Err(DatabaseError::invalid("database instance not found"));
        }
        let dir = self.store().root().join("backups");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut out = Vec::new();
        for entry in entries {
            if cancelled() {
                return Err(DatabaseError::Cancelled);
            }
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !BACKUP_NAME_RE.is_match(&name) || !name.starts_with(&format!("{id}-")) {
                continue;
            }
            let trimmed = name.trim_start_matches(&format!("{id}-"));
            if trimmed.len() <= 22 {
                continue;
            }
            let database = trimmed[..trimmed.len() - 22].to_string();
            if database.is_empty() {
                continue;
            }
            let longer = state.instances.keys().any(|other| {
                other != id
                    && other.starts_with(&format!("{id}-"))
                    && name.starts_with(&format!("{other}-"))
            });
            if longer {
                continue;
            }
            let Ok(info) = entry.metadata() else {
                continue;
            };
            if !info.is_file() {
                continue;
            }
            out.push(BackupFile {
                name,
                path: dir.join(entry.file_name()),
                database: database.clone(),
                size: info.len() as i64,
                modified: info.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
        out.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(out)
    }

    /// Permanently remove one finished backup of an instance.
    pub fn delete_backup(
        &self,
        id: &str,
        name: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        if name.is_empty()
            || name
                != PathBuf::from(name)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            || name.contains(['/', '\0'])
        {
            return Err(DatabaseError::invalid("invalid backup name"));
        }
        let _lease = self
            .store()
            .resource_leases_cancellable(&[ResourceKey::new("database instance", id)], cancelled)?;
        let list = self.backups(id, cancelled)?;
        for backup in list {
            if backup.name != name {
                continue;
            }
            let info = fs::symlink_metadata(&backup.path)?;
            if !info.is_file() {
                return Err(DatabaseError::invalid("backup is not a regular file"));
            }
            fs::remove_file(&backup.path)?;
            sync_directory(
                backup
                    .path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(".")),
            )?;
            return Ok(());
        }
        Err(DatabaseError::invalid("backup not found for this instance"))
    }
}
