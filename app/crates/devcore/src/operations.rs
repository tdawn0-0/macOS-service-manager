//! The cross-process operation journal, ported from the Go `operations.go`.
//!
//! Every mutating task creates `operations/<16-hex>/record.json` beside an
//! `owner.lock`, and holds an exclusive `flock(2)` on that lock for its whole
//! life. The lock is the cross-process truth: [`OperationJournal::operations`]
//! probes it to decide whether a record is still `Busy`, and a live owner
//! cannot be dismissed or retried from another caller.
//!
//! Records deliberately store no command output and no raw error text: either
//! may contain credentials. The fixed result strings are part of that privacy
//! contract, not placeholders. Nor do requests carry passwords, arguments or
//! environment; [`OperationJournal::create_user`] takes a password and never
//! writes it to the record.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::fs::{PRIVATE_FILE, atomic_write, digest_bytes, sync_directory};
use crate::model::Package;
use crate::state::{StateError, Store};

/// The record schema this build writes and reads.
pub const OPERATION_SCHEMA: i64 = 1;

/// Fixed statuses and result text. Never persist output or raw errors.
pub const STATUS_RUNNING: &str = "Running";
pub const STATUS_SUCCEEDED: &str = "Succeeded";
pub const STATUS_FAILED: &str = "Failed";
pub const STATUS_CANCELLED: &str = "Cancelled";
pub const STATUS_INTERRUPTED: &str = "Interrupted";

const RESULT_SUCCEEDED: &str = "Completed.";
const RESULT_FAILED: &str = "Stopped with an error. Inspect the current resource before retrying.";
const RESULT_CANCELLED: &str =
    "Cancelled or timed out. Partial work may remain; inspect the current resource.";
const RESULT_INTERRUPTED: &str = "Owner released its operation lease without a recorded result. Recheck live jobs and resource state; no success is assumed.";

static OPERATION_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,79}$").unwrap());

fn is_zero(value: &i64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The request half of an operation. Command arguments, environment,
/// passwords and URLs are deliberately absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OperationRequest {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub port: i64,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub resource: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default, rename = "SHA256")]
    pub sha256: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub source_path: String,
}

/// One durable operation record. Field names are the Go `encoding/json`
/// names, so a CLI and the GUI can read each other's journals during the
/// phased cutover.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OperationRecord {
    #[serde(default)]
    pub schema: i64,
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default)]
    pub request: OperationRequest,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub result: String,
    #[serde(default)]
    pub artifact: String,
    #[serde(default)]
    pub retry_of: String,
    #[serde(default)]
    pub followup: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub revision: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub busy: bool,
}

impl OperationRecord {
    pub fn phase_is_running(&self) -> bool {
        self.status == STATUS_RUNNING
    }
}

/// The progress and artifact surface handed to an operation's work.
///
/// Progress messages are scanned for the fixed Go phase prefixes; the original
/// message then goes to the journal's event sink.
pub struct OperationScope<'a> {
    writer: &'a OperationWriter,
    event: &'a (dyn Fn(&str) + Send + Sync),
}

impl OperationScope<'_> {
    /// Set the phase directly, as `CreateInstance`/`Backup` do in Go.
    pub fn phase(&self, phase: &str) {
        self.writer.phase(phase);
    }

    /// Advance the phase when `message` starts with a known progress prefix,
    /// then forward the message unchanged.
    pub fn emit(&self, message: &str) {
        if let Some(phase) = operation_phase(message) {
            self.writer.phase(phase);
        }
        (self.event)(message);
    }

    /// Record where a backup landed.
    pub fn artifact(&self, path: &str) {
        self.writer.artifact(path);
    }
}

