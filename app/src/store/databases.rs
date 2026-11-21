//! The databases workspace's state: the instances read from the development
//! home, the one shown, the preparation draft, its port preflight, each
//! instance's catalog, backups, connection profiles, the deletion preview and
//! the unfinished work lists. Every provider call does disk, process or
//! network work, so each one runs on a blocking thread and writes the store
//! when it answers; render code reads the store and never calls the provider
//! itself.
//!
//! The interaction contract is the Go one (`view_databases.go`,
//! `view_instance_deletion.go`, `view_database_work.go`,
//! `view_database_connections.go`): operations show a progress line and a
//! cancel while they run, a finished one keeps its result until dismissed,
//! and permanent data deletion needs the instance ID typed again. Removing an
//! instance never deletes data: internal data stays under `instances/<id>`,
//! an external data directory is never touched, and the preview refuses it.
//!
//! Tests install a provider of their own before the surface starts with
//! [`use_fake_provider`] and [`install_provider`]; production builds the
//! devcore [`Databases`] facade on first use.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use devcore::{
    databases::{
        BackupFile, ConnectionInfo, DatabaseWorkInfo, Databases, InstanceAction, InstanceCatalog,
        InstanceDeletionInfo, InstanceDeletionPreview, InstanceInitializationInfo, PortInspection,
    },
    health::read_job_state,
    model::{ConnectionProfile, Instance, Package, State},
    state::Store,
};
use nsh_base::alert_dialog;
use nsh_core::prelude::*;

use crate::store::software::SoftwareKind;

/// A cancellation flag an operation reads while it runs.
pub type CancelToken = Arc<AtomicBool>;

/// Where the provider sends the progress text devcore emits while it works.
pub type ProgressSink = Arc<dyn Fn(String) + Send + Sync>;

/// One read of the development home: the registered instances, the installed
/// packages they can use, their saved profiles and their launchd state.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub instances: Vec<Instance>,
    pub packages: BTreeMap<String, Package>,
    pub profiles: BTreeMap<String, ConnectionProfile>,
    /// `Running`, `Stopped` or `Unknown`, by instance id.
    pub statuses: BTreeMap<String, String>,
    /// The newer installed patches each instance may move to, newest first.
    pub upgrades: BTreeMap<String, Vec<String>>,
}

/// The database operations the screens need. The production implementation
/// wraps devcore's [`Databases`]; a headless test implements it with fixtures
/// and never runs launchd, a database binary or `lsof`.
pub trait Provider: Send + Sync {
    fn snapshot(&self) -> Result<Snapshot, String>;

    /// Receives the same progress text the Go `Event` sink gets.
    fn set_progress_sink(&self, _sink: ProgressSink) {}

    fn next_port(&self) -> Result<i64, String>;
    fn inspect_port(&self, port: i64) -> Result<PortInspection, String>;

    fn create(&self, instance: Instance, cancel: CancelToken) -> Result<(), String>;
    fn action(&self, id: &str, action: InstanceAction, cancel: CancelToken) -> Result<(), String>;

    fn catalog(&self, id: &str, cancel: CancelToken) -> Result<InstanceCatalog, String>;
    fn create_database(&self, id: &str, name: &str, cancel: CancelToken) -> Result<(), String>;
    fn create_user(
        &self,
        id: &str,
        user: &str,
        password: &str,
        database: &str,
        cancel: CancelToken,
    ) -> Result<(), String>;
    fn drop_database(&self, id: &str, name: &str, cancel: CancelToken) -> Result<(), String>;
    fn drop_user(
        &self,
        id: &str,
        user: &str,
        database: &str,
        cancel: CancelToken,
    ) -> Result<(), String>;

    fn backups(&self, id: &str, cancel: CancelToken) -> Result<Vec<BackupFile>, String>;
    fn backup(&self, id: &str, database: &str, cancel: CancelToken) -> Result<String, String>;
    fn restore(
        &self,
        id: &str,
        database: &str,
        path: &str,
        cancel: CancelToken,
    ) -> Result<(), String>;
    fn delete_backup(&self, id: &str, name: &str, cancel: CancelToken) -> Result<(), String>;

    fn save_profile(&self, profile: ConnectionProfile) -> Result<ConnectionProfile, String>;
    fn delete_profile(&self, instance_id: &str, profile_id: &str) -> Result<(), String>;
    fn connection_info(
        &self,
        instance_id: &str,
        profile_id: &str,
    ) -> Result<ConnectionInfo, String>;
    fn connection_details(&self, id: &str) -> Result<String, String>;
    fn password(&self, id: &str) -> Result<String, String>;
    fn open_client(&self, uri: &str) -> Result<(), String>;
    fn open_cli(
        &self,
        id: &str,
        profile_id: &str,
        database: Option<&str>,
        cancel: CancelToken,
    ) -> Result<(), String>;

    fn upgrade(&self, instance: &Instance, target: &str, cancel: CancelToken)
    -> Result<(), String>;

    fn preview_deletion(&self, id: &str) -> Result<InstanceDeletionPreview, String>;
    fn delete_instance_data(
        &self,
        id: &str,
        fingerprint: &str,
        confirmation: &str,
        cancel: CancelToken,
    ) -> Result<(), String>;
    fn pending_deletions(&self) -> Result<Vec<InstanceDeletionInfo>, String>;
    fn restore_deletion(&self, id: &str, revision: &str, cancel: CancelToken)
    -> Result<(), String>;
    fn finish_deletion(
        &self,
        id: &str,
        revision: &str,
        confirmation: &str,
        cancel: CancelToken,
    ) -> Result<(), String>;

    fn pending_initializations(&self) -> Result<Vec<InstanceInitializationInfo>, String>;
    fn resolve_initialization(
        &self,
        id: &str,
        revision: &str,
        cancel: CancelToken,
    ) -> Result<String, String>;
    fn pending_work(&self) -> Result<Vec<DatabaseWorkInfo>, String>;
    fn resolve_work(&self, id: &str, revision: &str) -> Result<(), String>;
}

// --- The production provider -------------------------------------------------

/// The development home, opened once. Its store forwards devcore's progress
/// text to the sink the store installs, exactly where Go observes `m.Event`.
struct Facade {
    store: Store,
    sink: Arc<Mutex<Option<ProgressSink>>>,
}

impl Facade {
    fn new() -> Self {
        let sink: Arc<Mutex<Option<ProgressSink>>> = Arc::new(Mutex::new(None));
        let hook = sink.clone();
        let store = Store::new(devcore::state::default_root()).with_event(move |message| {
            let current = hook
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(sink) = current {
                sink(message.to_string());
            }
        });
        Facade { store, sink }
    }

    fn databases(&self) -> Databases {
        Databases::new(self.store.clone())
    }

    fn state(&self) -> Result<State, String> {
        self.store.read().map_err(|error| error.to_string())
    }
}

impl Provider for Facade {
    fn snapshot(&self) -> Result<Snapshot, String> {
        let state = self.state()?;
        let databases = self.databases();
        let mut instances: Vec<Instance> = state.instances.values().cloned().collect();
        instances.sort_by(|left, right| left.id.cmp(&right.id));
        let mut statuses = BTreeMap::new();
        let mut upgrades = BTreeMap::new();
        for instance in &instances {
            let (job, error) =
                read_job_state(databases.engine(), &format!("db-{}", instance.id), &|| {
                    false
                });
            let status = match error {
                Ok(()) if job.running => "Running",
                Ok(()) => "Stopped",
                Err(error) => {
                    let message = error.to_string();
                    if message.contains("Could not find service")
                        || message.contains("Could not find specified service")
                    {
                        "Stopped"
                    } else {
                        "Unknown"
                    }
                }
            };
            statuses.insert(instance.id.clone(), status.to_string());
            upgrades.insert(
                instance.id.clone(),
                devcore::databases::upgrade::upgrade_targets(&state, instance),
            );
        }
        Ok(Snapshot {
            instances,
            packages: state.packages.clone(),
            profiles: state.connection_profiles.clone(),
            statuses,
            upgrades,
        })
    }

    fn set_progress_sink(&self, sink: ProgressSink) {
        *self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
    }

    fn next_port(&self) -> Result<i64, String> {
        let cancelled = || false;
        self.databases()
            .available_local_port(&cancelled)
            .map_err(|error| error.to_string())
    }

