//! Staged instance creation and initialization, ported from
//! `instance_initialization.go:190-357`.
//!
//! The data directory is built under `initializations/<random>/data` while
//! the `database instance:<id>` and `TCP port:<port>` resource leases (plus
//! `database locations:creation` for an external location) are held. Every
//! initialization job is recorded with its property-list digest, so only
//! jobs this operation wrote are ever stopped. Publication is a same-volume
//! `rename` into `instances/<id>` inside the state transaction; the undo
//! renames it back. A failure therefore never leaves a registered or live
//! partial instance.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::engine::{Engine, JobSpec};
use crate::fs::{
    PRIVATE_DIR, PRIVATE_FILE, atomic_write, create_private_dir_all, digest_bytes,
    read_regular_file, sync_directory,
};
use crate::model::{Instance, Package, State};
use crate::software::random_id;
use crate::state::{ResourceKey, Tx};

use super::lifecycle::verify_shutdown_target;
use super::locations::{database_data_directory, prepare_data_location, verify_data_location};
use super::{Cancelled, DatabaseError, Databases};

/// The initialization record written beside the staged data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializationRecord {
    #[serde(rename = "Schema")]
    pub schema: i64,
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Instance")]
    pub instance: Instance,
    #[serde(rename = "Identity")]
    pub identity: String,
    #[serde(rename = "Jobs")]
    pub jobs: BTreeMap<String, String>,
    #[serde(rename = "Ready")]
    pub ready: bool,
    #[serde(rename = "CreatedAt")]
    pub created_at: String,
}

impl InitializationRecord {
    pub fn new(id: String, instance: Instance, identity: String) -> Self {
        InitializationRecord {
            schema: 1,
            id,
            instance,
            identity,
            jobs: BTreeMap::new(),
            ready: false,
            created_at: crate::software::utc_now_rfc3339(),
        }
    }
}

/// The live preparation context. The engine's `job_saved` hook records each
/// job's digest here before launchd loads it.
pub(crate) struct InitContext {
    pub record: InitializationRecord,
    pub record_path: PathBuf,
    pub data_dir: PathBuf,
}

impl Databases {
    /// Run a preparation against an isolated context. The returned manager
    /// shares every production seam but has its own job-digest hook.
    fn worker(&self, context: Arc<Mutex<InitContext>>) -> Databases {
        let hook_context = context.clone();
        let engine = Engine::new(self.store().root(), self.runner())
            .with_uid(self.engine().uid())
            .with_command_timeout(self.engine().command_timeout())
            .with_stop_timeout(self.engine().stop_timeout())
            .with_state_timeout(self.engine().state_timeout())
            .with_poll_interval(self.engine().poll_interval())
            .with_job_saved_hook(Arc::new(move |job, path| {
                record_initialization_job(&hook_context, job, path)
                    .map_err(|error| crate::state::StateError::invalid(error.to_string()))
            }));
        Databases {
            store: self.store().clone(),
            runner: self.runner(),
            engine,
            software: self.software().clone(),
            binder: self.binder_arc(),
            signaler: self.signaler_arc(),
            probe: self.probe_arc(),
            mongo: self.mongo_arc(),
            redis: self.redis_arc(),
            publish_fault: self.publish_fault_clone(),
        }
    }