/// Work the journal can run. The actual install/database slices implement
/// this; the journal owns only the record, the owner lease and retry lineage.
pub trait OperationActions: Send + Sync {
    fn install(&self, scope: &OperationScope<'_>, package: &Package) -> Result<(), StateError>;

    fn repair_package(&self, scope: &OperationScope<'_>, id: &str) -> Result<(), StateError>;

    fn create_instance(
        &self,
        scope: &OperationScope<'_>,
        instance: &crate::model::Instance,
    ) -> Result<(), StateError>;

    fn backup(
        &self,
        scope: &OperationScope<'_>,
        id: &str,
        name: &str,
    ) -> Result<String, StateError>;

    fn restore(
        &self,
        scope: &OperationScope<'_>,
        id: &str,
        name: &str,
        path: &str,
    ) -> Result<(), StateError>;

    fn instance_action(
        &self,
        scope: &OperationScope<'_>,
        id: &str,
        action: &str,
    ) -> Result<(), StateError>;

    fn create_database(
        &self,
        scope: &OperationScope<'_>,
        id: &str,
        name: &str,
    ) -> Result<(), StateError>;

    fn create_user(
        &self,
        scope: &OperationScope<'_>,
        id: &str,
        name: &str,
        password: &str,
        database: &str,
    ) -> Result<(), StateError>;

    /// Used by retry to prove the install source still matches the record.
    fn resolve_package(&self, name: &str, version: &str) -> Result<Package, StateError>;

    /// Run an action that the Go wrapper deliberately does not journal
    /// (anything other than start/stop/restart).
    fn instance_action_direct(&self, id: &str, action: &str) -> Result<(), StateError>;
}

/// The journal for one development home.
#[derive(Clone)]
pub struct OperationJournal {
    store: Store,
    actions: Option<Arc<dyn OperationActions>>,
    event: Arc<dyn Fn(&str) + Send + Sync>,
}

impl std::fmt::Debug for OperationJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperationJournal")
            .field("root", &self.store.root())
            .finish()
    }
}

impl OperationJournal {
    pub fn new(store: Store) -> Self {
        OperationJournal {
            store,
            actions: None,
            event: Arc::new(|_| {}),
        }
    }

    pub fn with_actions(mut self, actions: Arc<dyn OperationActions>) -> Self {
        self.actions = Some(actions);
        self
    }