    fn inspect_port(&self, port: i64) -> Result<PortInspection, String> {
        let cancelled = || false;
        self.databases()
            .inspect_port(port, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn create(&self, instance: Instance, cancel: CancelToken) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .create_instance(instance, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn action(&self, id: &str, action: InstanceAction, cancel: CancelToken) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .instance_action(id, action, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn catalog(&self, id: &str, cancel: CancelToken) -> Result<InstanceCatalog, String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .inspect_instance_catalog(id, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn create_database(&self, id: &str, name: &str, cancel: CancelToken) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .create_database(id, name, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn create_user(
        &self,
        id: &str,
        user: &str,
        password: &str,
        database: &str,
        cancel: CancelToken,
    ) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .create_user(id, user, password, database, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn drop_database(&self, id: &str, name: &str, cancel: CancelToken) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .drop_database(id, name, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn drop_user(
        &self,
        id: &str,
        user: &str,
        database: &str,
        cancel: CancelToken,
    ) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .drop_user(id, user, database, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn backups(&self, id: &str, cancel: CancelToken) -> Result<Vec<BackupFile>, String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .backups(id, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn backup(&self, id: &str, database: &str, cancel: CancelToken) -> Result<String, String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .backup(id, database, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn restore(
        &self,
        id: &str,
        database: &str,
        path: &str,
        cancel: CancelToken,
    ) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .restore(id, database, path, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn delete_backup(&self, id: &str, name: &str, cancel: CancelToken) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .delete_backup(id, name, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn save_profile(&self, profile: ConnectionProfile) -> Result<ConnectionProfile, String> {
        self.databases()
            .save_connection_profile(profile)
            .map_err(|error| error.to_string())
    }

    fn delete_profile(&self, instance_id: &str, profile_id: &str) -> Result<(), String> {
        self.databases()
            .delete_connection_profile(instance_id, profile_id)
            .map_err(|error| error.to_string())
    }

    fn connection_info(
        &self,
        instance_id: &str,
        profile_id: &str,
    ) -> Result<ConnectionInfo, String> {
        let databases = self.databases();
        let answer = if profile_id.is_empty() {
            databases.connection_info(instance_id)
        } else {
            databases.profile_connection_info(instance_id, profile_id)
        };
        answer.map_err(|error| error.to_string())
    }

    fn connection_details(&self, id: &str) -> Result<String, String> {
        self.databases()
            .connection_text(id)
            .map_err(|error| error.to_string())
    }

    fn password(&self, id: &str) -> Result<String, String> {
        self.databases()
            .database_password(id)
            .map_err(|error| error.to_string())
    }

    fn open_client(&self, uri: &str) -> Result<(), String> {
        let cancelled = || false;
        self.databases()
            .engine()
            .run_command("/usr/bin/open", &[uri], &cancelled)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn open_cli(
        &self,
        id: &str,
        profile_id: &str,
        database: Option<&str>,
        cancel: CancelToken,
    ) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .open_database_cli(id, profile_id, database, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn upgrade(
        &self,
        instance: &Instance,
        target: &str,
        cancel: CancelToken,
    ) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .upgrade_instance_package(instance, target, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn preview_deletion(&self, id: &str) -> Result<InstanceDeletionPreview, String> {
        let cancelled = || false;
        self.databases()
            .preview_instance_deletion(id, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn delete_instance_data(
        &self,
        id: &str,
        fingerprint: &str,
        confirmation: &str,
        cancel: CancelToken,
    ) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .delete_instance_data(id, fingerprint, confirmation, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn pending_deletions(&self) -> Result<Vec<InstanceDeletionInfo>, String> {
        let cancelled = || false;
        self.databases()
            .pending_instance_deletions(&cancelled)
            .map_err(|error| error.to_string())
    }

    fn restore_deletion(
        &self,
        id: &str,
        revision: &str,
        cancel: CancelToken,
    ) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .restore_instance_deletion(id, revision, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn finish_deletion(
        &self,
        id: &str,
        revision: &str,
        confirmation: &str,
        cancel: CancelToken,
    ) -> Result<(), String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .finish_instance_deletion(id, revision, confirmation, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn pending_initializations(&self) -> Result<Vec<InstanceInitializationInfo>, String> {
        let cancelled = || false;
        self.databases()
            .pending_instance_initializations(&cancelled)
            .map_err(|error| error.to_string())
    }

    fn resolve_initialization(
        &self,
        id: &str,
        revision: &str,
        cancel: CancelToken,
    ) -> Result<String, String> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.databases()
            .resolve_instance_initialization(id, revision, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn pending_work(&self) -> Result<Vec<DatabaseWorkInfo>, String> {
        let cancelled = || false;
        self.databases()
            .pending_database_work(&cancelled)
            .map_err(|error| error.to_string())
    }

    fn resolve_work(&self, id: &str, revision: &str) -> Result<(), String> {
        self.databases()
            .resolve_database_work(id, revision)
            .map_err(|error| error.to_string())
    }
}

// --- Providers ---------------------------------------------------------------

static FAKE: LazyLock<Mutex<Option<Arc<dyn Provider>>>> = LazyLock::new(|| Mutex::new(None));
static FAKE_MODE: AtomicBool = AtomicBool::new(false);

fn fake() -> &'static Mutex<Option<Arc<dyn Provider>>> {
    &FAKE
}

static FACADE: LazyLock<Facade> = LazyLock::new(Facade::new);

fn provider_of() -> Option<Arc<dyn Provider>> {
    if let Some(provider) = fake()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return Some(provider);
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return None;
    }
    Some(Arc::new(Facade {
        store: FACADE.store.clone(),
        sink: FACADE.sink.clone(),
    }) as Arc<dyn Provider>)
}

/// Makes the surface use a test provider instead of devcore. Call it before
/// starting the headless surface, then [`install_provider`]. Tests only.
pub fn use_fake_provider() {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *fake()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

pub(crate) fn is_fake_mode() -> bool {
    FAKE_MODE.load(Ordering::Relaxed)
}

/// Gives the surface a provider of the test's own. Tests only.
pub fn install_provider(provider: Arc<dyn Provider>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *fake()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

// --- State -------------------------------------------------------------------

/// What the workspace shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    /// The form of a new instance.
    #[default]
    New,
    /// A registered instance's detail.
    Instance,
    /// The permanent-deletion review of one instance.
    DeletionPreview,
    /// The staged deletions left by an interrupted erasure.
    Deletions,
    /// The unfinished database-task reservations.
    Work,
    /// The unfinished preparations.
    Initializations,
}

/// One read of the development home.
#[derive(Debug, Clone, Default)]
struct LoadState {
    loading: bool,
    loaded: bool,
    error: Option<String>,
    /// Bumped for every read, so an older answer never replaces a newer one.
    sequence: u64,
}

/// A list read with its own staleness token.
#[derive(Debug, Clone)]
struct ListState<T> {
    items: Vec<T>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    sequence: u64,
}

impl<T> Default for ListState<T> {
    fn default() -> Self {
        ListState {
            items: Vec::new(),
            loading: false,
            loaded: false,
            error: None,
            sequence: 0,
        }
    }
}

/// One instance's catalog.
#[derive(Debug, Clone, Default)]
struct CatalogState {
    instance_id: String,
    catalog: InstanceCatalog,
    loading: bool,
    error: Option<String>,
    sequence: u64,
}

/// One instance's finished backups.
#[derive(Debug, Clone, Default)]
struct BackupsState {
    instance_id: String,
    rows: Vec<BackupFile>,
    loading: bool,
    error: Option<String>,
    sequence: u64,
}

/// The deletion review being read or shown.
#[derive(Debug, Clone, Default)]
struct PreviewState {
    instance_id: String,
    preview: Option<InstanceDeletionPreview>,
    loading: bool,
    error: Option<String>,
    sequence: u64,
}

/// The port preflight of the form.
#[derive(Debug, Clone, Default)]
struct PortState {
    /// The port the inspection describes; `None` means "no answer yet".
    port: Option<i64>,
    inspection: Option<PortInspection>,
    suggestion: Option<i64>,
    loading: bool,
    error: Option<String>,
    sequence: u64,
}

/// An operation in flight, or its result until dismissed.
#[derive(Debug, Clone)]
struct Task {
    /// `initialize`, `start`, `stop`, `restart`, `remove`, `backup`, ...
    kind: String,
    progress: String,
    /// The finished line: the Go result text, with its `Error:` line.
    result: Option<String>,
    running: bool,
    cancelled: bool,
    cancel: Option<CancelToken>,
}

/// A switch the connection draft is holding back until it is discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingSwitch {
    Profile(String),
    Instance(String),
    NewInstance,
}

/// The connection panel's selection (editable fields live in [`DraftSignals`]).
#[derive(Debug, Clone, Default)]
struct ProfileState {
    instance_id: String,
    /// `""` is the administrator, `"new"` a profile being added, else an id.
    selected: String,
    pending: Option<PendingSwitch>,
}

/// Per-field draft input signals kept outside [`Workspace`] so typing never
/// invalidates workspace-level views or tears down focused `input()` nodes.
#[derive(Clone, Copy)]
struct DraftSignals {
    // New-instance form
    draft_name: Signal<String>,
    draft_kind: Signal<&'static str>,
    draft_package: Signal<String>,
    draft_port: Signal<String>,
    draft_data_directory: Signal<String>,
    draft_persistent: Signal<bool>,
    draft_auto_port: Signal<bool>,
    // Profile editor
    profile_name: Signal<String>,
    profile_database: Signal<String>,
    profile_user: Signal<String>,
    profile_mongo_auth_source: Signal<String>,
    profile_redis_db: Signal<String>,
    // Catalog / databases & users
    database_name: Signal<String>,
    database_user: Signal<String>,
    database_password: Signal<String>,
    // Backups & restore
    backup_path: Signal<String>,
    restore_database: Signal<String>,
    // Permanent deletion confirmation
    confirmation: Signal<String>,
}

impl Default for DraftSignals {
    fn default() -> Self {
        DraftSignals {
            draft_name: signal(String::new()),
            draft_kind: signal("postgresql"),
            draft_package: signal(String::new()),
            draft_port: signal("55432".to_string()),
            draft_data_directory: signal(String::new()),
            draft_persistent: signal(true),
            draft_auto_port: signal(true),
            profile_name: signal(String::new()),
            profile_database: signal(String::new()),
            profile_user: signal(String::new()),
            profile_mongo_auth_source: signal(String::new()),
            profile_redis_db: signal("0".to_string()),
            database_name: signal(String::new()),
            database_user: signal(String::new()),
            database_password: signal(String::new()),
            backup_path: signal(String::new()),
            restore_database: signal(String::new()),
            confirmation: signal(String::new()),
        }
    }
}

impl DraftSignals {
    fn reset_new_draft(&self, package: String) {
        self.draft_name.set(String::new());
        self.draft_kind.set("postgresql");
        self.draft_package.set(package);
        self.draft_port.set("55432".to_string());
        self.draft_data_directory.set(String::new());
        self.draft_persistent.set(true);
        self.draft_auto_port.set(true);
    }

    fn clear_profile_draft(&self) {
        self.profile_name.set(String::new());
        self.profile_database.set(String::new());
        self.profile_user.set(String::new());
        self.profile_mongo_auth_source.set(String::new());
        self.profile_redis_db.set("0".to_string());
    }

    fn clear_instance_inputs(&self) {
        self.database_name.set(String::new());
        self.database_user.set(String::new());
        self.database_password.set(String::new());
        self.backup_path.set(String::new());
        self.restore_database.set(String::new());
    }
}

#[derive(Debug, Clone, Default)]
struct Workspace {
    mode: Mode,
    /// The instance the detail or the deletion review shows.
    selected: Option<String>,
    snapshot: Snapshot,
    load: LoadState,
    catalog: Option<CatalogState>,
    backups: Option<BackupsState>,
    preview: Option<PreviewState>,
    deletions: ListState<InstanceDeletionInfo>,
    initializations: ListState<InstanceInitializationInfo>,
    work: ListState<DatabaseWorkInfo>,
    tasks: BTreeMap<String, Task>,
    /// Non-task actions in flight, by a key of their own.
    aux_busy: BTreeSet<String>,
    port: PortState,
    profile: ProfileState,
    connection_details: Option<String>,
    action_error: Option<String>,
    sequence: u64,
}

fn workspace() -> Signal<Workspace> {
    global::<Workspace>()
}

fn drafts() -> DraftSignals {
    global::<DraftSignals>().get_untracked()
}

/// Runs store work outside the view that asked for it: a pane the store
/// change rebuilds must not cancel the task that is changing it.
fn task(future: impl Future<Output = ()> + 'static) {
    Owner::detached().run(move || {
        spawn(future);
    });
}

/// Runs a store change later, outside the view that asked for it.
fn later(delay: Duration, f: impl FnOnce() + 'static) {
    Owner::detached().run(move || {
        set_timeout(delay, f);
    });
}

fn next_sequence() -> u64 {
    workspace().update(|state| state.sequence += 1);
    workspace().with(|state| state.sequence)
}

fn ctrl_error(message: String) {
    workspace().update(|state| {
        state.action_error = Some(message);
    });
}

fn show_notice(message: impl Into<String>) {
    workspace().update(|state| {
        state.action_error = None;
    });
    crate::toast::success(message);
}

fn show_info_notice(message: impl Into<String>) {
    workspace().update(|state| {
        state.action_error = None;
    });
    crate::toast::info(message);
}

/// Task progress text, queued by the operation's thread and applied on the
/// engine's thread by [`pump_progress`]. devcore emits events from the
/// blocking worker, where reading a signal would be a reactive-runtime error.
static PENDING_PROGRESS: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static PROGRESS_PUMP: AtomicBool = AtomicBool::new(false);

fn note_progress(message: &str) {
    PENDING_PROGRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(message.to_string());
}

fn pump_progress() {
    if PROGRESS_PUMP.swap(true, Ordering::SeqCst) {
        return;
    }
    later(Duration::from_millis(80), pump_progress_once);
}

fn pump_progress_once() {
    let messages: Vec<String> = {
        let mut queued = PENDING_PROGRESS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *queued)
    };
    if !messages.is_empty() {
        workspace().update(|state| {
            for message in &messages {
                for (id, task) in state.tasks.iter_mut() {
                    if task.running && message.contains(id.as_str()) {
                        task.progress = message.clone();
                    }
                }
            }
        });
    }
    let running = workspace().with(|state| state.tasks.values().any(|task| task.running));
    if running {
        later(Duration::from_millis(80), pump_progress_once);
    } else {
        PROGRESS_PUMP.store(false, Ordering::SeqCst);
    }
}

/// Gives the installed provider the store's progress sink.
fn attach_sink() {
    if let Some(provider) = provider_of() {
        provider.set_progress_sink(Arc::new(|message| note_progress(&message)));
    }
}

// --- Reading -----------------------------------------------------------------

pub(crate) fn load() {
    attach_sink();
    let (loaded, loading, need_work, need_inits, need_deletions) = workspace().with(|state| {
        (
            state.load.loaded,
            state.load.loading,
            !state.work.loaded && !state.work.loading,
            !state.initializations.loaded && !state.initializations.loading,
            !state.deletions.loaded && !state.deletions.loading,
        )
    });
    if !loaded && !loading {
        refresh();
    }
    if need_work {
        load_work();
    }
    if need_inits {
        load_initializations();
    }
    if need_deletions {
        load_pending_deletions();
    }
    let needs_port = workspace().with(|state| {
        state.port.suggestion.is_none() && !state.port.loading && state.port.error.is_none()
    });
    if needs_port {
        suggest_port();
    }
}

/// Reads the development home again and keeps the instance shown when it
/// still exists.
pub(crate) fn refresh() {
    let Some(provider) = provider_of() else {
        workspace().update(|state| {
            state.load.loading = false;
            state.load.error = Some("no database provider is available".to_string());
        });
        return;
    };
    attach_sink();
    let sequence = workspace().with(|state| state.load.sequence + 1);
    workspace().update(|state| {
        state.load.sequence = sequence;
        state.load.loading = true;
        state.load.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.snapshot()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the database workspace did not answer: {error}")),
        };
        let mut default_package: Option<String> = None;
        let d = drafts();
        let draft_kind = d.draft_kind.get_untracked();
        let draft_package_empty = d.draft_package.get_untracked().is_empty();
        workspace().update(|state| {
            if state.load.sequence != sequence {
                return;
            }
            state.load.loading = false;
            state.load.loaded = true;
            match answer {
                Ok(snapshot) => {
                    state.snapshot = snapshot;
                    state.load.error = None;
                    if state.mode == Mode::New && draft_package_empty {
                        let package = state
                            .snapshot
                            .packages
                            .values()
                            .filter(|package| package.name == draft_kind)
                            .max_by(|left, right| left.version.cmp(&right.version))
                            .map(Package::id)
                            .unwrap_or_default();
                        default_package = Some(package);
                    }
                }
                Err(error) => state.load.error = Some(error),
            }
            if let Some(selected) = state.selected.clone()
                && !state
                    .snapshot
                    .instances
                    .iter()
                    .any(|instance| instance.id == selected)
            {
                state.selected = None;
                if state.mode == Mode::DeletionPreview {
                    state.mode = Mode::Instance;
                    state.preview = None;
                }
            }
        });
        if let Some(package) = default_package {
            d.draft_package.set(package);
        }
    });
}

pub(crate) fn load_error() -> Option<String> {
    workspace().with(|state| state.load.error.clone())
}

pub(crate) fn action_error() -> Option<String> {
    workspace().with(|state| state.action_error.clone())
}

pub(crate) fn mode() -> Mode {
    workspace().with(|state| state.mode)
}

pub(crate) fn instances() -> Vec<Instance> {
    workspace().with(|state| state.snapshot.instances.clone())
}

pub(crate) fn instance(id: &str) -> Option<Instance> {
    workspace().with(|state| {
        state
            .snapshot
            .instances
            .iter()
            .find(|instance| instance.id == id)
            .cloned()
    })
}

pub(crate) fn selected_id() -> Option<String> {
    workspace().with(|state| state.selected.clone())
}

/// The instance the detail or the deletion review shows.
pub(crate) fn selected_instance() -> Option<Instance> {
    let id = selected_id()?;
    instance(&id)
}

/// The status the row and the header show, including the phase of a running
/// operation (`Starting`, `Stopping`, `Restarting`), as the Go task phases do.
pub(crate) fn status(id: &str) -> String {
    workspace().with(|state| {
        if let Some(task) = state.tasks.get(id)
            && task.running
        {
            match task.kind.as_str() {
                "start" => return "Starting".to_string(),
                "stop" | "remove" => return "Stopping".to_string(),
                "restart" => return "Restarting".to_string(),
                _ => {}
            }
        }
        state
            .snapshot
            .statuses
            .get(id)
            .cloned()
            .unwrap_or_else(|| "Stopped".to_string())
    })
}

pub(crate) fn is_running(id: &str) -> bool {
    matches!(status(id).as_str(), "Running" | "Healthy")
}

pub(crate) fn package_version(id: &str) -> String {
    workspace().with(|state| {
        state
            .snapshot
            .packages
            .get(id)
            .map(|package| package.version.clone())
            .unwrap_or_default()
    })
}

/// The installed packages of an engine, newest first.
pub(crate) fn packages(kind: &str) -> Vec<Package> {
    workspace().with(|state| {
        let mut found: Vec<Package> = state
            .snapshot
            .packages
            .values()
            .filter(|package| package.name == kind)
            .cloned()
            .collect();
        found.sort_by(|left, right| left.version.cmp(&right.version).reverse());
        found
    })
}

pub(crate) fn operations_busy() -> bool {
    workspace()
        .with(|state| state.tasks.values().any(|task| task.running) || !state.aux_busy.is_empty())
}

/// The tasks to show, in instance-id order.
pub(crate) fn tasks() -> Vec<(String, String, String, Option<String>, bool)> {
    workspace().with(|state| {
        state
            .tasks
            .iter()
            .map(|(id, task)| {
                (
                    id.clone(),
                    task.kind.clone(),
                    task.progress.clone(),
                    task.result.clone(),
                    task.running,
                )
            })
            .collect()
    })
}

// --- Navigation --------------------------------------------------------------

/// Opens an instance's detail and reads what only then matters.
pub(crate) fn open_instance(id: &str) {
    if instance(id).is_none() {
        return;
    }
    if workspace().with(|state| state.profile.pending.is_some()) {
        return;
    }
    if profile_dirty() {
        workspace().update(|state| {
            state.profile.pending = Some(PendingSwitch::Instance(id.to_string()));
        });
        return;
    }
    show_instance(id);
}

fn show_instance(id: &str) {
    drafts().confirmation.set(String::new());
    workspace().update(|state| {
        state.mode = Mode::Instance;
        state.selected = Some(id.to_string());
        state.preview = None;
        state.action_error = None;
        state.connection_details = None;
    });
    load_profile_for(id);
    refresh();
    load_catalog(id);
    load_backups(id);
}

/// Opens the new-instance form, filling its port from the provider.
pub(crate) fn open_new() {
    if workspace().with(|state| state.profile.pending.is_some()) {
        return;
    }
    if profile_dirty() {
        workspace().update(|state| {
            state.profile.pending = Some(PendingSwitch::NewInstance);
        });
        return;
    }
    show_new();
}

fn show_new() {
    let package = packages("postgresql")
        .first()
        .map(Package::id)
        .unwrap_or_default();
    let d = drafts();
    d.confirmation.set(String::new());
    d.reset_new_draft(package);
    d.clear_profile_draft();
    d.clear_instance_inputs();
    workspace().update(|state| {
        state.mode = Mode::New;
        state.selected = None;
        state.preview = None;
        state.action_error = None;
        state.profile = ProfileState::default();
        state.port = PortState::default();
        state.connection_details = None;
    });
    suggest_port();
}

/// Discards the held-back connection switch and makes it.
pub(crate) fn discard_pending_switch() {
    let pending = workspace().with(|state| state.profile.pending.clone());
    workspace().update(|state| state.profile.pending = None);
    let Some(pending) = pending else {
        return;
    };
    match pending {
        PendingSwitch::Profile(id) => {
            let instance = workspace().with(|state| state.profile.instance_id.clone());
            load_profile_draft(&instance, &id);
        }
        PendingSwitch::Instance(id) => show_instance(&id),
        PendingSwitch::NewInstance => show_new(),
    }
}

pub(crate) fn cancel_pending_switch() {
    workspace().update(|state| state.profile.pending = None);
}

pub(crate) fn pending_switch() -> bool {
    workspace().with(|state| state.profile.pending.is_some())
}

// --- The new-instance form ---------------------------------------------------

pub(crate) fn draft_name() -> String {
    drafts().draft_name.get()
}

pub(crate) fn set_draft_name(next: String) {
    drafts().draft_name.set(next);
}

pub(crate) fn draft_kind() -> &'static str {
    drafts().draft_kind.get()
}

pub(crate) fn set_draft_kind(kind: &'static str) {
    if !SoftwareKind::from_key(kind).is_some_and(SoftwareKind::is_database) {
        return;
    }
    let package = packages(kind).first().map(Package::id).unwrap_or_default();
    let d = drafts();
    d.draft_kind.set(kind);
    d.draft_package.set(package);
    d.draft_auto_port.set(true);
    d.database_name.set(String::new());
    d.database_user.set(String::new());
    suggest_port();
}

pub(crate) fn draft_package() -> String {
    drafts().draft_package.get()
}

pub(crate) fn set_draft_package(id: String) {
    drafts().draft_package.set(id);
}

pub(crate) fn draft_port() -> String {
    drafts().draft_port.get()
}

pub(crate) fn set_draft_port(next: String) {
    let d = drafts();
    d.draft_port.set(next);
    d.draft_auto_port.set(false);
    later(Duration::from_millis(400), check_port_quietly);
}

pub(crate) fn draft_data_directory() -> String {
    drafts().draft_data_directory.get()
}

pub(crate) fn set_draft_data_directory(next: String) {
    drafts().draft_data_directory.set(next);
}

pub(crate) fn use_app_storage() {
    drafts().draft_data_directory.set(String::new());
}

pub(crate) fn draft_persistent() -> bool {
    drafts().draft_persistent.get()
}

pub(crate) fn set_draft_persistent(next: bool) {
    drafts().draft_persistent.set(next);
}

/// Asks the provider for a free port and puts it in an untouched field. The
/// Go wording is kept: the suggestion reserves nothing.
pub(crate) fn suggest_port() {
    let Some(provider) = provider_of() else {
        ctrl_error("no database provider is available".to_string());
        return;
    };
    attach_sink();
    let sequence = next_sequence();
    workspace().update(|state| {
        state.port.loading = true;
        state.port.error = None;
        state.port.sequence = sequence;
    });
    task(async move {
        let answer = match blocking(move || provider.next_port()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the port suggestion failed: {error}")),
        };
        let mut fill_port: Option<i64> = None;
        workspace().update(|state| {
            if state.port.sequence != sequence {
                return;
            }
            state.port.loading = false;
            match answer {
                Ok(port) => {
                    state.port.suggestion = Some(port);
                    fill_port = Some(port);
                    state.port.error = None;
                }
                Err(error) => state.port.error = Some(error),
            }
        });
        if let Some(port) = fill_port {
            let d = drafts();
            if d.draft_auto_port.get_untracked() || d.draft_port.get_untracked().trim().is_empty() {
                d.draft_port.set(port.to_string());
                d.draft_auto_port.set(true);
            }
        }
    });
}

/// The Go `Find a free port` result text.
pub(crate) fn find_free_port() {
    let Some(provider) = provider_of() else {
        ctrl_error("no database provider is available".to_string());
        return;
    };
    attach_sink();
    let sequence = next_sequence();
    workspace().update(|state| {
        state.port.loading = true;
        state.port.error = None;
        state.aux_busy.insert("find-port".to_string());
        state.port.sequence = sequence;
    });
    task(async move {
        let answer = match blocking(move || provider.next_port()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the port suggestion failed: {error}")),
        };
        let mut suggested: Option<i64> = None;
        workspace().update(|state| {
            state.aux_busy.remove("find-port");
            if state.port.sequence != sequence {
                return;
            }
            state.port.loading = false;
            match answer {
                Ok(port) => {
                    state.port.suggestion = Some(port);
                    suggested = Some(port);
                    state.action_error = None;
                }
                Err(error) => state.port.error = Some(error),
            }
        });
        if let Some(port) = suggested {
            let d = drafts();
            d.draft_port.set(port.to_string());
            d.draft_auto_port.set(true);
            show_info_notice(format!(
                "Suggested port: {port}. An unchanged form is filled automatically. Review, save and retry; this does not reserve the port."
            ));
        }
    });
}

/// The port preflight of the form, on request.
pub(crate) fn check_port() {
    let port = drafts()
        .draft_port
        .get_untracked()
        .trim()
        .parse::<i64>()
        .ok();
    if let Some(port) = port {
        inspect_port_now(port);
    }
}

fn check_port_quietly() {
    if workspace().with(|state| state.mode != Mode::New) {
        return;
    }
    let port = drafts()
        .draft_port
        .get_untracked()
        .trim()
        .parse::<i64>()
        .ok();
    if let Some(port) = port {
        inspect_port_now(port);
    }
}

fn inspect_port_now(port: i64) {
    let Some(provider) = provider_of() else {
        return;
    };
    attach_sink();
    let sequence = next_sequence();
    workspace().update(|state| {
        state.port.loading = true;
        state.port.error = None;
        state.port.sequence = sequence;
    });
    task(async move {
        let answer = match blocking(move || provider.inspect_port(port)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the port inspection did not answer: {error}")),
        };
        workspace().update(|state| {
            if state.port.sequence != sequence {
                return;
            }
            state.port.loading = false;
            match answer {
                Ok(inspection) => {
                    state.port.port = Some(inspection.port);
                    state.port.inspection = Some(inspection);
                }
                Err(error) => state.port.error = Some(error),
            }
        });
    });
}

/// The port inspection of the form, when it describes the port in the field.
pub(crate) fn port_inspection() -> Option<PortInspection> {
    let field = drafts().draft_port.get().trim().parse::<i64>().ok();
    workspace().with(|state| {
        let inspection = state.port.inspection.clone()?;
        (field == Some(inspection.port)).then_some(inspection)
    })
}

pub(crate) fn port_error() -> Option<String> {
    workspace().with(|state| state.port.error.clone())
}

pub(crate) fn port_loading() -> bool {
    workspace().with(|state| state.port.loading)
}

/// Creates the instance in the form. Non-numeric ports surface the Go
/// `Enter a valid port` message; everything else is devcore's own wording.
pub(crate) fn create_instance() {
    let d = drafts();
    let port_text = d.draft_port.get_untracked();
    let kind = d.draft_kind.get_untracked();
    let package = d.draft_package.get_untracked();
    let data_directory = d.draft_data_directory.get_untracked();
    let name = d.draft_name.get_untracked();
    let persistent = d.draft_persistent.get_untracked();
    let prepared = (|| {
        let port = match port_text.trim().parse::<i64>() {
            Ok(port) => port,
            Err(_) => return Err("Error: Enter a valid port".to_string()),
        };
        if package.is_empty() {
            let title = SoftwareKind::from_key(kind)
                .map(SoftwareKind::title)
                .unwrap_or(kind);
            return Err(format!("Install {title} first, then create the instance."));
        }
        Ok(Instance {
            data_identity: String::new(),
            data_directory: data_directory.trim().to_string(),
            data_owner: String::new(),
            id: name.trim().to_string(),
            kind: kind.to_string(),
            package,
            port,
            persistent,
        })
    })();
    let instance = match prepared {
        Ok(instance) => instance,
        Err(message) => {
            ctrl_error(message);
            return;
        }
    };
    if operations_busy() {
        return;
    }
    let Some(provider) = provider_of() else {
        ctrl_error("no database provider is available".to_string());
        return;
    };
    attach_sink();
    start_task(
        &instance.id,
        "initialize",
        format!("Preparing {}", instance.id),
    );
    let id = instance.id.clone();
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        register_cancel(&id, cancel.clone());
        let request = instance.clone();
        let token = cancel.clone();
        let answer = match blocking(move || provider.create(request, token)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the initialization did not finish: {error}")),
        };
        let result = match answer {
            Ok(()) => Ok(format!("Created {id}")),
            Err(error) => Err(error),
        };
        finish_task(&id, "initialize", result);
        if workspace().with(|state| {
            state.tasks.get(&id).is_some_and(|task| {
                task.result
                    .as_deref()
                    .is_some_and(|line| !line.contains("Error:"))
            })
        }) {
            let d = drafts();
            d.reset_new_draft(String::new());
            d.confirmation.set(String::new());
            workspace().update(|state| {
                state.mode = Mode::Instance;
                state.selected = Some(id.clone());
            });
            load_profile_for(&id);
            refresh();
            load_catalog(&id);
            load_backups(&id);
        }
    });
}

fn start_task(id: &str, kind: &str, progress: String) {
    workspace().update(|state| {
        state.tasks.insert(
            id.to_string(),
            Task {
                kind: kind.to_string(),
                progress,
                result: None,
                running: true,
                cancelled: false,
                cancel: None,
            },
        );
        state.action_error = None;
    });
    pump_progress();
}

fn register_cancel(id: &str, token: CancelToken) {
    workspace().update(|state| {
        if let Some(task) = state.tasks.get_mut(id) {
            task.cancel = Some(token);
        }
    });
}

fn task_label(kind: &str) -> &'static str {
    if kind == "initialize" {
        "Initialization"
    } else {
        "Database operation"
    }
}

fn finish_task(id: &str, kind: &str, answer: Result<String, String>) {
    let notification = if matches!(kind, "initialize" | "backup" | "restore" | "upgrade") {
        let (title, body, failed) = match &answer {
            Ok(msg) => {
                let title = match kind {
                    "initialize" => format!("Database {id} created"),
                    "backup" => format!("Backup completed for {id}"),
                    "restore" => format!("Restore completed for {id}"),
                    "upgrade" => format!("Database {id} upgraded"),
                    _ => format!("Database task completed for {id}"),
                };
                let body = if msg.is_empty() {
                    title.clone()
                } else {
                    msg.clone()
                };
                (title, body, false)
            }
            Err(error) => {
                let title = match kind {
                    "initialize" => format!("Database {id} creation failed"),
                    "backup" => format!("Backup failed for {id}"),
                    "restore" => format!("Restore failed for {id}"),
                    "upgrade" => format!("Upgrade failed for {id}"),
                    _ => format!("Database task failed for {id}"),
                };
                (title, error.clone(), true)
            }
        };
        Some((format!("database-{kind}-{id}"), title, body, failed))
    } else {
        None
    };
    workspace().update(|state| {
        let Some(task) = state.tasks.get_mut(id) else {
            return;
        };
        task.running = false;
        task.cancel = None;
        task.result = Some(match answer {
            Ok(result) => result,
            Err(error) => format!(
                "{} did not complete.\nError: {error}",
                task_label(&task.kind)
            ),
        });
    });
    if let Some((notif_id, title, body, failed)) = notification {
        crate::notify_task(
            &notif_id,
            &title,
            &body,
            crate::store::Section::Databases,
            failed,
        );
    }
}

/// Cancels the running operation of an instance, as the Go task's context
/// cancel does: files already written stay.
pub(crate) fn cancel_task(id: &str) {
    workspace().update(|state| {
        let Some(task) = state.tasks.get_mut(id) else {
            return;
        };
        if !task.running {
            return;
        }
        task.cancelled = true;
        if let Some(token) = &task.cancel {
            token.store(true, Ordering::Relaxed);
        }
    });
}

pub(crate) fn dismiss_task(id: &str) {
    workspace().update(|state| {
        if state.tasks.get(id).is_some_and(|task| !task.running) {
            state.tasks.remove(id);
        }
    });
}

// --- Lifecycle ---------------------------------------------------------------

pub(crate) fn start_instance(id: &str) {
    instance_action(id, InstanceAction::Start);
}

pub(crate) fn stop_instance(id: &str) {
    instance_action(id, InstanceAction::Stop);
}

pub(crate) fn restart_instance(id: &str) {
    instance_action(id, InstanceAction::Restart);
}

/// Removes the registration; the data directory is kept, and an external one
/// is never touched.
pub(crate) fn remove_instance(id: &str) {
    instance_action(id, InstanceAction::Remove);
}

/// Asks before removing an instance registration (used from the rail context
/// menu); data files are always kept.
pub(crate) fn request_remove_instance(id: &str) {
    let id_owned = id.to_string();
    let answer = alert_dialog()
        .title(format!("Remove {id_owned}?"))
        .description(
            "Removes the instance registration from Devbase. Its data files are kept on disk.",
        )
        .cancel("cancel", "Cancel")
        .destructive("confirm", "Remove Instance")
        .show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if chosen {
            remove_instance(&id_owned);
        }
    });
}

fn instance_action(id: &str, action: InstanceAction) {
    let Some(provider) = provider_of() else {
        ctrl_error("no database provider is available".to_string());
        return;
    };
    attach_sink();
    let verb = action.as_str();
    let phase = match action {
        InstanceAction::Start => "Starting",
        InstanceAction::Stop | InstanceAction::Remove => "Stopping",
        InstanceAction::Restart => "Restarting",
    };
    start_task(id, verb, format!("{phase} {id}"));
    let id = id.to_string();
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        register_cancel(&id, cancel.clone());
        let request = id.clone();
        let token = cancel.clone();
        let answer = match blocking(move || provider.action(&request, action, token)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the action did not finish: {error}")),
        };
        let result = match (action, &answer) {
            (InstanceAction::Start, Ok(())) => Ok(format!("Started {id}")),
            (InstanceAction::Stop, Ok(())) => Ok(format!("Stopped {id}")),
            (InstanceAction::Restart, Ok(())) => Ok(format!("Restarted {id}")),
            (InstanceAction::Remove, Ok(())) => Ok("Removed instance".to_string()),
            (_, Err(error)) => Err(error.clone()),
        };
        finish_task(&id, verb, result);
        let succeeded = answer.is_ok();
        workspace().update(|state| {
            if succeeded && action == InstanceAction::Remove {
                state.tasks.remove(&id);
                state.mode = Mode::New;
                state.selected = None;
                state.preview = None;
                state.profile = ProfileState::default();
            }
        });
        refresh();
        if succeeded {
            if action == InstanceAction::Remove {
                show_new();
            } else {
                load_catalog(&id);
            }
            load_backups(&id);
        }
    });
}

// --- Catalog -----------------------------------------------------------------

/// Reads the instance's databases and users when it is running.
fn load_catalog(id: &str) {
    if !is_running(id) {
        workspace().update(|state| {
            state.catalog = None;
        });
        return;
    }
    if workspace().with(|state| {
        state.catalog.as_ref().is_some_and(|catalog| {
            catalog.instance_id == id && (catalog.loading || catalog.error.is_none())
        })
    }) {
        return;
    }
    refresh_catalog(id);
}

pub(crate) fn refresh_catalog(id: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    attach_sink();
    let sequence = next_sequence();
    workspace().update(|state| {
        state.catalog = Some(CatalogState {
            instance_id: id.to_string(),
            catalog: state
                .catalog
                .as_ref()
                .filter(|catalog| catalog.instance_id == id)
                .map(|catalog| catalog.catalog.clone())
                .unwrap_or_default(),
            loading: true,
            error: None,
            sequence,
        });
    });
    let id = id.to_string();
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let request = id.clone();
        let answer = match blocking(move || provider.catalog(&request, token)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the catalog read did not finish: {error}")),
        };
        workspace().update(|state| {
            let Some(catalog) = state.catalog.as_mut() else {
                return;
            };
            if catalog.instance_id != id || catalog.sequence != sequence {
                return;
            }
            catalog.loading = false;
            match answer {
                Ok(read) => {
                    catalog.catalog = read;
                    catalog.error = None;
                }
                Err(error) => catalog.error = Some(error),
            }
        });
    });
}

pub(crate) fn catalog() -> Option<InstanceCatalog> {
    workspace().with(|state| {
        let catalog = state.catalog.as_ref()?;
        (catalog.error.is_none()).then(|| catalog.catalog.clone())
    })
}

pub(crate) fn catalog_loading() -> bool {
    workspace().with(|state| {
        state
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.loading)
    })
}

pub(crate) fn catalog_error() -> Option<String> {
    workspace().with(|state| {
        state
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.error.clone())
    })
}

pub(crate) fn database_name() -> String {
    drafts().database_name.get()
}

pub(crate) fn set_database_name(next: String) {
    drafts().database_name.set(next);
}

pub(crate) fn database_user() -> String {
    drafts().database_user.get()
}

pub(crate) fn set_database_user(next: String) {
    drafts().database_user.set(next);
}

pub(crate) fn database_password() -> String {
    drafts().database_password.get()
}

pub(crate) fn set_database_password(next: String) {
    drafts().database_password.set(next);
}

/// Creates a database in the instance.
pub(crate) fn create_database(id: &str) {
    let name = drafts().database_name.get_untracked().trim().to_string();
    run_catalog_task(
        id,
        "create database",
        format!("Creating database {name}"),
        format!("Created database {name}"),
        move |provider, instance, cancel| provider.create_database(instance, &name, cancel),
    );
}

/// Drops a database after the Go confirmation dialog.
pub(crate) fn request_drop_database(id: &str) {
    let name = drafts().database_name.get_untracked().trim().to_string();
    let id_owned = id.to_string();
    let answer = alert_dialog()
        .title(format!("Drop database {name}?"))
        .description(format!(
            "Permanently drops database {name} and its data in {id_owned}. This cannot be undone."
        ))
        .cancel("cancel", "Cancel")
        .destructive("confirm", "Drop Database")
        .show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if !chosen {
            return;
        }
        let name = name.clone();
        let cleared = name.clone();
        run_catalog_task(
            &id_owned,
            "drop database",
            format!("Dropping database {name}"),
            format!("Dropped database {name}"),
            move |provider, instance, cancel| provider.drop_database(instance, &name, cancel),
        );
        later(Duration::from_secs(2), move || {
            let d = drafts();
            if d.database_name.get_untracked().trim() == cleared {
                d.database_name.set(String::new());
            }
        });
    });
}

/// Creates a database user; the password is passed through and then dropped
/// from the form, never kept in the store.
pub(crate) fn create_database_user(id: &str) {
    let d = drafts();
    let user = d.database_user.get_untracked().trim().to_string();
    let database = d.database_name.get_untracked().trim().to_string();
    let password = d.database_password.get_untracked();
    d.database_password.set(String::new());
    run_catalog_task(
        id,
        "create user",
        format!("Creating user {user}"),
        format!("Created user {user}"),
        move |provider, instance, cancel| {
            provider.create_user(instance, &user, &password, &database, cancel)
        },
    );
}

/// Drops a user after the Go confirmation dialog.
pub(crate) fn request_drop_user(id: &str) {
    let d = drafts();
    let user = d.database_user.get_untracked().trim().to_string();
    let database = d.database_name.get_untracked().trim().to_string();
    let id_owned = id.to_string();
    let answer = alert_dialog()
        .title(format!("Drop user {user}?"))
        .description(format!(
            "Removes database user {user} from {id_owned}. This cannot be undone."
        ))
        .cancel("cancel", "Cancel")
        .destructive("confirm", "Drop User")
        .show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if !chosen {
            return;
        }
        let cleared = user.clone();
        run_catalog_task(
            &id_owned,
            "drop user",
            format!("Dropping user {user}"),
            format!("Dropped user {user}"),
            move |provider, instance, cancel| {
                provider.drop_user(instance, &user, &database, cancel)
            },
        );
        later(Duration::from_secs(2), move || {
            let d = drafts();
            if d.database_user.get_untracked().trim() == cleared {
                d.database_user.set(String::new());
            }
        });
    });
}

fn run_catalog_task(
    id: &str,
    kind: &'static str,
    progress: String,
    success: String,
    work: impl FnOnce(&Arc<dyn Provider>, &str, CancelToken) -> Result<(), String> + Send + 'static,
) {
    if operations_busy() {
        return;
    }
    let Some(provider) = provider_of() else {
        ctrl_error("no database provider is available".to_string());
        return;
    };
    attach_sink();
    start_task(id, kind, progress);
    let id = id.to_string();
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        register_cancel(&id, cancel.clone());
        let request = id.clone();
        let answer = match blocking(move || work(&provider, &request, cancel)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the operation did not finish: {error}")),
        };
        let result = match answer {
            Ok(()) => Ok(success),
            Err(error) => Err(error),
        };
        finish_task(&id, kind, result);
        refresh_catalog(&id);
        load_backups(&id);
    });
}

