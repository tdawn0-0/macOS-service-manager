//! Database instance foundation, ported from the Go `devplatform` database
//! files: `database_locations.go`, `ports.go`, `database.go`,
//! `instance_initialization.go`, `instance_deletion.go`,
//! `connection_profiles.go` and `connections.go`.
//!
//! The invariants are the Go ones:
//!
//! * Instance creation stages data under `initializations/<random>/data` and
//!   publishes it with a same-volume `rename` to `instances/<id>` inside the
//!   state transaction. A failed preparation never registers an instance and
//!   leaves the staging directory for inspection.
//! * Removing an instance only unregisters it. Internal data stays under
//!   `instances/<id>`; an external `DataDirectory` is never touched.
//!   Permanent erasure is a separate, explicitly confirmed operation for
//!   internal data only; external data is retained verbatim.
//! * Database passwords live in `instances/<id>/password` (0600) and, for
//!   MySQL, `instances/<id>/client.cnf` (0600). They never enter state, a
//!   process argument, a profile or an operation record.
//! * Every lifecycle action holds the `database instance:<id>` resource lease
//!   and runs the job through the injected [`CommandRunner`]; tests never
//!   execute a real database binary or `launchctl`.
//!
//! ```no_run
//! use devcore::databases::{Databases, InstanceAction};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let databases = Databases::new(devcore::state::Store::new(devcore::state::default_root()));
//! let cancelled = || false;
//! databases.instance_action("blog", InstanceAction::Start, &cancelled)?;
//! # Ok(())
//! # }
//! ```

pub mod actions;
pub mod backups;
pub mod data;
pub mod database_work;
pub mod deletion;
pub mod init;
pub mod inventory;
pub mod lifecycle;
pub mod locations;
pub mod mongo;
pub mod ports;
pub mod profiles;
pub mod redis;
pub mod telemetry;
pub mod upgrade;

#[cfg(test)]
mod tests;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::{CommandRunner, Engine, EngineError, SystemCommandRunner};
use crate::health::{HealthProbe, NetworkProbe};
use crate::launchd::CommandError;
use crate::software::{Software, SoftwareError};

use crate::state::{StateError, Store};

pub use actions::DatabaseActions;
pub use backups::BackupFile;
pub use database_work::{DatabaseWorkInfo, DatabaseWorkRecord, install_database_work_guard};
pub use deletion::{InstanceDeletionInfo, InstanceDeletionPreview};
pub use init::InstanceInitializationInfo;
pub use inventory::InstanceCatalog;
pub use lifecycle::{InstanceAction, verify_shutdown_job, verify_shutdown_target};
pub use locations::{
    DATA_OWNER_FILE, config_directory, database_data_directory, deletion_identity, path_within,
    plain_path, prepare_data_location, validate_data_location, verify_data_location,
};
pub use mongo::{MongoAdmin, WireMongoAdmin};
pub use ports::{
    AvailablePorts, ListeningPort, PortBindFailure, PortBinder, PortInspection, PortListener,
    PortUnavailableError, SystemPortBinder,
};
pub use profiles::ConnectionInfo;
pub use redis::{RedisAdmin, WireRedisAdmin};
pub use telemetry::InstanceTelemetry;

/// A cancellation predicate, matching the engine/software/shutdown seams.
pub type Cancelled<'a> = &'a dyn Fn() -> bool;

/// A fault injected after the staged data is published and before the state
/// document is written. Tests use it to prove the transaction undo restores
/// the staging path; production never installs one.
pub(crate) type PublishFault = Arc<dyn Fn() -> Result<(), StateError> + Send + Sync>;

/// Everything the database foundation can fail with. Message fragments match
/// the Go package so the UI keeps its current wording.
#[derive(Debug)]
pub enum DatabaseError {
    State(StateError),
    Engine(EngineError),
    Software(SoftwareError),
    Io(io::Error),
    Json(serde_json::Error),
    Invalid(String),
    PortUnavailable(PortUnavailableError),
    Cancelled,
}