    pub fn with_event(mut self, event: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.event = Arc::new(event);
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn operation_root(&self) -> PathBuf {
        self.store.root().join("operations")
    }

    /// The executor, or the "not configured" error the UI shows for tests.
    pub fn actions(&self) -> Result<Arc<dyn OperationActions>, StateError> {
        self.actions
            .clone()
            .ok_or_else(|| StateError::invalid("operation executor is not configured"))
    }

    /// Run one operation under a fresh owner lease.
    ///
    /// `retry_of` links a retry to the record it followed; the parent's
    /// `Followup` is written before any follow-up work starts.
    pub fn run_operation<F>(
        &self,
        request: OperationRequest,
        retry_of: &str,
        cancelled: &dyn Fn() -> bool,
        work: F,
    ) -> Result<(), StateError>
    where
        F: FnOnce(&OperationScope<'_>) -> Result<(), StateError>,
    {
        for value in [
            &request.resource,
            &request.name,
            &request.version,
            &request.arch,
            &request.sha256,
            &request.database,
        ] {
            if value.len() > 256 || value.contains(['\0', '\n', '\r']) {
                return Err(StateError::invalid("operation metadata is invalid"));
            }
        }
        if request.source_path.len() > 4096 || request.source_path.contains('\0') {
            return Err(StateError::invalid("operation source path is invalid"));
        }
        if cancelled() {
            return Err(StateError::Cancelled);
        }
        validate_operation_root(&self.store)?;
        self.store.init()?;
        let root = self.operation_root();
        create_private_dir(&root)?;
        let id = random_id();
        let dir = root.join(&id);
        create_private_dir(&dir)?;
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(PRIVATE_FILE)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join("owner.lock"))?;
        if !try_flock(&owner, libc::LOCK_EX)? {
            return Err(StateError::invalid("operation owner lease is busy"));
        }
        let writer = OperationWriter {
            path: dir.join("record.json"),
            state: Mutex::new(WriterState {
                record: OperationRecord {
                    schema: OPERATION_SCHEMA,
                    id: id.clone(),
                    request: request.clone(),
                    phase: "Queued".into(),
                    status: STATUS_RUNNING.into(),
                    retry_of: retry_of.to_string(),
                    started_at: now_rfc3339(),
                    ..OperationRecord::default()
                },
                error: None,
            }),
        };
        writer.save();
        if let Some(error) = writer.error() {
            return Err(StateError::invalid(error));
        }
        sync_directory(&root)?;

        (self.event)(&format!("Operation {id} · {}", request.kind));

        let mut failure: Option<StateError> = None;
        if !retry_of.is_empty() {
            match self.operation_files(retry_of) {
                Ok((parent_path, _)) => match read_operation(&parent_path, retry_of) {
                    Ok(mut prior) => {
                        prior.followup = id.clone();
                        prior.updated_at = now_rfc3339();
                        if let Err(error) = write_operation_record(&parent_path, &prior) {
                            failure = Some(error);
                        }
                    }
                    Err(error) => failure = Some(error),
                },
                Err(error) => failure = Some(error),
            }
        }
        if failure.is_none() {
            let scope = OperationScope {
                writer: &writer,
                event: self.event.as_ref(),
            };
            failure = work(&scope).err();
        }
        let was_cancelled = cancelled() || matches!(failure.as_ref(), Some(StateError::Cancelled));
        writer.finish(failure.is_some(), was_cancelled);
        let save_error = writer.error();
        match (failure, save_error) {
            (Some(error), Some(save)) => Err(StateError::invalid(format!("{error}; {save}"))),
            (Some(error), None) => Err(error),
            (None, Some(save)) => Err(StateError::invalid(save)),
            (None, None) => Ok(()),
        }
    }

    /// The wrapper around [`OperationActions::install`].
    pub fn install(
        &self,
        package: &Package,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        let actions = self.actions()?;
        let request = OperationRequest {
            kind: "install".into(),
            resource: package.id(),
            name: package.name.clone(),
            version: package.version.clone(),
            arch: package.arch.clone(),
            sha256: package.sha256.clone(),
            ..OperationRequest::default()
        };
        self.run_operation(request, "", cancelled, |scope| {
            actions.install(scope, package)
        })
    }

    pub fn repair_package(&self, id: &str, cancelled: &dyn Fn() -> bool) -> Result<(), StateError> {
        let actions = self.actions()?;
        let request = OperationRequest {
            kind: "repair".into(),
            resource: id.to_string(),
            ..OperationRequest::default()
        };
        self.run_operation(request, "", cancelled, |scope| {
            actions.repair_package(scope, id)
        })
    }

    pub fn create_instance(
        &self,
        instance: &crate::model::Instance,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        let actions = self.actions()?;
        let request = OperationRequest {
            kind: "initialize".into(),
            resource: instance.id.clone(),
            name: instance.kind.clone(),
            version: instance.package.clone(),
            port: instance.port,
            ..OperationRequest::default()
        };
        self.run_operation(request, "", cancelled, |scope| {
            scope.phase("Initializing");
            actions.create_instance(scope, instance)
        })
    }

    /// Run a backup and return the artifact path. The path is recorded in the
    /// journal; the backup contents never are.
    pub fn backup(
        &self,
        id: &str,
        name: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String, StateError> {
        let actions = self.actions()?;
        let request = OperationRequest {
            kind: "backup".into(),
            resource: id.to_string(),
            database: name.to_string(),
            ..OperationRequest::default()
        };
        let mut path = String::new();
        let result = self.run_operation(request, "", cancelled, |scope| {
            scope.phase("Backing up");
            path = actions.backup(scope, id, name)?;
            scope.artifact(&path);
            Ok(())
        });
        match result {
            Ok(()) => Ok(path),
            Err(error) => Err(error),
        }
    }

    pub fn restore(
        &self,
        id: &str,
        name: &str,
        path: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        if path.ends_with(".partial") {
            return Err(StateError::invalid(
                "incomplete backup output cannot be restored",
            ));
        }
        let actions = self.actions()?;
        let request = OperationRequest {
            kind: "restore".into(),
            resource: id.to_string(),
            database: name.to_string(),
            source_path: path.to_string(),
            ..OperationRequest::default()
        };
        self.run_operation(request, "", cancelled, |scope| {
            scope.phase("Restoring");
            actions.restore(scope, id, name, path)
        })
    }

    pub fn instance_action(
        &self,
        id: &str,
        action: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        let actions = self.actions()?;
        if action != "start" && action != "stop" && action != "restart" {
            // Unknown actions are not journal kinds; run them directly as the
            // Go wrapper does.
            return actions.instance_action_direct(id, action);
        }
        let request = OperationRequest {
            kind: format!("db-{action}"),
            resource: id.to_string(),
            ..OperationRequest::default()
        };
        self.run_operation(request, "", cancelled, |scope| {
            actions.instance_action(scope, id, action)
        })
    }

    pub fn create_database(
        &self,
        id: &str,
        name: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        let actions = self.actions()?;
        let request = OperationRequest {
            kind: "create-database".into(),
            resource: id.to_string(),
            database: name.to_string(),
            ..OperationRequest::default()
        };
        self.run_operation(request, "", cancelled, |scope| {
            actions.create_database(scope, id, name)
        })
    }

    /// The password is passed to the executor and never enters the record.
    pub fn create_user(
        &self,
        id: &str,
        name: &str,
        password: &str,
        database: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        let actions = self.actions()?;
        let request = OperationRequest {
            kind: "create-user".into(),
            resource: id.to_string(),
            database: database.to_string(),
            ..OperationRequest::default()
        };
        self.run_operation(request, "", cancelled, |scope| {
            actions.create_user(scope, id, name, password, database)
        })
    }

    /// List every record newest first. A held owner lease marks the record
    /// `Busy`; a released lease on a still-`Running` record is reported as
    /// `Interrupted` (without rewriting the record).
    pub fn operations(
        &self,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<OperationRecord>, StateError> {
        validate_operation_root(&self.store)?;
        let entries = match std::fs::read_dir(self.operation_root()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut records = Vec::new();
        for entry in entries {
            if cancelled() {
                return Err(StateError::Cancelled);
            }
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let id = name.to_string_lossy();
            let (record_path, lease_path) = self.operation_files(&id)?;
            let mut record = match read_operation(&record_path, &id) {
                Ok(record) => record,
                Err(StateError::Io(error)) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            match open_operation_lease(&lease_path)? {
                LeaseAttempt::Busy => record.busy = true,
                LeaseAttempt::Acquired(lease) => {
                    let latest = read_operation(&record_path, &id);
                    drop(lease);
                    match latest {
                        Ok(latest) => {
                            record = latest;
                            if record.status == STATUS_RUNNING {
                                record.status = STATUS_INTERRUPTED.into();
                                record.result = RESULT_INTERRUPTED.into();
                            }
                        }
                        Err(StateError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            records.push(record);
        }
        records.sort_by(|left, right| {
            started_key(&right.started_at).cmp(&started_key(&left.started_at))
        });
        Ok(records)
    }

    /// Delete a finished record, refusing a live owner or a stale revision.
    pub fn dismiss_operation(
        &self,
        id: &str,
        revision: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        self.with_reviewed_operation(id, revision, cancelled, |path, _| {
            std::fs::remove_file(path)?;
            sync_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
            Ok(())
        })
    }

    /// Retry a failed operation with the same request, linking the new record
    /// to the old one.
    pub fn retry_operation(
        &self,
        id: &str,
        revision: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), StateError> {
        let actions = self.actions()?;
        self.with_reviewed_operation(id, revision, cancelled, |_, mut record| {
            if record.status == STATUS_RUNNING {
                record.status = STATUS_INTERRUPTED.into();
            }
            if !operation_can_retry(&record) {
                return Err(StateError::invalid(
                    "review this operation in its resource settings; automatic retry is unavailable",
                ));
            }
            let request = record.request.clone();
            let parent = record.id.clone();
            self.run_operation(request.clone(), &parent, cancelled, |scope| match request
                .kind
                .as_str()
            {
                "install" => {
                    let package =
                        actions.resolve_package(&request.name, &request.version)?;
                    if package.id() != request.resource
                        || package.arch != request.arch
                        || package.sha256 != request.sha256
                    {
                        return Err(StateError::invalid(
                            "package source changed; review a fresh package preview",
                        ));
                    }
                    actions.install(scope, &package)
                }
                "repair" => actions.repair_package(scope, &request.resource),
                "db-start" | "db-stop" | "db-restart" => actions.instance_action(
                    scope,
                    &request.resource,
                    request.kind.trim_start_matches("db-"),
                ),
                "backup" => {
                    let path = actions.backup(scope, &request.resource, &request.database)?;
                    scope.artifact(&path);
                    Ok(())
                }
                _ => Err(StateError::invalid(format!(
                    "unsupported operation {}",
                    request.kind
                ))),
            })
        })
    }

    fn operation_files(&self, id: &str) -> Result<(PathBuf, PathBuf), StateError> {
        if !OPERATION_ID_RE.is_match(id) {
            return Err(StateError::invalid(format!("invalid identifier {id:?}")));
        }
        validate_operation_root(&self.store)?;
        let dir = self.operation_root().join(id);
        let info = std::fs::symlink_metadata(&dir)?;
        if !info.is_dir() || info.file_type().is_symlink() {
            return Err(StateError::invalid(
                "operation directory is not a plain directory",
            ));
        }
        Ok((dir.join("record.json"), dir.join("owner.lock")))
    }

    fn with_reviewed_operation<F>(
        &self,
        id: &str,
        revision: &str,
        cancelled: &dyn Fn() -> bool,
        action: F,
    ) -> Result<(), StateError>
    where
        F: FnOnce(&Path, OperationRecord) -> Result<(), StateError>,
    {
        if cancelled() {
            return Err(StateError::Cancelled);
        }
        let (record_path, lease_path) = self.operation_files(id)?;
        let lease = match open_operation_lease(&lease_path)? {
            LeaseAttempt::Busy => {
                return Err(StateError::invalid(
                    "operation is still owned by a live caller; wait or cancel it in that caller",
                ));
            }
            LeaseAttempt::Acquired(lease) => lease,
        };
        let record = read_operation(&record_path, id)?;
        if record.revision != revision {
            return Err(StateError::invalid(
                "operation changed; refresh before acting",
            ));
        }
        let result = action(&record_path, record);
        drop(lease);
        result
    }
}

/// Whether a record offers automatic retry.
pub fn operation_can_retry(record: &OperationRecord) -> bool {
    if record.busy
        || !record.followup.is_empty()
        || record.status == STATUS_SUCCEEDED
        || record.status == STATUS_RUNNING
    {
        return false;
    }
    matches!(
        record.request.kind.as_str(),
        "install" | "repair" | "db-start" | "db-stop" | "db-restart" | "backup"
    )
}

/// The phase a work kind starts in.
pub fn operation_work_phase(kind: &str) -> &'static str {
    match kind {
        "install" | "repair" => "Preparing",
        "initialize" => "Initializing",
        "backup" => "Backing up",
        "restore" => "Restoring",
        "db-start" => "Starting",
        "db-stop" => "Stopping",
        "db-restart" => "Restarting",
        _ => "Working",
    }
}

/// Map a progress message to a phase. Only the fixed prefixes the Go event
/// hook knows are eligible.
pub fn operation_phase(message: &str) -> Option<&'static str> {
    const PREFIXES: [(&str, &str); 9] = [
        ("Checking installed package ", "Checking files"),
        ("Downloading ", "Downloading"),
        ("Verifying and extracting ", "Extracting"),
        ("Committing package ", "Publishing"),
        ("Starting ", "Starting"),
        ("Stopping ", "Stopping"),
        ("Waiting ", "Waiting"),
        ("Installed ", "Committed"),
        ("Repaired ", "Committed"),
    ];
    PREFIXES
        .iter()
        .find(|(prefix, _)| message.starts_with(prefix))
        .map(|(_, phase)| *phase)
}

struct WriterState {
    record: OperationRecord,
    error: Option<String>,
}

struct OperationWriter {
    path: PathBuf,
    state: Mutex<WriterState>,
}

impl OperationWriter {
    fn save(&self) {
        let mut state = self.state.lock().unwrap();
        self.save_locked(&mut state);
    }

    fn save_locked(&self, state: &mut WriterState) {
        if state.error.is_some() {
            return;
        }
        state.record.updated_at = now_rfc3339();
        if let Err(error) = write_operation_record(&self.path, &state.record) {
            state.error = Some(error.to_string());
        }
    }

    fn phase(&self, phase: &str) {
        let mut state = self.state.lock().unwrap();
        if state.record.phase != phase {
            state.record.phase = phase.to_string();
            self.save_locked(&mut state);
        }
    }

    fn artifact(&self, path: &str) {
        let mut state = self.state.lock().unwrap();
        state.record.artifact = path.to_string();
        self.save_locked(&mut state);
    }

    fn finish(&self, failed: bool, cancelled: bool) {
        let mut state = self.state.lock().unwrap();
        state.record.phase = "Finished".into();
        if failed {
            state.record.status = if cancelled {
                STATUS_CANCELLED
            } else {
                STATUS_FAILED
            }
            .into();
            state.record.result = if cancelled {
                RESULT_CANCELLED
            } else {
                RESULT_FAILED
            }
            .into();
        } else {
            state.record.status = STATUS_SUCCEEDED.into();
            state.record.result = RESULT_SUCCEEDED.into();
        }
        self.save_locked(&mut state);
    }

    fn error(&self) -> Option<String> {
        self.state.lock().unwrap().error.clone()
    }
}

fn write_operation_record(path: &Path, record: &OperationRecord) -> Result<(), StateError> {
    let mut record = record.clone();
    record.revision.clear();
    record.busy = false;
    let mut bytes = serde_json::to_vec_pretty(&record)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes, PRIVATE_FILE)?;
    sync_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
    Ok(())
}

fn read_operation(path: &Path, id: &str) -> Result<OperationRecord, StateError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(StateError::invalid(
            "operation record is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err(StateError::invalid("operation record is too large"));
    }
    let mut record: OperationRecord = serde_json::from_slice(&bytes)?;
    if record.schema != OPERATION_SCHEMA || record.id != id {
        return Err(StateError::invalid("operation record identity mismatch"));
    }
    record.revision = digest_bytes(&bytes);
    record.busy = false;
    Ok(record)
}

enum LeaseAttempt {
    Acquired(File),
    Busy,
}

fn open_operation_lease(path: &Path) -> Result<LeaseAttempt, StateError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(StateError::invalid("operation lease is not a regular file"));
    }
    if try_flock(&file, libc::LOCK_EX)? {
        Ok(LeaseAttempt::Acquired(file))
    } else {
        Ok(LeaseAttempt::Busy)
    }
}

fn try_flock(file: &File, operation: libc::c_int) -> io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(false),
        _ => Err(error),
    }
}

fn validate_operation_root(store: &Store) -> Result<(), StateError> {
    if !store.root().is_absolute() {
        return Err(StateError::invalid("storage root must be absolute"));
    }
    let root = store.root().join("operations");
    match std::fs::symlink_metadata(&root) {
        Ok(info) if !info.is_dir() || info.file_type().is_symlink() => Err(StateError::invalid(
            "operation store must be a plain directory",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let info = std::fs::symlink_metadata(path)?;
            if info.is_dir() && !info.file_type().is_symlink() {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

fn random_id() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut file) = File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    }
    crate::fs::encode_hex(&bytes)
}

/// RFC 3339 in UTC with Go's `RFC3339Nano` trailing-zero trimming.
fn now_rfc3339() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as i64;
    let nanos = now.subsec_nanos();
    let mut time: libc::tm = unsafe { std::mem::zeroed() };
    let raw = seconds as libc::time_t;
    unsafe {
        libc::gmtime_r(&raw, &mut time);
    }
    let mut text = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        time.tm_year + 1900,
        time.tm_mon + 1,
        time.tm_mday,
        time.tm_hour,
        time.tm_min,
        time.tm_sec
    );
    if nanos > 0 {
        let mut fraction = format!("{nanos:09}");
        while fraction.ends_with('0') {
            fraction.pop();
        }
        text.push('.');
        text.push_str(&fraction);
    }
    text.push('Z');
    text
}

/// Epoch nanoseconds for a Go/RFC 3339 timestamp, used only for newest-first
/// ordering. Unparseable values sort last.
fn started_key(value: &str) -> i128 {
    parse_rfc3339_nanos(value).unwrap_or(i128::MIN)
}

fn parse_rfc3339_nanos(value: &str) -> Option<i128> {
    let (main, offset) = if let Some(main) = value.strip_suffix('Z') {
        (main, 0i64)
    } else if let Some(main) = value.strip_suffix('z') {
        (main, 0)
    } else {
        let index = value.rfind(['+', '-'])?;
        let sign = if value.as_bytes()[index] == b'+' {
            1
        } else {
            -1
        };
        let zone = &value[index + 1..];
        let (hours, minutes) = zone.split_once(':')?;
        let hours: i64 = hours.parse().ok()?;
        let minutes: i64 = minutes.parse().ok()?;
        (
            value[..index].trim_end(),
            sign * (hours * 3600 + minutes * 60),
        )
    };
    let (date, clock) = main.split_once('T').or_else(|| main.split_once('t'))?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    let (clock, fraction) = match clock.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (clock, None),
    };
    let mut clock = clock.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    let nanos: i64 = match fraction {
        Some(fraction) => {
            let digits: String = fraction.chars().take(9).collect();
            if !digits.chars().all(|character| character.is_ascii_digit()) {
                return None;
            }
            let scale = 10i64.checked_pow(9 - digits.len() as u32)?;
            digits.parse::<i64>().ok()? * scale
        }
        None => 0,
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset;
    Some(i128::from(seconds) * 1_000_000_000 + i128::from(nanos))
}

/// Howard Hinnant's `days_from_civil`, the inverse of `civil_from_days`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;