// --- Backups -----------------------------------------------------------------

fn load_backups(id: &str) {
    if workspace().with(|state| {
        state.backups.as_ref().is_some_and(|backups| {
            backups.instance_id == id && (backups.loading || backups.error.is_none())
        })
    }) {
        return;
    }
    refresh_backups(id);
}

pub(crate) fn refresh_backups(id: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    attach_sink();
    let sequence = next_sequence();
    workspace().update(|state| {
        state.backups = Some(BackupsState {
            instance_id: id.to_string(),
            rows: state
                .backups
                .as_ref()
                .filter(|backups| backups.instance_id == id)
                .map(|backups| backups.rows.clone())
                .unwrap_or_default(),
            loading: true,
            error: None,
            sequence,
        });
    });
    let id = id.to_string();
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let request = id.clone();
        let answer = match blocking(move || provider.backups(&request, cancel)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("Backups could not be listed: {error}")),
        };
        workspace().update(|state| {
            let Some(backups) = state.backups.as_mut() else {
                return;
            };
            if backups.instance_id != id || backups.sequence != sequence {
                return;
            }
            backups.loading = false;
            match answer {
                Ok(rows) => {
                    backups.rows = rows;
                    backups.error = None;
                }
                Err(error) => backups.error = Some(error),
            }
        });
    });
}