    /// `CreateInstance`: validate, stage, initialize and publish one instance.
    pub fn create_instance(
        &self,
        instance: Instance,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        let mut instance = instance;
        if !crate::model::valid_id(&instance.id) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {:?}",
                instance.id
            )));
        }
        if !matches!(
            instance.kind.as_str(),
            "postgresql" | "mysql" | "mongodb" | "redis"
        ) {
            return Err(DatabaseError::invalid(
                "supported engines: mysql, postgresql, redis, mongodb",
            ));
        }
        if !(1024..=65535).contains(&instance.port) {
            return Err(DatabaseError::invalid("database port must be 1024–65535"));
        }
        if !instance.data_directory.is_empty() {
            instance.data_owner = random_id();
        }
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        self.init()?;

        let mut keys = vec![
            ResourceKey::new("database instance", instance.id.clone()),
            ResourceKey::new("TCP port", instance.port.to_string()),
        ];
        if !instance.data_directory.is_empty() {
            keys.push(ResourceKey::new("database locations", "creation"));
        }
        let _resources = self.store().resource_leases_cancellable(&keys, cancelled)?;
        let _package =
            self.store()
                .package_lease_cancellable(&instance.package, false, cancelled)?;

        let state = self.store().read()?;
        self.check_new_instance(&state, &instance)?;
        let package = state
            .packages
            .get(&instance.package)
            .cloned()
            .ok_or_else(|| DatabaseError::invalid("database software is not installed"))?;
        package.validate()?;

        let initialization_id = random_id();
        let dir = self
            .store()
            .root()
            .join("initializations")
            .join(&initialization_id);
        let record_path = dir.join("record.json");
        let data_dir = dir.join("data");
        create_private_dir_all(&self.store().root().join("initializations"))?;
        fs::DirBuilder::new().mode(PRIVATE_DIR).create(&dir)?;
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(PRIVATE_FILE)
            .open(dir.join("owner.lock"))?;
        let lock_result = unsafe {
            libc::flock(
                std::os::unix::io::AsRawFd::as_raw_fd(&owner),
                libc::LOCK_EX | libc::LOCK_NB,
            )
        };
        if lock_result != 0 {
            return Err(DatabaseError::Io(std::io::Error::last_os_error()));
        }
        fs::DirBuilder::new().mode(PRIVATE_DIR).create(&data_dir)?;
        let metadata = fs::symlink_metadata(&data_dir)?;
        let identity = super::deletion_identity(&metadata)?;
        let context = Arc::new(Mutex::new(InitContext {
            record: InitializationRecord::new(
                initialization_id.clone(),
                instance.clone(),
                identity,
            ),
            record_path: record_path.clone(),
            data_dir: data_dir.clone(),
        }));
        write_context_record(&context)?;
        sync_directory(&dir)?;
        sync_directory(&self.store().root().join("initializations"))?;

        let worker = self.worker(context.clone());
        let prepared =
            worker.prepare_and_initialize(&mut instance, &state, &package, &context, cancelled);
        if let Err(error) = prepared {
            let cleanup = worker.stop_initialization_jobs(&context, &|| false);
            return Err(match cleanup {
                Ok(()) => error,
                Err(cleanup) => DatabaseError::invalid(format!(
                    "{error}; initialization cleanup failed: {cleanup}"
                )),
            });
        }

        {
            let mut guard = context.lock().unwrap_or_else(|error| error.into_inner());
            guard.record.ready = true;
        }
        write_context_record(&context)?;
        sync_directory(&dir)?;
        worker.store().emit("Database initialization prepared");
        if cancelled() {
            let _ = worker.stop_initialization_jobs(&context, &|| false);
            return Err(DatabaseError::Cancelled);
        }
        let final_dir = self.store().root().join("instances").join(&instance.id);
        let publish =
            self.store()
                .with_state_transaction_cancellable(cancelled, |latest: &mut State| {
                    if latest.packages.get(&instance.package) != Some(&package) {
                        return Err(crate::state::StateError::invalid(
                            "database software changed during initialization",
                        ));
                    }
                    self.check_new_instance(latest, &instance)
                        .map_err(crate::state::StateError::from)?;
                    if final_dir.exists() {
                        return Err(crate::state::StateError::invalid(
                            "instance data already exists; choose another ID",
                        ));
                    }
                    std::fs::rename(&data_dir, &final_dir)?;
                    let from = data_dir.clone();
                    let to = final_dir.clone();
                    let undo = Box::new(move || -> Result<(), crate::state::StateError> {
                        if fs::symlink_metadata(&from).is_ok() {
                            return Err(crate::state::StateError::invalid(
                                "initialization rollback path is occupied; data was retained",
                            ));
                        }
                        std::fs::rename(&to, &from)?;
                        Ok(())
                    });
                    sync_directory(&dir)?;
                    sync_directory(&self.store().root().join("instances"))?;
                    self.store().emit("Database initialization data published");
                    latest
                        .instances
                        .insert(instance.id.clone(), instance.clone());
                    if let Some(fault) = &self.publish_fault {
                        // Run the undo before the document is written so the test
                        // observes the same no-partial-instance guarantee a real
                        // write failure produces.
                        if let Err(error) = fault() {
                            undo()?;
                            return Err(error);
                        }
                        return Ok(Tx::new(()).with_commit(|| {}));
                    }
                    Ok(Tx::new(()).with_undo(undo).with_commit({
                        let store = worker.store().clone();
                        move || store.emit("Database initialization committed")
                    }))
                });
        publish.map_err(|error| match error {
            crate::state::StateError::Cancelled => DatabaseError::Cancelled,
            other => DatabaseError::State(other),
        })?;

        sync_directory(self.store().root())?;
        fs::remove_file(&record_path)?;
        sync_directory(&dir)?;
        Ok(())
    }

    fn prepare_and_initialize(
        &self,
        instance: &mut Instance,
        state: &State,
        _package: &Package,
        context: &Arc<Mutex<InitContext>>,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        prepare_data_location(self.store(), instance)?;
        {
            let mut guard = context.lock().unwrap_or_else(|error| error.into_inner());
            guard.record.instance = instance.clone();
        }
        write_context_record(context)?;
        self.store()
            .emit(&format!("Initializing database {}", instance.id));
        self.initialize_data(state, instance, context, cancelled)?;
        self.stop_initialization_jobs(context, cancelled)?;
        Ok(())
    }

    fn initialize_data(
        &self,
        state: &State,
        instance: &Instance,
        context: &Arc<Mutex<InitContext>>,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        verify_data_location(self.store(), instance)?;
        let data_dir = context
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_dir
            .clone();
        if !instance.data_directory.is_empty() && instance.kind == "redis" {
            create_private_dir_all(&database_data_directory(
                self.store().root(),
                instance,
                Some(&data_dir),
            ))?;
        }
        let password = format!("{}{}", random_id(), random_id());
        atomic_write(
            &data_dir.join("password"),
            password.as_bytes(),
            PRIVATE_FILE,
        )?;

        match instance.kind.as_str() {
            "postgresql" => {
                let binary = self.software().binary(state, &instance.package, "initdb")?;
                let target =
                    database_data_directory(self.store().root(), instance, Some(&data_dir));
                self.run_initialization_tool(
                    context,
                    &binary,
                    &[
                        "-D".to_string(),
                        target.display().to_string(),
                        "-U".to_string(),
                        "devmanager".to_string(),
                        "--locale=C".to_string(),
                        "--encoding=UTF8".to_string(),
                        "--auth-host=scram-sha-256".to_string(),
                        "--auth-local=scram-sha-256".to_string(),
                        format!("--pwfile={}", data_dir.join("password").display()),
                    ],
                    cancelled,
                )?;
            }
            "mysql" | "mongodb" => {
                self.create_other(state, instance, context, &password, cancelled)?;
            }
            "redis" => {
                self.software()
                    .binary(state, &instance.package, "redis-server")?;
                let target =
                    database_data_directory(self.store().root(), instance, Some(&data_dir));
                let mut config = format!(
                    "bind 127.0.0.1\nport {}\ndir {:?}\nrequirepass {}\ndaemonize no\n",
                    instance.port,
                    target.to_string_lossy(),
                    password
                );
                if instance.persistent {
                    config.push_str("appendonly yes\n");
                } else {
                    config.push_str("save \"\"\nappendonly no\n");
                }
                atomic_write(
                    &data_dir.join("valkey.conf"),
                    config.as_bytes(),
                    PRIVATE_FILE,
                )?;
            }
            _ => return Err(DatabaseError::invalid("unsupported database engine")),
        }
        Ok(())
    }

    fn create_other(
        &self,
        state: &State,
        instance: &Instance,
        context: &Arc<Mutex<InitContext>>,
        password: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        let data_dir = context
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_dir
            .clone();
        let target = database_data_directory(self.store().root(), instance, Some(&data_dir));
        create_private_dir_all(&target)?;
        let job = format!("init-{}-server", initialization_id(context));
        match instance.kind.as_str() {
            "mongodb" => {
                self.software().binary(state, &instance.package, "mongod")?;
                self.start_instance_job(
                    state, instance, &job, &data_dir, &target, &data_dir, cancelled,
                )?;
                self.mongo()
                    .create_root_user(instance.port, "devmanager", password)?;
            }
            "mysql" => {
                let binary = self.software().binary(state, &instance.package, "mysqld")?;
                let package = &state.packages[&instance.package];
                let basedir = self
                    .software()
                    .store()
                    .package_directory(package)
                    .display()
                    .to_string();
                self.run_initialization_tool(
                    context,
                    &binary,
                    &[
                        "--no-defaults".to_string(),
                        "--initialize-insecure".to_string(),
                        format!("--basedir={basedir}"),
                        format!("--datadir={}", target.display()),
                    ],
                    cancelled,
                )?;
                self.mysql_config(&data_dir, instance.port, "")?;
                self.start_instance_job(
                    state, instance, &job, &data_dir, &target, &data_dir, cancelled,
                )?;
                let mysql = self.software().binary(state, &instance.package, "mysql")?;
                let sql = format!(
                    "ALTER USER 'root'@'localhost' IDENTIFIED BY {}",
                    sql_literal(password)
                );
                let sql_path = data_dir.join("init-user.sql");
                atomic_write(&sql_path, sql.as_bytes(), PRIVATE_FILE)?;
                let config = data_dir.join("client.cnf");
                let result = self.engine().run_command(
                    &mysql.display().to_string(),
                    &[
                        &format!("--defaults-file={}", config.display()),
                        "--batch",
                        "--skip-column-names",
                        &format!("--execute=source {}", sql_path.display()),
                    ],
                    cancelled,
                );
                let _ = fs::remove_file(&sql_path);
                result.map_err(|error| {
                    DatabaseError::invalid(format!("MySQL command failed: {error}"))
                })?;
                self.mysql_config(&data_dir, instance.port, password)?;
            }
            _ => return Err(DatabaseError::invalid("unsupported engine")),
        }
        Ok(())
    }

    fn mysql_config(&self, dir: &Path, port: i64, password: &str) -> Result<(), DatabaseError> {
        let contents = format!(
            "[client]\nuser=root\npassword=\"{}\"\nhost=127.0.0.1\nport={}\nprotocol=tcp\n",
            password, port
        );
        atomic_write(&dir.join("client.cnf"), contents.as_bytes(), PRIVATE_FILE)?;
        Ok(())
    }

    fn run_initialization_tool(
        &self,
        context: &Arc<Mutex<InitContext>>,
        binary: &Path,
        args: &[String],
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        let initialization_id = initialization_id(context);
        let job = format!("init-{initialization_id}-tool");
        let working_dir = context
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_dir
            .clone();
        let mut argv = Vec::with_capacity(args.len() + 1);
        argv.push(binary.display().to_string());
        argv.extend(args.iter().cloned());
        let spec = JobSpec {
            id: job.clone(),
            args: argv,
            working_dir: working_dir.display().to_string(),
            env: BTreeMap::from([
                ("LANG".to_string(), "C".to_string()),
                ("LC_ALL".to_string(), "C".to_string()),
            ]),
            restart: false,
        };
        self.engine().start_job(&spec, cancelled)?;
        let owned = context
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record
            .jobs
            .contains_key(&job);
        if !owned {
            return Err(DatabaseError::invalid(
                "initialization tool is not recorded as owned",
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(5 * 60);
        loop {
            if cancelled() {
                return Err(DatabaseError::Cancelled);
            }
            let Some(state) = self.engine().job_state(&job)? else {
                return Err(DatabaseError::invalid(
                    "initialization tool job is no longer registered",
                ));
            };
            if !state.running
                && state.pid == 0
                && state.runs > 0
                && let Some(code) = state.last_exit_status
            {
                if code != 0 {
                    return Err(DatabaseError::invalid(format!(
                        "initialization tool failed with exit code {code}; read its managed log"
                    )));
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(DatabaseError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stop every initialization job this context recorded. Ownership is
    /// proven with the stored plist digest before anything is booted out.
    pub(crate) fn stop_initialization_jobs(
        &self,
        context: &Arc<Mutex<InitContext>>,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        let record = context
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record
            .clone();
        for job in record.jobs.keys() {
            let path = self.engine().job_path(job);
            let bytes = match read_regular_file(
                path.parent().unwrap_or_else(|| Path::new(".")),
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default(),
                1 << 20,
            ) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    verify_shutdown_target(self.engine(), &self.engine().target(job), cancelled)?;
                    continue;
                }
                Err(error) => return Err(DatabaseError::Io(error)),
            };
            if digest_bytes(&bytes) != record.jobs[job] {
                return Err(DatabaseError::invalid(
                    "initialization job configuration changed; keep its data for inspection",
                ));
            }
            self.engine().stop_job(job, cancelled)?;
            verify_shutdown_target(self.engine(), &self.engine().target(job), cancelled)?;
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(DatabaseError::Io(error)),
            }
        }
        Ok(())
    }

    fn check_new_instance(&self, state: &State, instance: &Instance) -> Result<(), DatabaseError> {
        super::validate_data_location(self.store(), state, instance)?;
        if state.instances.contains_key(&instance.id) {
            return Err(DatabaseError::invalid("instance already exists"));
        }
        if state
            .instances
            .values()
            .any(|other| other.port == instance.port)
        {
            return Err(DatabaseError::invalid("instance port already assigned"));
        }
        if !state.proxy_rules.is_empty() && state.proxy_listener_port == instance.port {
            return Err(DatabaseError::invalid(
                "database port is the HTTPS proxy's listener port",
            ));
        }
        match fs::symlink_metadata(self.store().root().join("instances").join(&instance.id)) {
            Ok(_) => {
                return Err(DatabaseError::invalid(
                    "instance data already exists; choose another ID",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(DatabaseError::Io(error)),
        }
        super::ports::free_database_port(self.binder(), instance.port)
    }
}

fn initialization_id(context: &Arc<Mutex<InitContext>>) -> String {
    context
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .record
        .id
        .clone()
}

fn write_context_record(context: &Arc<Mutex<InitContext>>) -> Result<(), DatabaseError> {
    let (path, record) = {
        let guard = context.lock().unwrap_or_else(|error| error.into_inner());
        (guard.record_path.clone(), guard.record.clone())
    };
    let mut bytes = serde_json::to_vec_pretty(&record)?;
    bytes.push(b'\n');
    atomic_write(&path, &bytes, PRIVATE_FILE)?;
    Ok(())
}

fn record_initialization_job(
    context: &Arc<Mutex<InitContext>>,
    job: &str,
    path: &Path,
) -> Result<(), DatabaseError> {
    let mut guard = context.lock().unwrap_or_else(|error| error.into_inner());
    if job != format!("init-{}-tool", guard.record.id)
        && job != format!("init-{}-server", guard.record.id)
    {
        return Err(DatabaseError::invalid("unexpected initialization job"));
    }
    let bytes = read_regular_file(
        path.parent().unwrap_or_else(|| Path::new(".")),
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
        1 << 20,
    )?;
    guard
        .record
        .jobs
        .insert(job.to_string(), digest_bytes(&bytes));
    let path = guard.record_path.clone();
    let mut bytes = serde_json::to_vec_pretty(&guard.record)?;
    bytes.push(b'\n');
    atomic_write(&path, &bytes, PRIVATE_FILE)?;
    sync_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
    Ok(())
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

/// An interrupted initialization discovered at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceInitializationInfo {
    pub id: String,
    pub instance_id: String,
    pub kind: String,
    pub package: String,
    pub path: PathBuf,
    pub status: String,
    pub detail: String,
    pub revision: String,
    pub port: i64,
    pub busy: bool,
    pub can_resolve: bool,
}

fn initialization_paths(
    root: &Path,
    id: &str,
) -> Result<(PathBuf, PathBuf, PathBuf), DatabaseError> {
    if !crate::model::valid_id(id) {
        return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
    }
    if !root.is_absolute() {
        return Err(DatabaseError::invalid("storage root must be absolute"));
    }
    for name in [
        "instances",
        "jobs",
        "initializations",
        "retained-initializations",
    ] {
        match fs::symlink_metadata(root.join(name)) {
            Ok(info) if !info.is_dir() || info.file_type().is_symlink() => {
                return Err(DatabaseError::invalid(
                    "deletion storage is not a plain directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let dir = root.join("initializations").join(id);
    Ok((dir.clone(), dir.join("record.json"), dir.join("data")))
}

fn read_initialization(
    path: &Path,
    id: &str,
) -> Result<(InitializationRecord, Vec<u8>), DatabaseError> {
    let bytes = read_regular_file(
        path.parent().unwrap_or_else(|| Path::new(".")),
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
        65536,
    )?;
    let record: InitializationRecord = serde_json::from_slice(&bytes)?;
    if record.schema != 1
        || record.id != id
        || !crate::model::valid_id(&record.instance.id)
        || record.identity.is_empty()
    {
        return Err(DatabaseError::invalid(
            "initialization record identity mismatch",
        ));
    }
    for (job, digest) in &record.jobs {
        if (job != &format!("init-{id}-server") && job != &format!("init-{id}-tool"))
            || !crate::fs::valid_digest(digest)
        {
            return Err(DatabaseError::invalid(
                "initialization job ownership is invalid",
            ));
        }
    }
    Ok((record, bytes))
}

fn path_identity(path: &Path) -> Result<Option<String>, DatabaseError> {
    match fs::symlink_metadata(path) {
        Ok(info) => {
            if !info.is_dir() || info.file_type().is_symlink() {
                return Err(DatabaseError::invalid(
                    "initialization data path is not a plain directory",
                ));
            }
            Ok(Some(super::deletion_identity(&info)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn initialization_info(
    root: &Path,
    state: &State,
    record: &InitializationRecord,
    record_bytes: &[u8],
    data: &Path,
) -> Result<InstanceInitializationInfo, DatabaseError> {
    let mut info = InstanceInitializationInfo {
        id: record.id.clone(),
        instance_id: record.instance.id.clone(),
        kind: record.instance.kind.clone(),
        package: record.instance.package.clone(),
        port: record.instance.port,
        path: data.to_path_buf(),
        status: "Incomplete".into(),
        can_resolve: true,
        detail: "Stop any recorded initialization jobs and archive the prepared data. The instance can then be configured again.".into(),
        ..InstanceInitializationInfo::default()
    };
    let stage = path_identity(data)?;
    let canonical = root.join("instances").join(&record.instance.id);
    let current = path_identity(&canonical)?;
    let archive = root.join("retained-initializations").join(&record.id);
    let retained = path_identity(&archive)?;
    let registered = state.instances.get(&record.instance.id);
    if let Some(stage) = &stage
        && stage != &record.identity
    {
        return Err(DatabaseError::invalid(
            "initialization data identity changed",
        ));
    }
    if current.as_deref() == Some(record.identity.as_str()) {
        info.path = canonical.clone();
        if registered.is_some() {
            info.status = "Published".into();
            info.detail =
                "This data is registered. Finish clears only the initialization record; the database remains intact."
                    .into();
        } else {
            info.status = "Uncommitted".into();
            info.detail = "Data moved but registration did not commit. Reconcile moves it back and archives it, keeping all files.".into();
        }
    }
    if stage.is_none() && current.as_deref() != Some(record.identity.as_str()) {
        if retained.as_deref() == Some(record.identity.as_str()) {
            info.path = archive;
            info.status = "Archived".into();
            info.detail = "The prepared data is already archived. Finish clears only the initialization record and keeps the archive.".into();
        } else {
            info.can_resolve = false;
            info.status = "Needs inspection".into();
            info.detail =
                "The recorded data directory is missing. No other data will be moved.".into();
        }
    }
    if !record.instance.data_directory.is_empty() {
        info.detail.push_str(&format!(
            " External database files stay at {}. Only application metadata is archived; this location cannot be reused automatically.",
            super::database_data_directory(root, &record.instance, None).display()
        ));
    }
    #[derive(serde::Serialize)]
    struct Fingerprint<'a> {
        #[serde(rename = "Record")]
        record: String,
        #[serde(rename = "Stage")]
        stage: Option<&'a str>,
        #[serde(rename = "Canonical")]
        canonical: Option<&'a str>,
        #[serde(rename = "Retained")]
        retained: Option<&'a str>,
        #[serde(rename = "Registered")]
        registered: Option<&'a crate::model::Instance>,
        #[serde(rename = "Instance")]
        instance: &'a crate::model::Instance,
    }
    let bytes = serde_json::to_vec(&Fingerprint {
        record: crate::fs::digest_bytes(record_bytes),
        stage: stage.as_deref(),
        canonical: current.as_deref(),
        retained: retained.as_deref(),
        registered,
        instance: &record.instance,
    })?;
    info.revision = crate::fs::digest_bytes(&bytes);
    Ok(info)
}

fn try_owner_lock(path: &Path) -> Result<Option<std::fs::File>, DatabaseError> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(false)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(error.into());
        }
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(DatabaseError::invalid(
            "initialization owner lease is not a regular file",
        ));
    }
    let result = unsafe {
        libc::flock(
            std::os::unix::io::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    if result == 0 {
        return Ok(Some(file));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(None)
    } else {
        Err(error.into())
    }
}

impl Databases {
    /// `PendingInstanceInitializations`: every preparation record left behind.
    pub fn pending_instance_initializations(
        &self,
        cancelled: Cancelled<'_>,
    ) -> Result<Vec<InstanceInitializationInfo>, DatabaseError> {
        initialization_paths(self.store().root(), "inspect")?;
        let root = self.store().root();
        let entries = match fs::read_dir(root.join("initializations")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let state = self.store().read()?;
        let mut out = Vec::new();
        for entry in entries {
            if cancelled() {
                return Err(DatabaseError::Cancelled);
            }
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let (dir, path, data) = initialization_paths(root, &id)?;
            let info = fs::symlink_metadata(&dir)?;
            if !info.is_dir() || info.file_type().is_symlink() {
                return Err(DatabaseError::invalid(
                    "deletion storage is not a plain directory",
                ));
            }
            let (record, bytes) = match read_initialization(&path, &id) {
                Ok(value) => value,
                Err(DatabaseError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(error) => return Err(error),
            };
            let mut info = initialization_info(root, &state, &record, &bytes, &data)?;
            match try_owner_lock(&dir.join("owner.lock")) {
                Ok(Some(_lease)) => {}
                Ok(None) => {
                    info.busy = true;
                    info.can_resolve = false;
                    info.status = "Initializing".into();
                    info.detail =
                        "A caller still owns this initialization. Wait or cancel it in that caller."
                            .into();
                }
                Err(error) => return Err(error),
            }
            out.push(info);
        }
        out.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(out)
    }

    /// `ResolveInstanceInitialization`: stop recorded jobs and archive the
    /// prepared data without ever deleting it. Returns the archive path.
    pub fn resolve_instance_initialization(
        &self,
        id: &str,
        revision: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<String, DatabaseError> {
        let root = self.store().root();
        let (dir, path, data) = initialization_paths(root, id)?;
        let info = fs::symlink_metadata(&dir)?;
        if !info.is_dir() || info.file_type().is_symlink() {
            return Err(DatabaseError::invalid(
                "deletion storage is not a plain directory",
            ));
        }
        let _owner = try_owner_lock(&dir.join("owner.lock"))?.ok_or_else(|| {
            DatabaseError::invalid("initialization is still owned by another caller")
        })?;
        let (record, bytes) = read_initialization(&path, id)?;
        let _instance = self.store().resource_leases_cancellable(
            &[ResourceKey::new("database instance", &record.instance.id)],
            cancelled,
        )?;
        let state = self.store().read()?;
        let current = initialization_info(root, &state, &record, &bytes, &data)?;
        if current.revision != revision || !current.can_resolve {
            return Err(DatabaseError::invalid(
                "initialization changed; refresh its status before reconciling",
            ));
        }
        let context = Arc::new(Mutex::new(InitContext {
            record: record.clone(),
            record_path: path.clone(),
            data_dir: data.clone(),
        }));
        self.stop_initialization_jobs(&context, cancelled)?;
        let archive = Arc::new(Mutex::new(String::new()));
        let archive_hook = archive.clone();
        let store = self.store().clone();
        self.store()
            .with_state_transaction_cancellable(cancelled, |latest| {
                let now = initialization_info(root, latest, &record, &bytes, &data)
                    .map_err(crate::state::StateError::from)?;
                if now.revision != revision {
                    return Err(crate::state::StateError::invalid(
                        "instance changed while stopping initialization jobs; refresh",
                    ));
                }
                if now.status == "Published" || now.status == "Archived" {
                    if now.status == "Archived" {
                        *archive_hook.lock().unwrap() = now.path.display().to_string();
                    }
                    return Ok(Tx::new(()));
                }
                let mut from = root.join("initializations").join(id).join("data");
                if now.status == "Uncommitted" {
                    verify_shutdown_target(
                        self.engine(),
                        &self.engine().target(&format!("db-{}", record.instance.id)),
                        cancelled,
                    )
                    .map_err(crate::state::StateError::from)?;
                    from = root.join("instances").join(&record.instance.id);
                }
                create_private_dir_all(&root.join("retained-initializations"))?;
                let to = root.join("retained-initializations").join(id);
                match fs::symlink_metadata(&to) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => {
                        return Err(crate::state::StateError::invalid(
                            "retained initialization path is occupied",
                        ));
                    }
                }
                fs::rename(&from, &to)?;
                *archive_hook.lock().unwrap() = to.display().to_string();
                store.emit("Database initialization data archived");
                let undo_from = from.clone();
                let undo_to = to.clone();
                let undo = Box::new(move || -> Result<(), crate::state::StateError> {
                    fs::rename(&undo_to, &undo_from)?;
                    Ok(())
                });
                sync_directory(&root.join("retained-initializations"))?;
                if let Some(parent) = from.parent() {
                    sync_directory(parent)?;
                }
                Ok(Tx::new(()).with_undo(undo).with_commit({
                    let store = store.clone();
                    move || store.emit("Database initialization archive committed")
                }))
            })?;
        fs::remove_file(&path)?;
        sync_directory(&dir)?;
        let archive = archive.lock().unwrap().clone();
        Ok(archive)
    }
}
