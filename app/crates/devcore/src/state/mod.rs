//! File-locked, transactional state persistence, ported from the Go
//! `devplatform` state layer.
//!
//! Exactly one JSON document (`state.json`, schema 2) and one lock file
//! (`state.lock`) hold the workspace. Reads are lock-free; every mutation runs
//! inside [`Store::with_state_transaction`], which takes a real `flock(2)`
//! exclusive lock, re-reads the document, and publishes a new one atomically
//! (temp file + rename). The GUI and the CLI are separate processes, so this
//! is a file lock, never an in-process mutex.
//!
//! Schema policy: a document whose `schemaVersion` is not [`model::SCHEMA`] is
//! rejected before any other field is decoded and is never rewritten. Phase 2
//! ships no migrator; a future structural change bumps the version and adds a
//! one-way migration that backs up the original bytes first.
//!
//! ```no_run
//! use devcore::state::{Store, StateError};
//!
//! # fn main() -> Result<(), StateError> {
//! let store = Store::new(devcore::state::default_root());
//! let state = store.read()?;
//! println!("{} packages", state.packages.len());
//! store.with_state(|state| {
//!     state.proxy_rules_revision += 1;
//!     Ok(())
//! })?;
//! # Ok(())
//! # }
//! ```

mod error;
mod lock;
mod package_locks;
mod paths;
mod resource_locks;

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::fs::{PRIVATE_FILE, create_private_dir_all};
use crate::model::{Package, SCHEMA, State, decode_state_strict};
use lock::{FileLease, try_flock};

pub use error::{PackageBusyError, ResourceBusyError, StateError};
pub use package_locks::PackageLease;
pub use paths::{
    DEFAULT_BREW_CELLAR, DEV_HOME_ENV, JAVA_OWNER_FILE, cache_directory, default_root,
    default_storage_root, download_directory, home_from_environment, is_user_storage,
    java_directory, package_directory,
};
pub use resource_locks::{ResourceKey, ResourceLease};

/// The delay between lock attempts, matching the Go loops.
const LOCK_POLL: Duration = Duration::from_millis(100);

/// A rollback action run while the state lock is still held when publication
/// fails or the operation is cancelled after its side effects.
pub type Undo = Box<dyn FnOnce() -> Result<(), StateError>>;

/// A commit action run only after the new state is durably written.
pub type Commit = Box<dyn FnOnce()>;

/// The value a transaction closure returns, plus its optional hooks.
pub struct Tx<R> {
    value: R,
    undo: Option<Undo>,
    commit: Option<Commit>,
}

impl<R> Tx<R> {
    /// A transaction with no hooks.
    pub fn new(value: R) -> Self {
        Tx {
            value,
            undo: None,
            commit: None,
        }
    }

    /// Run `undo` if the state write fails or the operation is cancelled after
    /// this closure returns.
    pub fn with_undo(mut self, undo: impl FnOnce() -> Result<(), StateError> + 'static) -> Self {
        self.undo = Some(Box::new(undo));
        self
    }

    /// Run `commit` after the new state is durable.
    pub fn with_commit(mut self, commit: impl FnOnce() + 'static) -> Self {
        self.commit = Some(Box::new(commit));
        self
    }
}

/// Whether a resource-holder check rejects the lease. Installed by the
/// database-work slice; the persistence layer only calls it in the same place
/// the Go code does (`database instance` leases).
pub type DatabaseWorkCheck = Arc<dyn Fn(&str) -> Result<(), StateError> + Send + Sync>;

/// An open development-home workspace.
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    home: Option<PathBuf>,
    brew_cellar: Option<PathBuf>,
    event: Arc<dyn Fn(&str) + Send + Sync>,
    database_work_check: Option<DatabaseWorkCheck>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("root", &self.root).finish()
    }
}