pub(crate) fn backups() -> Vec<BackupFile> {
    workspace().with(|state| {
        let Some(backups) = state.backups.as_ref() else {
            return Vec::new();
        };
        if backups.error.is_some() {
            return Vec::new();
        }
        backups.rows.clone()
    })
}

pub(crate) fn backups_loading() -> bool {
    workspace().with(|state| {
        state
            .backups
            .as_ref()
            .is_some_and(|backups| backups.loading)
    })
}

pub(crate) fn backups_error() -> Option<String> {
    workspace().with(|state| {
        state
            .backups
            .as_ref()
            .and_then(|backups| backups.error.clone())
    })
}

pub(crate) fn backup_path() -> String {
    drafts().backup_path.get()
}

pub(crate) fn set_backup_path(next: String) {
    drafts().backup_path.set(next);
}

pub(crate) fn restore_database() -> String {
    drafts().restore_database.get()
}

pub(crate) fn set_restore_database(next: String) {
    drafts().restore_database.set(next);
}

/// Uses a listed backup for the restore form, naming the new database the way
/// the Go row does.
pub(crate) fn use_backup_for_restore(row: BackupFile) {
    let is_redis = selected_instance().is_some_and(|instance| instance.kind == "redis");
    let d = drafts();
    d.backup_path.set(row.path.display().to_string());
    // The Redis target is the other numbered database, as the Go row chooses.
    d.restore_database.set(if is_redis {
        if row.database == "0" { "1" } else { "0" }.to_string()
    } else {
        format!("{}{}", row.database, "_restored")
    });
}

