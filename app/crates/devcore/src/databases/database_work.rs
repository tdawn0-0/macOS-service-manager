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
//! [`Databases::run_database_worker`] is the launchd worker entry point: it
//! verifies the recorded job, exempts its own record from the idle guard, runs
//! the recorded backup/restore/create action, and writes the result the
//! review APIs read. Synchronous backup/restore/create actions run through the
//! operation journal.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::fs::{
    atomic_write, create_private_dir_all, digest_bytes, read_regular_file, sync_directory,
};
use crate::health::read_job_state;
use crate::model::Instance;
use crate::operations::{OperationJournal, OperationRequest, OperationScope};
use crate::software::Software;
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
        match self.engine().job_state(&job) {
            Ok(Some(state)) => Ok(state.running || state.pid != 0),
            Ok(None) => Ok(false),
            Err(error) => Err(DatabaseError::invalid(format!(
                "could not observe database task {id}: {error}"
            ))),
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

/// A held raw resource lease. The resolver acquires the record's own instance
/// lease while the record still exists; the store's ordinary lease path would
/// refuse it through [`require_database_work_idle`], so this mirrors Go's
/// `databaseWork` context exemption by taking the same `flock(2)` directly.
struct RawResourceLease {
    _file: File,
}

fn acquire_raw_instance_lease(
    store: &Store,
    instance_id: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<RawResourceLease, StateError> {
    if !store.root().is_absolute() {
        return Err(StateError::invalid("storage root must be absolute"));
    }
    let dir = store.root().join("resource-locks");
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(StateError::invalid(
                "resource lock store must be a plain directory",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    create_private_dir_all(&dir)?;
    if cancelled() {
        return Err(StateError::Cancelled);
    }
    let value = format!("database instance:{instance_id}");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join(digest_bytes(value.as_bytes())))?;
    if !file.metadata()?.is_file() {
        return Err(StateError::invalid("resource lease must be a regular file"));
    }
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(StateError::ResourceBusy(ResourceBusyError {
                kind: "database instance".into(),
                id: instance_id.to_string(),
            }));
        }
        return Err(error.into());
    }
    Ok(RawResourceLease { _file: file })
}

/// Take the per-task `control.lock` operation lease. Any failure to open or
/// lock it means a caller still owns the task, matching Go's single message.
fn open_control_lease(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "operation lease is not a regular file",
        ));
    }
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

