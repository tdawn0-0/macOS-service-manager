//! Background database-task reservations, ported from `database_work.go`.
//!
//! A database task records `database-work/<id>/record.json` and reserves its
//! instance for the whole task. The reservation is the record itself: every
//! `database instance:<id>` resource lease consults
//! [`require_database_work_idle`] through the store hook that
//! [`install_database_work_guard`] installs, so the refusal is cross-process
//! (a GUI and a CLI see the same directory) and cannot be bypassed with an
//! in-process mutex.
//!
//! This slice does not run the launchd worker. It owns the records, the idle
//! discipline and the orphan-review listing; backup/restore/create actions run
//! synchronously through the operation journal.

use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::fs::{digest_bytes, read_regular_file};
use crate::model::Instance;
use crate::state::{ResourceBusyError, StateError, Store};

use super::locations::deletion_identity;
use super::{Cancelled, DatabaseError, Databases};

/// One durable database-task reservation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DatabaseWorkRecord {
    #[serde(default)]
    pub schema: i64,
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default)]
    pub identity: String,
    #[serde(default)]
    pub job_digest: String,
    #[serde(default)]
    pub parent_operation: String,
    #[serde(default)]
    pub instance: Instance,
    #[serde(default)]
    pub request: DatabaseWorkRequest,
    #[serde(default)]
    pub created_at: String,
}

/// The kind-specific half of a reservation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DatabaseWorkRequest {
    #[serde(default)]
    pub kind: String,
    #[serde(default, rename = "InstanceID")]
    pub instance_id: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub source_path: String,
}

/// What the unfinished-task UI shows for one reservation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseWorkInfo {
    pub id: String,
    pub instance_id: String,
    pub kind: String,
    pub database: String,
    pub status: String,
    pub detail: String,
    pub output: String,
    pub revision: String,
    pub caller_active: bool,
    pub running: bool,
    pub can_resolve: bool,
}

/// The Go `DatabaseWorkBusyError` shape, preserved as a typed busy resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseWorkBusyError {
    pub instance_id: String,
    pub work_id: String,
}

impl std::fmt::Display for DatabaseWorkBusyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "database instance {} has unfinished task {}; review Unfinished database tasks before changing it",
            self.instance_id, self.work_id
        )
    }
}

impl std::error::Error for DatabaseWorkBusyError {}

/// Install the cross-process database-work check on a store. The persistence
/// layer consults it immediately after it takes every `database instance`
/// lease, the same point Go's `requireDatabaseWorkIdle` runs.
pub fn install_database_work_guard(store: Store) -> Store {
    let root = store.root().to_path_buf();
    store.with_database_work_check(Arc::new(move |id| {
        require_database_work_idle(&root, id).map_err(|error| {
            if error.instance_id.is_empty() {
                // A malformed record is a hard error: it may hide a live
                // reservation, so never let it look like an unrelated busy id.
                return StateError::invalid(error.work_id);
            }
            StateError::ResourceBusy(ResourceBusyError {
                kind: "database instance".into(),
                id: error.instance_id,
            })
        })
    }))
}

fn validate_work_root(root: &Path) -> Result<Option<PathBuf>, DatabaseWorkBusyError> {
    let dir = root.join("database-work");
    match fs::symlink_metadata(&dir) {
        Ok(info) if !info.is_dir() || info.file_type().is_symlink() => Err(DatabaseWorkBusyError {
            instance_id: String::new(),
            work_id: "database-work store is not a plain directory".into(),
        }),
        Ok(_) => Ok(Some(dir)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DatabaseWorkBusyError {
            instance_id: String::new(),
            work_id: error.to_string(),
        }),
    }
}

/// The exact refusal Go raises when an instance has an unfinished task.
pub fn require_database_work_idle(
    root: &Path,
    instance_id: &str,
) -> Result<(), DatabaseWorkBusyError> {
    for record in database_work_records(root)? {
        if record.instance.id == instance_id {
            return Err(DatabaseWorkBusyError {
                instance_id: instance_id.to_string(),
                work_id: record.id,
            });
        }
    }
    Ok(())
}

/// Every valid reservation in the workspace. A malformed directory is an
/// error, never silently ignored: it may be hiding a live reservation.
pub fn database_work_records(
    root: &Path,
) -> Result<Vec<DatabaseWorkRecord>, DatabaseWorkBusyError> {
    let Some(dir) = validate_work_root(root)? else {
        return Ok(Vec::new());
    };
    let entries = fs::read_dir(&dir).map_err(|error| DatabaseWorkBusyError {
        instance_id: String::new(),
        work_id: error.to_string(),
    })?;
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| DatabaseWorkBusyError {
            instance_id: String::new(),
            work_id: error.to_string(),
        })?;
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        match read_database_work(&dir, &id) {
            Ok(record) => records.push(record),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(DatabaseWorkBusyError {
                    instance_id: String::new(),
                    work_id: error.to_string(),
                });
            }
        }
    }
    Ok(records)
}