/// The system's open dialog chooses the restore file, as the Go
/// `Choose backup file…` button did.
pub(crate) fn choose_backup_file() {
    task(async move {
        let answer =
            pick_paths(nsh_core::file_dialog::OpenDialog::files().title("Restore database backup"))
                .await;
        if let Ok(Some(paths)) = answer
            && let Some(path) = paths.first()
        {
            set_backup_path(path.display().to_string());
        }
    });
}

/// The system's folder dialog chooses the parent of a new data directory.
pub(crate) fn choose_data_parent() {
    task(async move {
        let answer = pick_paths(
            nsh_core::file_dialog::OpenDialog::folders()
                .title("Choose parent folder for database data"),
        )
        .await;
        let Ok(Some(paths)) = answer else {
            return;
        };
        let Some(parent) = paths.first() else {
            return;
        };
        let d = drafts();
        let name_owned = d.draft_name.get_untracked();
        let name = name_owned.trim();
        let leaf = if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            format!("{}-data", d.draft_kind.get_untracked())
        } else {
            name.to_string()
        };
        d.draft_data_directory
            .set(parent.join(leaf).display().to_string());
    });
}

/// Shows a backup in Finder.
pub(crate) fn reveal(path: &std::path::Path) {
    let path = path.to_path_buf();
    task(async move {
        let _ = reveal_path(path).await;
    });
}

