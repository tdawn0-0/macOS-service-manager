//! Fast, non-destructive runtime telemetry for a database instance:
//! data directory logical disk usage, latest backup summary, and the last
//! three non-empty lines of `stderr` when the instance is `Failed`.

use std::fs;
use std::path::Path;

use crate::diagnostics::tail;

use super::backups::format_utc_minute;
use super::{DatabaseError, Databases};

/// Entry cap so telemetry on a large external directory remains bounded.
const MAX_TELEMETRY_ENTRIES: usize = 20_000;
/// Recursion depth cap for directory traversal.
const MAX_TELEMETRY_DEPTH: usize = 16;
/// Bounded read cap when inspecting `db-<id>.err.log`.
const STDERR_TAIL_BYTES: i64 = 16 << 10;
const STDERR_PREVIEW_LINES: usize = 3;

/// Non-destructive telemetry snapshot for one managed database instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceTelemetry {
    /// Total logical size in bytes of regular files in the instance's data directory.
    pub data_size_bytes: Option<u64>,
    /// Latest finished backup `(summary, size_bytes)` if one exists.
    pub last_backup: Option<(String, u64)>,
    /// Last up to 3 non-empty lines from `logs/db-<id>.err.log` when status is `Failed`.
    pub last_stderr: Vec<String>,
}

impl Databases {
    /// Collect non-destructive telemetry for `id` given its observed `status`.
    pub fn instance_telemetry(
        &self,
        id: &str,
        status: &str,
    ) -> Result<InstanceTelemetry, DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        let instance = self.instance(id)?;
        let data_dir = self.data_directory(&instance);
        let data_size_bytes = directory_logical_bytes(&data_dir);

        let last_backup = self
            .backups(id, &|| false)
            .ok()
            .and_then(|rows| rows.into_iter().next())
            .map(|latest| {
                let summary = format!(
                    "{} · {}",
                    latest.database,
                    format_utc_minute(latest.modified)
                );
                (summary, latest.size.max(0) as u64)
            });

        let last_stderr = if status == "Failed" {
            let err_log = self
                .store()
                .root()
                .join("logs")
                .join(format!("db-{id}.err.log"));
            read_last_stderr_lines(&err_log, STDERR_PREVIEW_LINES)
        } else {
            Vec::new()
        };

        Ok(InstanceTelemetry {
            data_size_bytes,
            last_backup,
            last_stderr,
        })
    }
}

fn directory_logical_bytes(root: &Path) -> Option<u64> {
    let meta = fs::symlink_metadata(root).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    if meta.is_file() {
        return Some(meta.len());
    }
    if !meta.is_dir() {
        return None;
    }
    let mut total: u64 = 0;
    let mut visited: usize = 0;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_TELEMETRY_DEPTH || visited >= MAX_TELEMETRY_ENTRIES {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_TELEMETRY_ENTRIES {
                break;
            }
            let Ok(info) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if info.file_type().is_symlink() {
                continue;
            }
            if info.is_file() {
                total = total.saturating_add(info.len());
            } else if info.is_dir() && depth < MAX_TELEMETRY_DEPTH {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    Some(total)
}

fn read_last_stderr_lines(path: &Path, limit: usize) -> Vec<String> {
    let Ok(snapshot) = tail(path, STDERR_TAIL_BYTES, 32) else {
        return Vec::new();
    };
    let mut non_empty: Vec<String> = snapshot
        .lines
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    if non_empty.len() > limit {
        non_empty = non_empty.split_off(non_empty.len() - limit);
    }
    non_empty
}
