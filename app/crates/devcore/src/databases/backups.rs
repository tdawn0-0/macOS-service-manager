//! Finished database backups, ported from `backups.go`.
//!
//! A backup is one `<instance>-<database>-<16 hex>.dump` regular file in the
//! workspace `backups/` directory. Listing only trusts that naming convention
//! and refuses names that another instance's longer ID also prefixes. Deleting
//! takes the instance lease first, so it cannot race a running backup.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;
use std::sync::LazyLock;

use crate::fs::sync_directory;
use crate::model::BackupSchedule;
use crate::state::ResourceKey;

use super::locations::path_within;
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

    /// Update (or clear) the automatic backup schedule for an instance.
    /// Retention `keep_last` is clamped to at least 1.
    pub fn set_backup_schedule(
        &self,
        id: &str,
        schedule: Option<BackupSchedule>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        let normalized = schedule.map(|mut schedule| {
            schedule.keep_last = schedule.keep_last.max(1);
            if let Some(ref ts) = schedule.last_run_at
                && ts.trim().is_empty()
            {
                schedule.last_run_at = None;
            }
            schedule
        });
        self.store().with_state(|state| {
            let instance = state
                .instances
                .get_mut(id)
                .ok_or_else(|| crate::state::StateError::invalid("database instance not found"))?;
            instance.backup_schedule = normalized;
            Ok(())
        })?;
        Ok(())
    }

    /// Prune finished backups for `id` under `backups/`, keeping the newest
    /// `keep_last` backups (minimum 1). Never touches instance data or any
    /// external data directory. Returns the filenames removed.
    pub fn prune_backups(
        &self,
        id: &str,
        keep_last: u32,
        cancelled: Cancelled<'_>,
    ) -> Result<Vec<String>, DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let keep = usize::try_from(keep_last.max(1)).unwrap_or(1);
        let _lease = self
            .store()
            .resource_leases_cancellable(&[ResourceKey::new("database instance", id)], cancelled)?;
        let backups_dir = self.store().root().join("backups");
        let list = self.backups(id, cancelled)?;
        if list.len() <= keep {
            return Ok(Vec::new());
        }
        let mut removed = Vec::new();
        for backup in list.into_iter().skip(keep) {
            if cancelled() {
                return Err(DatabaseError::Cancelled);
            }
            if !path_within(&backup.path, &backups_dir) {
                return Err(DatabaseError::invalid(
                    "backup path is outside the backups directory",
                ));
            }
            let info = fs::symlink_metadata(&backup.path)?;
            if !info.is_file() {
                return Err(DatabaseError::invalid("backup is not a regular file"));
            }
            fs::remove_file(&backup.path)?;
            removed.push(backup.name);
        }
        if !removed.is_empty() {
            sync_directory(&backups_dir)?;
        }
        Ok(removed)
    }
}

/// Format a [`SystemTime`] as an RFC 3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`).
pub(crate) fn format_rfc3339_utc(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Format a [`SystemTime`] as `YYYY-MM-DD HH:MM` UTC for compact summaries.
pub(crate) fn format_utc_minute(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60
    )
}

/// Parse an RFC 3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`, with optional
/// fractional seconds) into a [`SystemTime`].
pub(crate) fn parse_rfc3339_utc(raw: &str) -> Option<SystemTime> {
    let trimmed = raw.trim();
    let without_z = trimmed.strip_suffix('Z')?;
    let (date, time_raw) = without_z.split_once('T')?;
    let time = time_raw.split('.').next()?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut time_parts = time.split(':');
    let hour: u64 = time_parts.next()?.parse().ok()?;
    let minute: u64 = time_parts.next()?.parse().ok()?;
    let second: u64 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let total_secs =
        days.checked_mul(86_400)? + (hour as i64) * 3600 + (minute as i64) * 60 + (second as i64);
    if total_secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(total_secs as u64))
}

/// Whether a backup schedule is enabled and its cadence window has elapsed at `now`.
pub(crate) fn schedule_is_due(schedule: &BackupSchedule, now: SystemTime) -> bool {
    if !schedule.enabled {
        return false;
    }
    let Some(ref last_raw) = schedule.last_run_at else {
        return true;
    };
    let Some(last) = parse_rfc3339_utc(last_raw) else {
        return true;
    };
    let interval = Duration::from_secs(schedule.cadence.interval_secs());
    match now.duration_since(last) {
        Ok(elapsed) => elapsed >= interval,
        Err(_) => false,
    }
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let d = i64::from(day);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