impl DatabaseError {
    pub fn invalid(message: impl Into<String>) -> Self {
        DatabaseError::Invalid(message.into())
    }

    /// Whether another operation owns the instance or package.
    pub fn is_busy(&self) -> bool {
        match self {
            DatabaseError::State(error) => error.is_busy(),
            DatabaseError::Engine(EngineError::State(error)) => error.is_busy(),
            DatabaseError::Software(error) => error.is_busy(),
            _ => false,
        }
    }
}

impl std::fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatabaseError::State(error) => error.fmt(f),
            DatabaseError::Engine(error) => error.fmt(f),
            DatabaseError::Software(error) => error.fmt(f),
            DatabaseError::Io(error) => error.fmt(f),
            DatabaseError::Json(error) => error.fmt(f),
            DatabaseError::Invalid(message) => f.write_str(message),
            DatabaseError::PortUnavailable(error) => error.fmt(f),
            DatabaseError::Cancelled => f.write_str("context canceled"),
        }
    }
}

impl std::error::Error for DatabaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DatabaseError::State(error) => Some(error),
            DatabaseError::Engine(error) => Some(error),
            DatabaseError::Software(error) => Some(error),
            DatabaseError::Io(error) => Some(error),
            DatabaseError::Json(error) => Some(error),
            DatabaseError::PortUnavailable(error) => Some(error),
            DatabaseError::Invalid(_) | DatabaseError::Cancelled => None,
        }
    }
}

impl From<StateError> for DatabaseError {
    fn from(error: StateError) -> Self {
        match error {
            StateError::Cancelled => DatabaseError::Cancelled,
            other => DatabaseError::State(other),
        }
    }
}

impl From<EngineError> for DatabaseError {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::Cancelled => DatabaseError::Cancelled,
            other => DatabaseError::Engine(other),
        }
    }
}

impl From<SoftwareError> for DatabaseError {
    fn from(error: SoftwareError) -> Self {
        DatabaseError::Software(error)
    }
}

impl From<io::Error> for DatabaseError {
    fn from(error: io::Error) -> Self {
        DatabaseError::Io(error)
    }
}

impl From<serde_json::Error> for DatabaseError {
    fn from(error: serde_json::Error) -> Self {
        DatabaseError::Json(error)
    }
}

impl From<CommandError> for DatabaseError {
    fn from(error: CommandError) -> Self {
        DatabaseError::Engine(EngineError::Command(error))
    }
}

impl From<DatabaseError> for StateError {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::State(error) => error,
            DatabaseError::Cancelled => StateError::Cancelled,
            DatabaseError::Io(error) => StateError::Io(error),
            other => StateError::invalid(other.to_string()),
        }
    }
}

/// The database manager for one development home.
///
/// It owns a [`Store`], a launchd [`Engine`] sharing the same command runner,
/// and the [`Software`] facade used to resolve executable paths. The port
/// binder and health probe are injected exactly like the Go test fields
/// `listenLoopback`/`dialLoopback` and `processProbe`.
#[derive(Clone)]
pub struct Databases {
    store: Store,
    runner: Arc<dyn CommandRunner>,
    engine: Engine,
    software: Software,
    binder: Arc<dyn PortBinder>,
    probe: Arc<dyn HealthProbe>,
    mongo: Arc<dyn MongoAdmin>,
    redis: Arc<dyn RedisAdmin>,
    publish_fault: Option<PublishFault>,
}

impl std::fmt::Debug for Databases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Databases")
            .field("root", &self.store.root())
            .finish()
    }
}

impl Databases {
    /// Production seams: real commands, real loopback binds and TCP probes.
    pub fn new(store: Store) -> Self {
        Databases::with_runner(store, Arc::new(SystemCommandRunner::new()))
    }