fn read_database_work(dir: &Path, id: &str) -> std::io::Result<DatabaseWorkRecord> {
    let info = fs::symlink_metadata(dir)?;
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "database task directory is not a plain directory",
        ));
    }
    let bytes = read_regular_file(dir, "record.json", 65_536)?;
    let record: DatabaseWorkRecord = serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let identity = deletion_identity(&info)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;
    if record.schema != 1
        || record.id != id
        || record.request.instance_id != record.instance.id
        || record.identity.is_empty()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "database task identity mismatch",
        ));
    }
    if identity != record.identity {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "database task directory changed",
        ));
    }
    if !record.job_digest.is_empty() && !crate::fs::valid_digest(&record.job_digest) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid database task ownership digest",
        ));
    }
    if !matches!(
        record.request.kind.as_str(),
        "backup" | "restore" | "create-database" | "create-user"
    ) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unknown database task kind",
        ));
    }
    Ok(record)
}

impl Databases {
    fn database_work_job_running(&self, id: &str) -> Result<bool, DatabaseError> {
        let job = format!("dbwork-{id}");
        match self.engine().run_with_timeout(
            "launchctl",
            &["print", &self.engine().target(&job)],
            std::time::Duration::from_secs(1),
        ) {
            Ok(output) => {
                let state = crate::health::parse_job_state(&output);
                Ok(state.running || state.pid != 0)
            }
            Err(error) => {
                let message = error.to_string();
                if message.contains("Could not find service")
                    || message.contains("Could not find specified service")
                {
                    Ok(false)
                } else {
                    Err(DatabaseError::invalid(format!(
                        "could not observe database task {id}: {error}"
                    )))
                }
            }
        }
    }

    /// `pending_database_work`: the unfinished-task list, with result state.
    pub fn pending_database_work(
        &self,
        cancelled: Cancelled<'_>,
    ) -> Result<Vec<DatabaseWorkInfo>, DatabaseError> {
        let records = database_work_records(self.store().root())
            .map_err(|error| DatabaseError::invalid(error.to_string()))?;
        let mut infos = Vec::new();
        for record in records {
            if cancelled() {
                return Err(DatabaseError::Cancelled);
            }
            let dir = self.store().root().join("database-work").join(&record.id);
            let result = read_work_result(&dir);
            let mut info = DatabaseWorkInfo {
                id: record.id.clone(),
                instance_id: record.instance.id.clone(),
                kind: record.request.kind.clone(),
                database: record.request.database.clone(),
                status: "Interrupted".into(),
                detail: "Review this task before changing the instance. Database and output files are retained.".into(),
                ..DatabaseWorkInfo::default()
            };
            match result {
                Ok(Some(status)) => {
                    info.status = status;
                    info.detail = "The worker finished. Finish review to release this instance. Database and output files are retained.".into();
                }
                Ok(None) => {}
                Err(error) => return Err(error),
            }
            info.caller_active = control_lease_active(&dir.join("control.lock"))?;
            info.running = self.database_work_job_running(&record.id)?;
            info.can_resolve = !info.caller_active && !info.running;
            if info.caller_active {
                info.status = "Running".into();
                info.detail =
                    "The caller still owns this task. Cancel it there or wait for completion."
                        .into();
            } else if info.running {
                info.status = "Running without caller".into();
                info.detail = "The caller exited, but the recorded worker is still running. Refresh to wait, or explicitly stop it. Existing database changes are not undone.".into();
            } else if info.status == "Interrupted" {
                info.can_resolve = true;
            }
            info.output = work_artifact(&dir, &record);
            let record_bytes = fs::read(dir.join("record.json")).unwrap_or_default();
            let result_bytes = fs::read(dir.join("result.json")).unwrap_or_default();
            info.revision = digest_bytes(&[record_bytes, result_bytes].concat());
            infos.push(info);
        }
        infos.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(infos)
    }
}

fn control_lease_active(path: &Path) -> Result<bool, DatabaseError> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(false)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let result = unsafe {
        libc::flock(
            std::os::unix::io::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    if result == 0 {
        return Ok(false);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(true)
    } else {
        Err(error.into())
    }
}

fn read_work_result(dir: &Path) -> Result<Option<String>, DatabaseError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct WorkResult {
        #[serde(default)]
        status: String,
    }
    let bytes = match read_regular_file(dir, "result.json", 65_536) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let result: WorkResult = serde_json::from_slice(&bytes)?;
    if !matches!(result.status.as_str(), "Succeeded" | "Failed" | "Cancelled") {
        return Err(DatabaseError::invalid("invalid database task result"));
    }
    Ok(Some(result.status))
}

fn work_artifact(dir: &Path, record: &DatabaseWorkRecord) -> String {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Artifact {
        #[serde(default)]
        path: String,
    }
    let Ok(bytes) = read_regular_file(dir, "artifact.json", 8192) else {
        return String::new();
    };
    let Ok(artifact) = serde_json::from_slice::<Artifact>(&bytes) else {
        return String::new();
    };
    let path = crate::fs::clean_path(Path::new(&artifact.path));
    if record.request.kind != "backup"
        || path.parent().map(Path::to_path_buf)
            != Some(crate::fs::clean_path(
                &dir.join("..").join("..").join("backups"),
            ))
    {
        return String::new();
    }
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !file.starts_with(&format!("{}-", record.instance.id)) {
        return String::new();
    }
    path.display().to_string()
}