pub(crate) fn backup(id: &str) {
    let database = database_name().trim().to_string();
    run_catalog_task(
        id,
        "backup",
        format!("Backing up {database}"),
        String::new(),
        move |provider, instance, cancel| {
            provider
                .backup(instance, &database, cancel)
                .map(notify_backup)
        },
    );
}

/// Turns the provider's backup path into the task's result line, as the Go
/// task showed the path `Backup` returned.
fn notify_backup(path: String) {
    workspace().update(|state| {
        if let Some(task) = state
            .tasks
            .values_mut()
            .find(|task| task.running && task.kind == "backup")
        {
            task.result = Some(path.clone());
        }
    });
}

/// Asks before restoring, then restores into a new database.
pub(crate) fn request_restore(id: &str) {
    let path = backup_path();
    if path.trim().is_empty() {
        ctrl_error("Choose a backup file to restore first.".to_string());
        return;
    }
    let target = {
        let chosen = restore_database().trim().to_string();
        if chosen.is_empty() {
            database_name().trim().to_string()
        } else {
            chosen
        }
    };
    let id_owned = id.to_string();
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let answer = alert_dialog()
        .title(format!("Restore into {target}?"))
        .description(format!(
            "Creates database {target} on {id_owned} from {name}. The source database is not changed."
        ))
        .cancel("cancel", "Cancel")
        .destructive("confirm", "Restore Backup")
        .show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if !chosen {
            return;
        }
        let database = target.clone();
        run_catalog_task(
            &id_owned,
            "restore",
            format!("Restoring into {database}"),
            format!("Restored into {database}"),
            move |provider, instance, cancel| provider.restore(instance, &database, &path, cancel),
        );
    });
}

/// Asks before deleting a finished backup, in the Go wording.
pub(crate) fn request_delete_backup(id: &str, row: BackupFile) {
    let id_owned = id.to_string();
    let when = format_modified(row.modified);
    let database = row.database.clone();
    let name = row.name.clone();
    let answer = alert_dialog()
        .title("Delete this backup?")
        .description(format!(
            "Permanently deletes the backup of {database} from {when}. The database itself is not changed."
        ))
        .cancel("cancel", "Cancel")
        .destructive("confirm", "Delete Backup")
        .show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if !chosen {
            return;
        }
        let Some(provider) = provider_of() else {
            ctrl_error("no database provider is available".to_string());
            return;
        };
        let key = format!("delete-backup:{name}");
        workspace().update(|state| {
            state.aux_busy.insert(key.clone());
            state.action_error = None;
        });
        let request = id_owned.clone();
        let file = name.clone();
        let answer = match blocking(move || {
            let cancel: CancelToken = Arc::new(AtomicBool::new(false));
            provider.delete_backup(&request, &file, cancel)
        })
        .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("the delete did not finish: {error}")),
        };
        let succeeded = answer.is_ok();
        workspace().update(|state| {
            state.aux_busy.remove(&key);
            match answer {
                Ok(()) => {
                    state.action_error = None;
                }
                Err(error) => {
                    state.action_error = Some(error);
                }
            }
        });
        refresh_backups(&id_owned);
        if succeeded {
            show_notice("Deleted backup");
        }
    });
}

/// How a backup's date reads (`2006-01-02 15:04`).
pub(crate) fn format_modified(modified: SystemTime) -> String {
    let seconds = modified
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    let minutes = seconds.div_euclid(60);
    let (year, month, day) = civil_from_days(minutes.div_euclid(1440));
    let hour = minutes.div_euclid(60).rem_euclid(24);
    let minute = minutes.rem_euclid(60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Howard Hinnant's civil-from-days, so the UI needs no date crate.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `0 B`, `12.3 MiB`, as the Go rows wrote it.
pub(crate) fn format_size(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    if bytes < 1024 {
        return format!("{} B", bytes.max(0));
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

// --- Connection profiles -----------------------------------------------------

/// The profiles of an instance, by id.
pub(crate) fn profiles(instance_id: &str) -> Vec<ConnectionProfile> {
    workspace().with(|state| {
        let mut found: Vec<ConnectionProfile> = state
            .snapshot
            .profiles
            .values()
            .filter(|profile| profile.instance_id == instance_id)
            .cloned()
            .collect();
        found.sort_by(|left, right| left.name.cmp(&right.name));
        found
    })
}

fn load_profile_for(instance_id: &str) {
    if workspace().with(|state| state.profile.instance_id != instance_id) {
        load_profile_draft(instance_id, "");
    }
}

fn load_profile_draft(instance_id: &str, profile_id: &str) {
    let saved = workspace().with(|state| {
        state
            .snapshot
            .profiles
            .get(profile_id)
            .filter(|profile| profile.instance_id == instance_id)
            .cloned()
    });
    let (draft, redis_db) = match saved {
        Some(profile) => {
            let redis = profile.redis_db.to_string();
            (profile, redis)
        }
        None => (
            ConnectionProfile {
                instance_id: instance_id.to_string(),
                ..ConnectionProfile::default()
            },
            "0".to_string(),
        ),
    };
    let d = drafts();
    d.profile_name.set(draft.name);
    d.profile_database.set(draft.database);
    d.profile_user.set(draft.username);
    d.profile_mongo_auth_source.set(draft.mongo_auth_source);
    d.profile_redis_db.set(redis_db);
    workspace().update(|state| {
        state.profile = ProfileState {
            instance_id: instance_id.to_string(),
            selected: if profile_id == "new" {
                "new".to_string()
            } else {
                draft.id.clone()
            },
            pending: None,
        };
    });
}

/// The selected connection: `""` is the administrator, `"new"` a draft.
pub(crate) fn selected_profile() -> String {
    workspace().with(|state| state.profile.selected.clone())
}

/// Chooses a profile, holding the switch back while the draft has changes.
pub(crate) fn select_profile(profile_id: &str) {
    let instance_id = workspace().with(|state| state.profile.instance_id.clone());
    if instance_id.is_empty() {
        return;
    }
    if workspace().with(|state| state.profile.selected == profile_id) {
        return;
    }
    if profile_dirty() {
        workspace().update(|state| {
            state.profile.pending = Some(PendingSwitch::Profile(profile_id.to_string()));
        });
        return;
    }
    load_profile_draft(&instance_id, profile_id);
}

/// Opens the profile editor on a fresh draft for the instance shown.
pub(crate) fn add_profile(instance_id: &str) {
    select_profile_for(instance_id, "new");
}

fn select_profile_for(instance_id: &str, profile_id: &str) {
    if profile_dirty() {
        workspace().update(|state| {
            state.profile.pending = Some(PendingSwitch::Profile(profile_id.to_string()));
        });
        return;
    }
    load_profile_draft(instance_id, profile_id);
}

/// Whether the draft differs from the saved profile (or is a used "new").
pub(crate) fn profile_dirty() -> bool {
    let (instance_id, selected, saved) = workspace().with(|state| {
        let selected = state.profile.selected.clone();
        let saved = if selected.is_empty() || selected == "new" {
            None
        } else {
            state.snapshot.profiles.get(&selected).cloned()
        };
        (state.profile.instance_id.clone(), selected, saved)
    });
    if instance_id.is_empty() || selected.is_empty() {
        return false;
    }
    let d = drafts();
    let name = d.profile_name.get();
    let database = d.profile_database.get();
    let username = d.profile_user.get();
    let mongo_auth_source = d.profile_mongo_auth_source.get();
    let redis = d.profile_redis_db.get().trim().parse::<i64>().ok();
    if selected == "new" {
        return !name.is_empty()
            || !database.is_empty()
            || !username.is_empty()
            || !mongo_auth_source.is_empty()
            || redis.unwrap_or(0) != 0;
    }
    match saved {
        Some(saved) => {
            name != saved.name
                || database != saved.database
                || username != saved.username
                || mongo_auth_source != saved.mongo_auth_source
                || redis != Some(saved.redis_db)
        }
        None => true,
    }
}

pub(crate) fn profile_name() -> String {
    drafts().profile_name.get()
}

pub(crate) fn set_profile_name(next: String) {
    drafts().profile_name.set(next);
}

pub(crate) fn profile_database() -> String {
    drafts().profile_database.get()
}

pub(crate) fn set_profile_database(next: String) {
    drafts().profile_database.set(next);
}

pub(crate) fn profile_user() -> String {
    drafts().profile_user.get()
}

pub(crate) fn set_profile_user(next: String) {
    drafts().profile_user.set(next);
}

pub(crate) fn profile_mongo_auth_source() -> String {
    drafts().profile_mongo_auth_source.get()
}

pub(crate) fn set_profile_mongo_auth_source(next: String) {
    drafts().profile_mongo_auth_source.set(next);
}

pub(crate) fn profile_redis_db() -> String {
    drafts().profile_redis_db.get()
}

pub(crate) fn set_profile_redis_db(next: String) {
    drafts().profile_redis_db.set(next);
}

/// Saves the profile draft. The profile never carries a credential.
pub(crate) fn save_profile(id: &str) {
    if operations_busy() {
        return;
    }
    let selected = selected_profile();
    let is_redis = instance(id).is_some_and(|instance| instance.kind == "redis");
    let d = drafts();
    let prepared = (|| {
        let mut profile = ConnectionProfile {
            id: if selected == "new" {
                String::new()
            } else {
                selected
            },
            instance_id: id.to_string(),
            name: d.profile_name.get_untracked(),
            database: d.profile_database.get_untracked(),
            username: d.profile_user.get_untracked(),
            mongo_auth_source: d.profile_mongo_auth_source.get_untracked(),
            redis_db: 0,
        };
        if is_redis {
            match d.profile_redis_db.get_untracked().trim().parse::<i64>() {
                Ok(value) => profile.redis_db = value,
                Err(_) => return Err("Enter a Redis database number from 0 to 15".to_string()),
            }
        }
        Ok(profile)
    })();
    let profile = match prepared {
        Ok(profile) => profile,
        Err(message) => {
            ctrl_error(message);
            return;
        }
    };
    let Some(provider) = provider_of() else {
        ctrl_error("no database provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.aux_busy.insert("profile-save".to_string());
        state.action_error = None;
    });
    let id_owned = id.to_string();
    task(async move {
        let answer = match blocking(move || provider.save_profile(profile)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the profile was not saved: {error}")),
        };
        workspace().update(|state| {
            state.aux_busy.remove("profile-save");
        });
        match answer {
            Ok(saved) => {
                let d = drafts();
                d.profile_name.set(saved.name);
                d.profile_database.set(saved.database);
                d.profile_user.set(saved.username);
                d.profile_mongo_auth_source.set(saved.mongo_auth_source);
                d.profile_redis_db.set(saved.redis_db.to_string());
                show_notice("Connection profile saved");
                workspace().update(|state| {
                    state.profile.selected = saved.id;
                });
            }
            Err(error) => ctrl_error(error),
        }
        refresh();
        load_profile_for(&id_owned);
    });
}

/// Deletes the selected profile; the database and its user remain.
pub(crate) fn delete_profile(id: &str) {
    let profile_id = selected_profile();
    if profile_id.is_empty() || profile_id == "new" {
        return;
    }
    let Some(provider) = provider_of() else {
        ctrl_error("no database provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.aux_busy.insert("profile-delete".to_string());
        state.action_error = None;
    });
    let id_owned = id.to_string();
    let delete_id = profile_id.clone();
    let id_for_call = id_owned.clone();
    task(async move {
        let request = delete_id.clone();
        let answer = match blocking(move || provider.delete_profile(&id_for_call, &request)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the profile was not deleted: {error}")),
        };
        workspace().update(|state| {
            state.aux_busy.remove("profile-delete");
        });
        match answer {
            Ok(()) => show_notice("Connection profile deleted; database and user remain"),
            Err(error) => ctrl_error(error),
        }
        refresh();
        load_profile_for(&id_owned);
        load_profile_draft(&id_owned, "");
    });
}

/// The profile the copy actions use for `instance_id`: the administrator when
/// none is chosen or when acting on a different instance from the rail.
fn copy_profile_for(instance_id: &str) -> String {
    workspace().with(|state| {
        if state.profile.instance_id != instance_id {
            return String::new();
        }
        let selected = state.profile.selected.clone();
        if selected == "new" {
            String::new()
        } else {
            selected
        }
    })
}

fn selected_database_for(instance_id: &str, kind: &str) -> Option<String> {
    if selected_id().as_deref() != Some(instance_id) {
        return None;
    }
    let name = drafts().database_name.get_untracked().trim().to_string();
    if name.is_empty() {
        return None;
    }
    if kind == "redis" {
        name.parse::<i64>()
            .ok()
            .filter(|value| (0..=15).contains(value))
            .map(|value| value.to_string())
    } else {
        Some(name)
    }
}

/// Copies the credential-free URI of the chosen connection.
pub(crate) fn copy_uri(id: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let profile = copy_profile_for(id);
    let id_owned = id.to_string();
    task(async move {
        let answer = match blocking(move || provider.connection_info(&id_owned, &profile)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the connection did not answer: {error}")),
        };
        match answer {
            Ok(info) => {
                set_clipboard(info.uri);
                show_info_notice("Connection URI copied");
            }
            Err(error) => ctrl_error(error),
        }
    });
}

/// Copies the `.env` assignment, with the database override the form holds
/// for the administrator connection.
pub(crate) fn copy_env(id: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let profile = copy_profile_for(id);
    let kind = instance(id)
        .map(|instance| instance.kind)
        .unwrap_or_default();
    let database = selected_database_for(id, &kind);
    let id_owned = id.to_string();
    let profile_for_call = profile.clone();
    task(async move {
        let answer =
            match blocking(move || provider.connection_info(&id_owned, &profile_for_call)).await {
                Ok(answer) => answer,
                Err(error) => Err(format!("the connection did not answer: {error}")),
            };
        match answer {
            Ok(info) => {
                let info = match (profile.is_empty(), database) {
                    (true, Some(database)) => info.with_database(&kind, &database),
                    _ => info,
                };
                set_clipboard(info.env_assignment_for_kind(&kind));
                show_info_notice(".env connection copied");
            }
            Err(error) => ctrl_error(error),
        }
    });
}

/// Copies the credential-free JDBC URL for PostgreSQL and MySQL.
pub(crate) fn copy_jdbc(id: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let profile = copy_profile_for(id);
    let kind = instance(id)
        .map(|instance| instance.kind)
        .unwrap_or_default();
    let database = selected_database_for(id, &kind);
    let id_owned = id.to_string();
    let profile_for_call = profile.clone();
    task(async move {
        let answer =
            match blocking(move || provider.connection_info(&id_owned, &profile_for_call)).await {
                Ok(answer) => answer,
                Err(error) => Err(format!("the connection did not answer: {error}")),
            };
        let answer = answer.and_then(|info| {
            let info = match (profile.is_empty(), database) {
                (true, Some(database)) => info.with_database(&kind, &database),
                _ => info,
            };
            info.jdbc_url(&kind).map_err(|error| error.to_string())
        });
        match answer {
            Ok(jdbc) => {
                set_clipboard(jdbc);
                show_info_notice("JDBC URL copied");
            }
            Err(error) => ctrl_error(error),
        }
    });
}

/// Copies the administrator password by reading its 0600 file in the task.
pub(crate) fn copy_password(id: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let id_owned = id.to_string();
    task(async move {
        let request = id_owned.clone();
        let answer = match blocking(move || provider.password(&request)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the credential was not read: {error}")),
        };
        match answer {
            Ok(password) => {
                set_clipboard(password);
                show_info_notice("Database password copied");
            }
            Err(error) => ctrl_error(error),
        }
    });
}

/// Opens the URI in the default client.
pub(crate) fn open_client(id: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let profile = copy_profile_for(id);
    let id_owned = id.to_string();
    let call_provider = Arc::clone(&provider);
    task(async move {
        let answer =
            match blocking(move || call_provider.connection_info(&id_owned, &profile)).await {
                Ok(answer) => answer,
                Err(error) => Err(format!("the connection did not answer: {error}")),
            };
        let answer = answer.and_then(|info| provider.open_client(&info.uri));
        match answer {
            Ok(()) => show_info_notice("Opening the default client"),
            Err(error) => ctrl_error(error),
        }
    });
}

/// Opens the engine's CLI in Terminal. devcore's launcher reads the secret
/// inside the shell script; the store only passes the profile choice.
pub(crate) fn open_cli(id: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let profile = copy_profile_for(id);
    let kind = instance(id)
        .map(|instance| instance.kind)
        .unwrap_or_default();
    let database = selected_database_for(id, &kind);
    let id_owned = id.to_string();
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let answer = match blocking(move || {
            provider.open_cli(&id_owned, &profile, database.as_deref(), cancel)
        })
        .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("the Terminal launch did not finish: {error}")),
        };
        match answer {
            Ok(()) => show_info_notice("Opening CLI in Terminal"),
            Err(error) => ctrl_error(error),
        }
    });
}