impl Store {
    /// Open the workspace at `root`. The user's home is read from `$HOME`
    /// when it is needed for the user-storage locations.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store {
            root: root.into(),
            home: None,
            brew_cellar: None,
            event: Arc::new(|_| {}),
            database_work_check: None,
        }
    }

    /// Override the user's home directory (tests and CLI environments).
    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    /// Override the Homebrew Cellar used for formula package directories.
    pub fn with_brew_cellar(mut self, cellar: impl Into<PathBuf>) -> Self {
        self.brew_cellar = Some(cellar.into());
        self
    }

    /// Receive the same progress text the Go `Event` sink gets.
    pub fn with_event(mut self, event: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.event = Arc::new(event);
        self
    }

    /// Install the database-work idle check used by `database instance`
    /// resource leases (the journal slice provides the real implementation).
    pub fn with_database_work_check(mut self, check: DatabaseWorkCheck) -> Self {
        self.database_work_check = Some(check);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The effective home directory for storage decisions.
    pub fn home(&self) -> Result<PathBuf, StateError> {
        if let Some(home) = &self.home {
            return Ok(home.clone());
        }
        let home = home_from_environment();
        if home.as_os_str().is_empty() {
            return Err(StateError::invalid(
                "could not determine the user's home folder",
            ));
        }
        Ok(home)
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join("state.lock")
    }

    /// Create the workspace directories the Go `init()` creates.
    pub fn init(&self) -> Result<(), StateError> {
        if !self.root.is_absolute() {
            return Err(StateError::invalid("storage root must be absolute"));
        }
        for directory in [
            "",
            "packages",
            "instances",
            "jobs",
            "logs",
            "proxy",
            "backups",
            "integrity",
            "package-locks",
        ] {
            create_private_dir_all(&self.root.join(directory))?;
        }
        create_private_dir_all(&self.download_directory())?;
        Ok(())
    }

    pub fn uses_user_storage(&self) -> bool {
        self.home()
            .map(|home| is_user_storage(&self.root, &home))
            .unwrap_or(false)
    }

    pub fn cache_directory(&self) -> PathBuf {
        let home = self.home().unwrap_or_default();
        cache_directory(&self.root, &home)
    }

    pub fn download_directory(&self) -> PathBuf {
        let home = self.home().unwrap_or_default();
        download_directory(&self.root, &home)
    }

    pub fn java_directory(&self) -> PathBuf {
        let home = self.home().unwrap_or_default();
        java_directory(&self.root, &home)
    }

    /// The install directory for a package, including the Java user-level
    /// bundle and Homebrew keg layouts.
    pub fn package_directory(&self, package: &Package) -> PathBuf {
        let home = self.home().unwrap_or_default();
        package_directory(&self.root, &home, self.brew_cellar.as_deref(), package)
    }

    /// Read the state document without taking a lock. A missing document is a
    /// fresh workspace; an unsupported schema version is rejected without
    /// touching the bytes.
    pub fn read(&self) -> Result<State, StateError> {
        let bytes = match std::fs::read(self.state_path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(State::default());
            }
            Err(error) => return Err(error.into()),
        };
        let header: Header = serde_json::from_slice(&bytes).map_err(StateError::ReadJson)?;
        if header.schema_version != SCHEMA {
            return Err(StateError::UnsupportedSchema {
                found: header.schema_version,
                expected: SCHEMA,
            });
        }
        decode_state_strict(&bytes)
    }

    /// Write a complete state document under the exclusive state lock.
    pub fn write_state(&self, state: &State) -> Result<(), StateError> {
        self.init()?;
        let _lease = self.acquire_state_lock(&|| false)?;
        self.write_state_locked(state)
    }

    /// Run a mutation inside a state transaction.
    pub fn with_state<R>(
        &self,
        f: impl FnOnce(&mut State) -> Result<R, StateError>,
    ) -> Result<R, StateError> {
        self.with_state_transaction(|state| f(state).map(Tx::new))
    }

    /// Run a mutation with rollback/commit hooks while holding the state lock.
    pub fn with_state_transaction<R>(
        &self,
        f: impl FnOnce(&mut State) -> Result<Tx<R>, StateError>,
    ) -> Result<R, StateError> {
        self.with_state_transaction_cancellable(&|| false, f)
    }

    /// Like [`Store::with_state_transaction`], but checks `cancelled` while
    /// waiting for the lock and before publishing.
    pub fn with_state_transaction_cancellable<R>(
        &self,
        cancelled: &dyn Fn() -> bool,
        f: impl FnOnce(&mut State) -> Result<Tx<R>, StateError>,
    ) -> Result<R, StateError> {
        self.init()?;
        let _lease = self.acquire_state_lock(cancelled)?;
        if cancelled() {
            return Err(StateError::Cancelled);
        }
        let mut state = self.read()?;
        let Tx {
            value,
            undo,
            commit,
        } = f(&mut state)?;
        let mut undo = undo;
        if cancelled() {
            return Err(apply_undo(StateError::Cancelled, undo.take()));
        }
        match self.write_state_locked(&state) {
            Ok(()) => {
                if let Some(commit) = commit {
                    commit();
                }
                Ok(value)
            }
            Err(error) => Err(apply_undo(error, undo.take())),
        }
    }

    fn write_state_locked(&self, state: &State) -> Result<(), StateError> {
        let mut bytes = serde_json::to_vec_pretty(state)?;
        bytes.push(b'\n');
        crate::fs::atomic_write(&self.state_path(), &bytes, PRIVATE_FILE)?;
        Ok(())
    }

    fn acquire_state_lock(&self, cancelled: &dyn Fn() -> bool) -> Result<FileLease, StateError> {
        if !self.root.is_absolute() {
            return Err(StateError::invalid("storage root must be absolute"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(PRIVATE_FILE)
            .open(self.lock_path())?;
        let mut announced = false;
        loop {
            if cancelled() {
                return Err(StateError::Cancelled);
            }
            if try_flock(&file, libc::LOCK_EX)? {
                if cancelled() {
                    return Err(StateError::Cancelled);
                }
                return Ok(FileLease::new(file));
            }
            if !announced {
                self.emit("Waiting for another workspace change");
                announced = true;
            }
            std::thread::sleep(LOCK_POLL);
        }
    }

    pub(crate) fn emit(&self, message: &str) {
        (self.event)(message);
    }
}

fn apply_undo(error: StateError, undo: Option<Undo>) -> StateError {
    match undo {
        Some(undo) => match undo() {
            Ok(()) => error,
            Err(undo_error) => {
                StateError::invalid(format!("{error}; rollback failed: {undo_error}"))
            }
        },
        None => error,
    }
}

#[derive(Deserialize)]
struct Header {
    #[serde(default, rename = "schemaVersion")]
    schema_version: i64,
}

#[cfg(test)]
mod tests;