    /// Build the manager around an injected command runner. The runner serves
    /// both the launchd engine and the initialisation tools.
    pub fn with_runner(store: Store, runner: Arc<dyn CommandRunner>) -> Self {
        // Database-work records reserve an instance across processes; the
        // persistence layer calls this check when it acquires the instance
        // lease, exactly where Go calls requireDatabaseWorkIdle.
        let store = install_database_work_guard(store);
        let engine = Engine::new(store.root(), runner.clone());
        let software = Software::new(store.clone()).with_runner(runner.clone());
        Databases {
            store,
            runner,
            engine,
            software,
            binder: Arc::new(SystemPortBinder),
            probe: Arc::new(NetworkProbe),
            mongo: Arc::new(WireMongoAdmin),
            redis: Arc::new(WireRedisAdmin),
            publish_fault: None,
        }
    }

    pub fn with_binder(mut self, binder: Arc<dyn PortBinder>) -> Self {
        self.binder = binder;
        self
    }

    pub fn with_probe(mut self, probe: Arc<dyn HealthProbe>) -> Self {
        self.probe = probe;
        self
    }

    /// Override the MongoDB administrative seam (tests never open a socket).
    pub fn with_mongo_admin(mut self, mongo: Arc<dyn MongoAdmin>) -> Self {
        self.mongo = mongo;
        self
    }

    /// Override the Redis RESP seam (tests never open a socket).
    pub fn with_redis_admin(mut self, redis: Arc<dyn RedisAdmin>) -> Self {
        self.redis = redis;
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn software(&self) -> &Software {
        &self.software
    }

    pub(crate) fn binder(&self) -> &dyn PortBinder {
        self.binder.as_ref()
    }

    pub(crate) fn binder_arc(&self) -> Arc<dyn PortBinder> {
        self.binder.clone()
    }

    pub(crate) fn probe(&self) -> &dyn HealthProbe {
        self.probe.as_ref()
    }

    pub(crate) fn probe_arc(&self) -> Arc<dyn HealthProbe> {
        self.probe.clone()
    }

    pub(crate) fn mongo(&self) -> &dyn MongoAdmin {
        self.mongo.as_ref()
    }

    pub(crate) fn mongo_arc(&self) -> Arc<dyn MongoAdmin> {
        self.mongo.clone()
    }

    pub(crate) fn redis(&self) -> &dyn RedisAdmin {
        self.redis.as_ref()
    }

    pub(crate) fn redis_arc(&self) -> Arc<dyn RedisAdmin> {
        self.redis.clone()
    }

    pub(crate) fn runner(&self) -> Arc<dyn CommandRunner> {
        self.runner.clone()
    }

    pub(crate) fn publish_fault_clone(&self) -> Option<PublishFault> {
        self.publish_fault.clone()
    }

    #[cfg(test)]
    pub(crate) fn with_publish_fault(mut self, fault: PublishFault) -> Self {
        self.publish_fault = Some(fault);
        self
    }

    /// Create the workspace directories this slice needs: the store's base
    /// directories plus the database staging areas.
    pub fn init(&self) -> Result<(), DatabaseError> {
        self.store.init()?;
        for name in [
            "initializations",
            "retained-initializations",
            "deletions",
            "database-work",
        ] {
            crate::fs::create_private_dir_all(&self.store.root().join(name))?;
        }
        Ok(())
    }

    /// The instance record from state, or `instance not found`.
    pub fn instance(&self, id: &str) -> Result<crate::model::Instance, DatabaseError> {
        let state = self.store.read()?;
        state
            .instances
            .get(id)
            .cloned()
            .ok_or_else(|| DatabaseError::invalid("instance not found"))
    }

    /// The on-disk data directory for an instance in its steady-state layout.
    pub fn data_directory(&self, instance: &crate::model::Instance) -> PathBuf {
        database_data_directory(self.store.root(), instance, None)
    }

    /// The instance configuration directory (`valkey.conf`, `client.cnf`).
    pub fn configuration_directory(&self, instance: &crate::model::Instance) -> PathBuf {
        config_directory(self.store.root(), instance, None)
    }
}