/// Reads the administrator's credential-free connection block.
pub(crate) fn show_connection_details(id: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let id_owned = id.to_string();
    workspace().update(|state| {
        state.aux_busy.insert("connection-details".to_string());
    });
    task(async move {
        let request = id_owned.clone();
        let answer = match blocking(move || provider.connection_details(&request)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the connection did not answer: {error}")),
        };
        workspace().update(|state| {
            state.aux_busy.remove("connection-details");
            match answer {
                Ok(text) => {
                    state.connection_details = Some(text);
                    state.action_error = None;
                }
                Err(error) => state.action_error = Some(error),
            }
        });
    });
}

pub(crate) fn connection_details() -> Option<String> {
    workspace().with(|state| state.connection_details.clone())
}

// --- Upgrades ----------------------------------------------------------------

/// The newer installed patch the instance may move to.
pub(crate) fn upgrade_target(id: &str) -> Option<String> {
    workspace().with(|state| {
        state
            .snapshot
            .upgrades
            .get(id)
            .and_then(|targets| targets.first().cloned())
    })
}

/// Asks before moving a stopped instance to a newer patch.
pub(crate) fn request_upgrade(id: &str) {
    let Some(instance) = instance(id) else {
        return;
    };
    let Some(target) = upgrade_target(id) else {
        return;
    };
    let version = package_version(&target);
    let current = package_version(&instance.package);
    let title = SoftwareKind::from_key(&instance.kind)
        .map(SoftwareKind::title)
        .unwrap_or(instance.kind.as_str())
        .to_string();
    let id_owned = id.to_string();
    let answer = alert_dialog()
        .title(format!("Upgrade {id_owned} to {title} {version}?"))
        .description(format!(
            "The instance will use the newer server the next time it starts. Its data is not copied or deleted, and no backup is taken: back up the databases you need before continuing. After the newer server has started, going back to {current} is not supported."
        ))
        .cancel("cancel", "Cancel")
        .destructive("confirm", "Upgrade Instance")
        .show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if !chosen {
            return;
        }
        let Some(provider) = provider_of() else {
            return;
        };
        start_task(
            &id_owned,
            "upgrade",
            format!("Upgrading {id_owned} to {version}"),
        );
        let target = target.clone();
        let version_owned = version.clone();
        let id_task = id_owned.clone();
        let instance = instance.clone();
        task(async move {
            let cancel: CancelToken = Arc::new(AtomicBool::new(false));
            register_cancel(&id_task, cancel.clone());
            let answer = match blocking(move || provider.upgrade(&instance, &target, cancel)).await
            {
                Ok(answer) => answer,
                Err(error) => Err(format!("the upgrade did not finish: {error}")),
            };
            let result = match answer {
                Ok(()) => Ok(format!(
                    "Upgraded {id_task} to {version_owned}. Start the instance to use it."
                )),
                Err(error) => Err(error),
            };
            finish_task(&id_task, "upgrade", result);
            refresh();
        });
    });
}

// --- Permanent deletion ------------------------------------------------------

pub(crate) fn confirmation() -> String {
    drafts().confirmation.get()
}

pub(crate) fn set_confirmation(next: String) {
    drafts().confirmation.set(next);
}

/// Opens the permanent-deletion review of an instance.
pub(crate) fn open_deletion_preview(id: &str) {
    drafts().confirmation.set(String::new());
    workspace().update(|state| {
        state.mode = Mode::DeletionPreview;
        state.selected = Some(id.to_string());
        state.preview = Some(PreviewState {
            instance_id: id.to_string(),
            preview: None,
            loading: true,
            error: None,
            sequence: state.sequence,
        });
        state.action_error = None;
    });
    refresh_deletion_preview();
}

/// Reads the review again.
pub(crate) fn refresh_deletion_preview() {
    let Some(provider) = provider_of() else {
        return;
    };
    let Some(id) = selected_id() else {
        return;
    };
    attach_sink();
    let sequence = next_sequence();
    workspace().update(|state| {
        let Some(preview) = state.preview.as_mut() else {
            return;
        };
        if preview.instance_id != id {
            return;
        }
        preview.sequence = sequence;
        preview.loading = true;
        preview.error = None;
    });
    task(async move {
        let request = id.clone();
        let answer = match blocking(move || provider.preview_deletion(&request)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the deletion preview did not answer: {error}")),
        };
        workspace().update(|state| {
            let Some(preview) = state.preview.as_mut() else {
                return;
            };
            if preview.instance_id != id || preview.sequence != sequence {
                return;
            }
            preview.loading = false;
            match answer {
                Ok(read) => {
                    preview.preview = Some(read);
                    preview.error = None;
                }
                Err(error) => preview.error = Some(error),
            }
        });
    });
}