fn database_work_dir(root: &Path, id: &str) -> Result<PathBuf, StateError> {
    if !root.is_absolute() {
        return Err(StateError::invalid("storage root must be absolute"));
    }
    if !crate::model::valid_id(id) {
        return Err(StateError::invalid("invalid database task ID"));
    }
    let root = root.join("database-work");
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(StateError::invalid(
                "database-work store is not a plain directory",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(root.join(id))
}

/// The reviewed revision shown by [`Databases::pending_database_work`].
fn database_work_revision(dir: &Path) -> Result<String, DatabaseError> {
    let record = fs::read(dir.join("record.json"))?;
    let result = fs::read(dir.join("result.json")).unwrap_or_default();
    Ok(digest_bytes(&[record, result].concat()))
}

impl Databases {
    /// Resolve one reviewed unfinished database task.
    ///
    /// This never retries writes and never rolls back an already changed
    /// target. It marks the output for retention, stops only the
    /// digest-verified finite job, reserves the instance, records the backup
    /// artifact, and then clears the reservation. A held `control.lock`, a
    /// changed revision, or a still-held execution lease refuses the request.
    pub fn resolve_database_work(&self, id: &str, revision: &str) -> Result<(), DatabaseError> {
        let actions = Arc::new(super::DatabaseActions::new(self.clone()));
        let journal = OperationJournal::new(self.store().clone()).with_actions(actions);
        let request = OperationRequest {
            kind: "reconcile-database-work".into(),
            resource: id.to_string(),
            ..OperationRequest::default()
        };
        journal
            .run_operation(request, "", &|| false, |scope| {
                self.resolve_database_work_inner(id, revision, scope)
            })
            .map_err(DatabaseError::from)
    }

    fn resolve_database_work_inner(
        &self,
        id: &str,
        revision: &str,
        scope: &OperationScope<'_>,
    ) -> Result<(), StateError> {
        let dir = database_work_dir(self.store().root(), id)?;
        let record = read_database_work(&dir, id).map_err(StateError::Io)?;
        let _control = open_control_lease(&dir.join("control.lock")).map_err(|_| {
            StateError::invalid("database task still has a caller; cancel it there or wait")
        })?;

        // This caller now owns control.lock; the revision still validates the
        // exact files the user reviewed.
        let current =
            database_work_revision(&dir).map_err(|error| StateError::invalid(error.to_string()))?;
        if current != revision {
            return Err(StateError::invalid(
                "database task changed; refresh and review its current result",
            ));
        }

        // Keep diagnostic output even though the caller is gone; a normal
        // cancellation still removes unfinished output in the original caller.
        atomic_write(&dir.join("retain-output"), record.id.as_bytes(), 0o600)?;
        sync_directory(&dir)?;

        self.stop_database_work_job(&record).map_err(|error| {
            StateError::invalid(format!("stop database task {}: {error}", record.id))
        })?;
        let _lease = acquire_raw_instance_lease(self.store(), &record.instance.id, &|| false)?;
        let output = work_artifact(&dir, &record);
        if !output.is_empty() {
            scope.artifact(&output);
        }
        clear_database_work(&dir).map_err(|error| StateError::invalid(error.to_string()))?;
        Ok(())
    }

    /// Stop only the finite job whose plist still matches the recorded digest,
    /// then verify launchd no longer knows it. A changed job keeps its files.
    fn stop_database_work_job(&self, record: &DatabaseWorkRecord) -> Result<(), DatabaseError> {
        let job = format!("dbwork-{}", record.id);
        let path = self.engine().job_path(&job);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut bytes = Vec::new();
        match read_regular_file(parent, &name, 1 << 20) {
            Ok(data) => {
                if record.job_digest.is_empty() || digest_bytes(&data) != record.job_digest {
                    return Err(DatabaseError::invalid(
                        "database task job ownership changed; keep its files for inspection",
                    ));
                }
                self.engine().stop_job(&job, &|| false)?;
                bytes = data;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        super::lifecycle::verify_shutdown_job(self.engine(), &job, &|| false)?;
        if !bytes.is_empty() {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

fn clear_database_work(dir: &Path) -> Result<(), DatabaseError> {
    let _execution = open_control_lease(&dir.join("execution.lock")).map_err(|_| {
        DatabaseError::invalid(
            "database task process still owns its execution lease; wait and refresh",
        )
    })?;
    fs::remove_dir_all(dir)?;
    if let Some(parent) = dir.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

/// The worker's own hard deadline, matching Go's 30-minute `context.WithTimeout`.
const DATABASE_WORKER_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// `input.json`: the only place a database-task password may live. It is
/// removed as soon as the worker has read it.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DatabaseWorkInput {
    #[serde(default)]
    user: String,
    #[serde(default)]
    password: String,
}

/// `result.json`: what `pending_database_work` and `resolve_database_work`
/// read back. Field names match Go's untagged struct.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct DatabaseWorkResult {
    status: String,
    output: String,
    finished_at: String,
}

/// `artifact.json`: the published backup path `work_artifact` will read.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct DatabaseWorkArtifact {
    path: String,
}

impl Databases {
    /// `RunDatabaseWorker`: the internal launchd entry point for one recorded
    /// database task. It never replays an existing result and runs only while
    /// the recorded job identifies this process.
    ///
    /// The worker holds its own record's `execution.lock` and exempts exactly
    /// that record from the database-work idle guard, mirroring Go's
    /// `m.databaseWork` context. Other callers still see the reservation.
    pub fn run_database_worker(
        &self,
        id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), DatabaseError> {
        let deadline = Instant::now() + DATABASE_WORKER_TIMEOUT;
        let cancelled = {
            let outer = cancelled;
            move || outer() || Instant::now() >= deadline
        };

        let dir = database_work_dir(self.store().root(), id).map_err(DatabaseError::State)?;
        let record = read_database_work(&dir, id).map_err(DatabaseError::Io)?;
        if read_work_result(&dir)?.is_some() {
            return Err(DatabaseError::invalid("database task already has a result"));
        }

        // Wait like Go for another execution lease holder to release; a
        // vanished/regular-file failure is immediate.
        let _execution = loop {
            match open_execution_lease(&dir.join("execution.lock")) {
                Ok(file) => break file,
                Err(error) if error.raw_os_error() == Some(libc::EWOULDBLOCK) => {
                    if cancelled() {
                        return Err(DatabaseError::Cancelled);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => {
                    if cancelled() {
                        return Err(DatabaseError::Cancelled);
                    }
                    return Err(DatabaseError::Io(error));
                }
            }
        };

        // The plist must still be the one this record describes, and the
        // running process must be the recorded launchd job.
        let job = format!("dbwork-{id}");
        let path = self.engine().job_path(&job);
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let job_bytes = read_regular_file(parent, &name, 1 << 20)
            .map_err(|_| DatabaseError::invalid("database task job ownership is not verified"))?;
        if record.job_digest.is_empty() || digest_bytes(&job_bytes) != record.job_digest {
            return Err(DatabaseError::invalid(
                "database task job ownership is not verified",
            ));
        }
        let (job_state, error) = read_job_state(self.engine(), &job, &cancelled);
        if error.is_err() || job_state.pid != std::process::id() as i32 {
            return Err(DatabaseError::invalid(
                "database task must run as its recorded launchd job",
            ));
        }

        let input_bytes =
            read_regular_file(&dir, "input.json", 1 << 20).map_err(DatabaseError::Io)?;
        let input: DatabaseWorkInput =
            serde_json::from_slice(&input_bytes).map_err(DatabaseError::Json)?;
        // Go defers this removal; reading is the last moment the password is
        // needed, so remove before any database client runs.
        let _ = fs::remove_file(dir.join("input.json"));

        super::verify_data_location(self.store(), &record.instance)?;
        self.store().emit(&format!("Executing database task {id}"));

        let worker = self.worker_databases(id);
        let request = &record.request;
        let executed: Result<String, DatabaseError> = match request.kind.as_str() {
            "backup" => worker.backup(&request.instance_id, &request.database, &cancelled),
            "restore" => worker
                .restore(
                    &request.instance_id,
                    &request.database,
                    &request.source_path,
                    &cancelled,
                )
                .map(|()| String::new()),
            "create-database" => worker
                .create_database(&request.instance_id, &request.database, &cancelled)
                .map(|()| String::new()),
            "create-user" => worker
                .create_user(
                    &request.instance_id,
                    &input.user,
                    &input.password,
                    &request.database,
                    &cancelled,
                )
                .map(|()| String::new()),
            _ => Err(DatabaseError::invalid("unknown database task kind")),
        };

        let (status, output, failure) = match executed {
            Ok(output) => ("Succeeded", output, None),
            Err(error) => {
                if cancelled() {
                    ("Cancelled", String::new(), Some(error))
                } else {
                    ("Failed", String::new(), Some(error))
                }
            }
        };

        if let Some(error) = &failure {
            let mut secrets = vec![input.password.clone()];
            let password_path = self
                .store()
                .root()
                .join("instances")
                .join(&record.instance.id)
                .join("password");
            if let Ok(bytes) = read_regular_file(
                password_path.parent().unwrap_or_else(|| Path::new(".")),
                password_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default(),
                4096,
            ) {
                secrets.push(String::from_utf8_lossy(&bytes).into_owned());
            }
            let message = redact_worker_message(error.to_string(), &secrets);
            atomic_write(&dir.join("error.log"), message.as_bytes(), 0o600)?;
        }

        if status == "Succeeded" && request.kind == "backup" && !output.is_empty() {
            let artifact = DatabaseWorkArtifact {
                path: output.clone(),
            };
            let mut artifact_bytes =
                serde_json::to_vec_pretty(&artifact).map_err(DatabaseError::Json)?;
            artifact_bytes.push(b'\n');
            atomic_write(&dir.join("artifact.json"), &artifact_bytes, 0o600)?;
            sync_directory(&dir)?;
        }

        let result = DatabaseWorkResult {
            status: status.into(),
            output,
            finished_at: crate::software::utc_now_rfc3339(),
        };
        let mut bytes = serde_json::to_vec_pretty(&result).map_err(DatabaseError::Json)?;
        bytes.push(b'\n');
        atomic_write(&dir.join("result.json"), &bytes, 0o600)?;
        sync_directory(&dir)?;
        if failure.is_some() {
            return Err(DatabaseError::invalid(format!(
                "database task {id} did not complete"
            )));
        }
        Ok(())
    }

    /// A clone of this manager whose database-work guard exempts `exempt_id`,
    /// the Rust equivalent of Go's worker-held `m.databaseWork` context.
    fn worker_databases(&self, exempt_id: &str) -> Databases {
        let root = self.store().root().to_path_buf();
        let exempt = exempt_id.to_string();
        let store = self
            .store()
            .clone()
            .with_database_work_check(Arc::new(move |instance_id| {
                require_database_work_idle_except(&root, instance_id, Some(&exempt)).map_err(
                    |error| {
                        if error.instance_id.is_empty() {
                            StateError::invalid(error.work_id)
                        } else {
                            StateError::ResourceBusy(ResourceBusyError {
                                kind: "database instance".into(),
                                id: error.instance_id,
                            })
                        }
                    },
                )
            }));
        let runner = self.runner.clone();
        Databases {
            engine: Engine::new(store.root(), runner.clone()),
            software: Software::new(store.clone()).with_runner(runner.clone()),
            store,
            runner,
            binder: self.binder.clone(),
            signaler: self.signaler.clone(),
            probe: self.probe.clone(),
            mongo: self.mongo.clone(),
            redis: self.redis.clone(),
            publish_fault: None,
        }
    }
}

/// The database-work guard with Go's worker exemption: only the record the
/// worker itself holds stops blocking `instance_id`.
fn require_database_work_idle_except(
    root: &Path,
    instance_id: &str,
    exempt: Option<&str>,
) -> Result<(), DatabaseWorkBusyError> {
    for record in database_work_records(root)? {
        if record.instance.id == instance_id && Some(record.id.as_str()) != exempt {
            return Err(DatabaseWorkBusyError {
                instance_id: instance_id.to_string(),
                work_id: record.id,
            });
        }
    }
    Ok(())
}

/// The read-only exclusive lease Go's `openOperationLease` takes.
fn open_execution_lease(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "operation lease is not a regular file",
        ));
    }
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

/// Replace the task's passwords, including their JSON-escaped forms, exactly
/// like the Go worker does before writing `error.log`.
fn redact_worker_message(mut message: String, secrets: &[String]) -> String {
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        message = message.replace(secret, "[redacted]");
        if let Ok(quoted) = serde_json::to_string(secret)
            && quoted.len() > 2
        {
            message = message.replace(&quoted[1..quoted.len() - 1], "[redacted]");
        }
    }
    if message.len() > 16_000 {
        let mut limit = 16_000;
        while !message.is_char_boundary(limit) {
            limit -= 1;
        }
        message.truncate(limit);
    }
    message
}

#[cfg(test)]
#[path = "database_work_resolve_tests.rs"]
mod resolve_tests;

#[cfg(test)]
#[path = "database_work_worker_tests.rs"]
mod worker_tests;