pub(crate) fn deletion_preview() -> Option<InstanceDeletionPreview> {
    workspace().with(|state| {
        state
            .preview
            .as_ref()
            .and_then(|preview| preview.preview.clone())
    })
}

pub(crate) fn deletion_preview_loading() -> bool {
    workspace().with(|state| {
        state
            .preview
            .as_ref()
            .is_some_and(|preview| preview.loading)
    })
}

pub(crate) fn deletion_preview_error() -> Option<String> {
    workspace().with(|state| {
        state
            .preview
            .as_ref()
            .and_then(|preview| preview.error.clone())
    })
}

/// Leaves the review without changing anything.
pub(crate) fn cancel_deletion_preview() {
    let id = selected_id();
    drafts().confirmation.set(String::new());
    workspace().update(|state| {
        state.preview = None;
        state.mode = if id.is_some() {
            Mode::Instance
        } else {
            Mode::New
        };
    });
}

/// The typed ID still has to match, the review still has to allow it, and no
/// other operation may be running.
pub(crate) fn can_delete() -> bool {
    let typed = drafts().confirmation.get();
    workspace().with(|state| {
        let Some(preview) = state
            .preview
            .as_ref()
            .and_then(|preview| preview.preview.as_ref())
        else {
            return false;
        };
        preview.can_delete
            && typed.trim() == preview.instance_id
            && !state.tasks.values().any(|task| task.running)
            && state.aux_busy.is_empty()
    })
}

/// Erases the reviewed internal data after the typed confirmation.
pub(crate) fn delete_instance_data() {
    if !can_delete() {
        return;
    }
    let Some(provider) = provider_of() else {
        return;
    };
    let Some(preview) = deletion_preview() else {
        return;
    };
    let id = preview.instance_id.clone();
    start_task(&id, "delete", format!("Permanently deleting data for {id}"));
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        register_cancel(&id, cancel.clone());
        let request = id.clone();
        let fingerprint = preview.fingerprint.clone();
        let confirmation = preview.instance_id.clone();
        let answer = match blocking(move || {
            provider.delete_instance_data(&request, &fingerprint, &confirmation, cancel)
        })
        .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("the deletion did not finish: {error}")),
        };
        let result = match answer {
            Ok(()) => Ok("Database deletion status refreshed.".to_string()),
            Err(error) => Err(format!(
                "Database deletion stopped. Review its current state before continuing.\nError: {error}"
            )),
        };
        finish_task(&id, "delete", result);
        drafts().confirmation.set(String::new());
        refresh();
        load_pending_deletions();
        later(Duration::from_millis(300), move || {
            workspace().update(|state| {
                let exists = state
                    .snapshot
                    .instances
                    .iter()
                    .any(|instance| instance.id == id);
                if !exists {
                    state.mode = Mode::New;
                    state.selected = None;
                    state.preview = None;
                    state.profile = ProfileState::default();
                } else {
                    state.mode = Mode::Deletions;
                    state.preview = None;
                }
            });
        });
    });
}

// --- Staged deletions --------------------------------------------------------

/// Opens the staged-deletion list.
pub(crate) fn open_deletions() {
    drafts().confirmation.set(String::new());
    workspace().update(|state| {
        state.mode = Mode::Deletions;
        state.preview = None;
        state.action_error = None;
    });
    load_pending_deletions();
}

pub(crate) fn refresh_deletions() {
    load_pending_deletions();
}

fn load_pending_deletions() {
    let Some(provider) = provider_of() else {
        return;
    };
    let sequence = workspace().with(|state| state.deletions.sequence + 1);
    workspace().update(|state| {
        state.deletions.sequence = sequence;
        state.deletions.loading = true;
        state.deletions.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.pending_deletions()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the deletion list did not answer: {error}")),
        };
        workspace().update(|state| {
            if state.deletions.sequence != sequence {
                return;
            }
            state.deletions.loading = false;
            state.deletions.loaded = true;
            match answer {
                Ok(rows) => {
                    state.deletions.items = rows;
                    state.deletions.error = None;
                }
                Err(error) => state.deletions.error = Some(error),
            }
        });
    });
}

pub(crate) fn pending_deletions() -> Vec<InstanceDeletionInfo> {
    workspace().with(|state| state.deletions.items.clone())
}

pub(crate) fn pending_deletions_error() -> Option<String> {
    workspace().with(|state| state.deletions.error.clone())
}

/// Restores a staged deletion when removal had not committed.
pub(crate) fn restore_deletion(id: &str, revision: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let key = format!("restore-deletion:{id}");
    let id_owned = id.to_string();
    let revision = revision.to_string();
    workspace().update(|state| {
        state.aux_busy.insert(key.clone());
        state.action_error = None;
    });
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let answer =
            match blocking(move || provider.restore_deletion(&id_owned, &revision, cancel)).await {
                Ok(answer) => answer,
                Err(error) => Err(format!("the restore did not finish: {error}")),
            };
        workspace().update(|state| {
            state.aux_busy.remove(&key);
        });
        match answer {
            Ok(()) => {
                show_notice("Database deletion status refreshed.");
                load_pending_deletions();
                refresh();
            }
            Err(error) => ctrl_error(error),
        }
    });
}

/// Erases the staged data of a committed deletion after the typed ID.
pub(crate) fn finish_deletion(id: &str, revision: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let confirmation = confirmation();
    let key = format!("finish-deletion:{id}");
    let id_owned = id.to_string();
    let revision = revision.to_string();
    workspace().update(|state| {
        state.aux_busy.insert(key.clone());
        state.action_error = None;
    });
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let answer = match blocking(move || {
            provider.finish_deletion(&id_owned, &revision, &confirmation, cancel)
        })
        .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("the deletion did not finish: {error}")),
        };
        drafts().confirmation.set(String::new());
        workspace().update(|state| {
            state.aux_busy.remove(&key);
        });
        match answer {
            Ok(()) => {
                show_notice("Database deletion status refreshed.");
                load_pending_deletions();
                refresh();
            }
            Err(error) => ctrl_error(error),
        }
    });
}

// --- Unfinished preparations and tasks ---------------------------------------

pub(crate) fn open_initializations() {
    drafts().confirmation.set(String::new());
    workspace().update(|state| {
        state.mode = Mode::Initializations;
        state.preview = None;
        state.action_error = None;
    });
    load_initializations();
}

pub(crate) fn refresh_initializations() {
    load_initializations();
}

fn load_initializations() {
    let Some(provider) = provider_of() else {
        return;
    };
    let sequence = workspace().with(|state| state.initializations.sequence + 1);
    workspace().update(|state| {
        state.initializations.sequence = sequence;
        state.initializations.loading = true;
        state.initializations.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.pending_initializations()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the initialization list did not answer: {error}")),
        };
        workspace().update(|state| {
            if state.initializations.sequence != sequence {
                return;
            }
            state.initializations.loading = false;
            state.initializations.loaded = true;
            match answer {
                Ok(rows) => {
                    state.initializations.items = rows;
                    state.initializations.error = None;
                }
                Err(error) => state.initializations.error = Some(error),
            }
        });
    });
}

pub(crate) fn pending_initializations() -> Vec<InstanceInitializationInfo> {
    workspace().with(|state| state.initializations.items.clone())
}

pub(crate) fn pending_initializations_error() -> Option<String> {
    workspace().with(|state| state.initializations.error.clone())
}

/// Stops recorded jobs and archives the prepared data; nothing is deleted.
pub(crate) fn resolve_initialization(id: &str, revision: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let key = format!("resolve-initialization:{id}");
    let id_owned = id.to_string();
    let revision = revision.to_string();
    workspace().update(|state| {
        state.aux_busy.insert(key.clone());
        state.action_error = None;
    });
    task(async move {
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let answer =
            match blocking(move || provider.resolve_initialization(&id_owned, &revision, cancel))
                .await
            {
                Ok(answer) => answer,
                Err(error) => Err(format!("the initialization was not reconciled: {error}")),
            };
        workspace().update(|state| {
            state.aux_busy.remove(&key);
        });
        match answer {
            Ok(archive) if !archive.is_empty() => show_notice(format!(
                "Initialization archived; files retained at {archive}"
            )),
            Ok(_) => show_notice("Initialization record reconciled. Registered data was kept."),
            Err(error) => ctrl_error(error),
        }
        load_initializations();
        refresh();
    });
}

pub(crate) fn open_work() {
    drafts().confirmation.set(String::new());
    workspace().update(|state| {
        state.mode = Mode::Work;
        state.preview = None;
        state.action_error = None;
    });
    load_work();
}

pub(crate) fn refresh_work() {
    load_work();
}

fn load_work() {
    let Some(provider) = provider_of() else {
        return;
    };
    let sequence = workspace().with(|state| state.work.sequence + 1);
    workspace().update(|state| {
        state.work.sequence = sequence;
        state.work.loading = true;
        state.work.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.pending_work()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the database task list did not answer: {error}")),
        };
        workspace().update(|state| {
            if state.work.sequence != sequence {
                return;
            }
            state.work.loading = false;
            state.work.loaded = true;
            match answer {
                Ok(rows) => {
                    state.work.items = rows;
                    state.work.error = None;
                }
                Err(error) => state.work.error = Some(error),
            }
        });
    });
}

pub(crate) fn database_work() -> Vec<DatabaseWorkInfo> {
    workspace().with(|state| state.work.items.clone())
}

pub(crate) fn database_work_error() -> Option<String> {
    workspace().with(|state| state.work.error.clone())
}

/// Releases an unfinished task's reservation; files already written and the
/// backup output stay.
pub(crate) fn resolve_work(id: &str, revision: &str) {
    let Some(provider) = provider_of() else {
        return;
    };
    let key = format!("resolve-work:{id}");
    let id_owned = id.to_string();
    let revision = revision.to_string();
    workspace().update(|state| {
        state.aux_busy.insert(key.clone());
        state.action_error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.resolve_work(&id_owned, &revision)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the task was not resolved: {error}")),
        };
        workspace().update(|state| {
            state.aux_busy.remove(&key);
        });
        match answer {
            Ok(()) => show_notice("Database task status refreshed."),
            Err(error) => ctrl_error(error),
        }
        load_work();
        refresh();
    });
}

/// Returns from a settings list to the instance or the form.
pub(crate) fn back_to_settings() {
    let selected = selected_id();
    drafts().confirmation.set(String::new());
    workspace().update(|state| {
        state.preview = None;
        state.mode = if selected.is_some() {
            Mode::Instance
        } else {
            Mode::New
        };
    });
}

// --- Formatting --------------------------------------------------------------

/// The first eight characters of an id, as the Go lists label rows.
pub(crate) fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}
