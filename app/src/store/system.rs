//! Diagnostics, ports, tasks and Settings, ported from `view_diagnostics.go`,
//! `view_ports.go`, `view_operations.go`, `view_settings.go`, `view_quit.go`
//! and `settings.go`.
//!
//! devcore owns every observation and mutation: `diagnostics` for the runtime
//! snapshot, diagnosis and redacted export, `operations` for the journal,
//! `databases::ports` for the port inventory, `helper` for the privileged
//! helper client, `launchd` for the login item and devcli's wrapper for the
//! public CLI. Every call here runs on a blocking thread and writes the store
//! when it answers; render code only reads the store.
//!
//! The helper's one administrator prompt lives inside devcore's injected
//! `AdminRunner`, never here: this module starts an install and shows its busy
//! state, status message and verbatim error.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use devcore::{
    databases::{DatabaseActions, Databases, ListeningPort, PortInspection},
    diagnostics::{DiagnosticReport, Diagnostics},
    engine::{Engine, SystemCommandRunner},
    helper::{Client as HelperClient, HelperState as CoreHelperState},
    launchd::{
        SystemHost, SystemRunner, disable_launch_at_login, enable_launch_at_login,
        launch_at_login_enabled, resolve_login_executable,
    },
    operations::{OperationJournal, OperationRecord},
    shutdown::{Shutdown, ShutdownPlan},
    software::{Software, cache::DownloadCacheInfo},
    state::Store,
};
use nsh_core::prelude::*;

use crate::{
    store::{
        Section,
        software::{format_bytes, plural},
    },
    theme as tokens,
};

/// How a helper install ended: the freshly read status, and the error, if any.
/// The status is refreshed on failure exactly as `settings.go:194-208` does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelperChange {
    pub status: HelperStatus,
    pub error: Option<String>,
}

/// How a CLI wrapper change ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliChange {
    pub status: CliStatus,
    pub error: Option<String>,
}

/// How a login-item change ended; `enabled` is the observed state afterwards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginChange {
    pub enabled: bool,
    pub error: Option<String>,
}

/// What clearing the download cache removed and what remains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheClear {
    pub removed: CacheInfo,
    pub current: CacheInfo,
}

/// The privileged helper's passive state, as the Settings card shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelperStatus {
    pub installed: bool,
    pub message: String,
}

/// The managed `devbase` wrapper's status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliStatus {
    pub installed: bool,
    pub wrapper_path: String,
    pub target_executable: String,
    pub in_path: bool,
    pub managed_by_devbase: bool,
}

impl From<devcli::wrapper::CLIInstallStatus> for CliStatus {
    fn from(status: devcli::wrapper::CLIInstallStatus) -> Self {
        CliStatus {
            installed: status.installed,
            wrapper_path: status.wrapper_path,
            target_executable: status.target_executable,
            in_path: status.in_path,
            managed_by_devbase: status.managed_by_devbase,
        }
    }
}

/// The download cache's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheInfo {
    pub directory: String,
    pub files: i64,
    pub bytes: i64,
}

impl From<DownloadCacheInfo> for CacheInfo {
    fn from(info: DownloadCacheInfo) -> Self {
        CacheInfo {
            directory: info.directory.to_string_lossy().into_owned(),
            files: info.files,
            bytes: info.bytes,
        }
    }
}

/// Everything the Settings screen reads in one pass (`refreshSettings`).
#[derive(Debug, Clone, Default)]
pub struct SettingsSnapshot {
    pub helper: HelperStatus,
    pub cli: CliStatus,
    pub cache: CacheInfo,
    pub log_rotation_enabled: bool,
    pub login_at_login: bool,
    pub workspace_root: String,
}

// --- Providers ---------------------------------------------------------------

/// The diagnostics half of the system facade.
pub trait DiagnosticsProvider: Send + Sync {
    /// `instance:<id>`, `proxy`, `tunnel` and `tunnel:token` states.
    fn snapshot(&self) -> Result<BTreeMap<String, String>, String>;
    fn diagnose(&self) -> Result<DiagnosticReport, String>;
    fn export(&self, report: &DiagnosticReport) -> Result<PathBuf, String>;
    fn log_rotation_disabled(&self) -> Result<bool, String>;
    fn set_log_rotation(&self, enabled: bool) -> Result<(), String>;
}

/// The cross-process task journal.
pub trait OperationsProvider: Send + Sync {
    fn operations(&self) -> Result<Vec<OperationRecord>, String>;
    fn retry(&self, id: &str, revision: &str) -> Result<(), String>;
    fn dismiss(&self, id: &str, revision: &str) -> Result<(), String>;
}

/// Port inspection and the listening-port inventory.
pub trait PortsProvider: Send + Sync {
    fn inspect(&self, port: i64) -> Result<PortInspection, String>;
    fn available_local_port(&self) -> Result<i64, String>;
    fn listening(&self) -> Result<Vec<ListeningPort>, String>;
}

/// Helper, CLI wrapper, cache, log rotation, login item and quit scope.
pub trait SettingsProvider: Send + Sync {
    fn snapshot(&self) -> Result<SettingsSnapshot, String>;
    fn install_helper(&self) -> HelperChange;
    fn uninstall_helper(&self) -> HelperChange;
    fn install_cli(&self) -> CliChange;
    fn uninstall_cli(&self) -> CliChange;
    fn clear_cache(&self) -> Result<CacheClear, String>;
    fn set_log_rotation(&self, enabled: bool) -> Result<(), String>;
    fn set_login_item(&self, enabled: bool) -> LoginChange;
    fn open_full_disk_access(&self) -> Result<(), String>;
    fn open_login_items(&self) -> Result<(), String>;
    fn reveal(&self, path: &str) -> Result<(), String>;
    fn preview_shutdown(&self) -> Result<ShutdownPlan, String>;
    fn shutdown(&self, plan: &ShutdownPlan) -> Result<(), String>;
}

// The tests' registry, mirroring the software store's: one fake mode for all
// four facets, so a headless surface never reaches the real system.
static FAKE_MODE: AtomicBool = AtomicBool::new(false);
static DIAGNOSTICS_FAKE: LazyLock<Mutex<Option<Arc<dyn DiagnosticsProvider>>>> =
    LazyLock::new(|| Mutex::new(None));
static OPERATIONS_FAKE: LazyLock<Mutex<Option<Arc<dyn OperationsProvider>>>> =
    LazyLock::new(|| Mutex::new(None));
static PORTS_FAKE: LazyLock<Mutex<Option<Arc<dyn PortsProvider>>>> =
    LazyLock::new(|| Mutex::new(None));
static SETTINGS_FAKE: LazyLock<Mutex<Option<Arc<dyn SettingsProvider>>>> =
    LazyLock::new(|| Mutex::new(None));

fn take<T: ?Sized>(slot: &'static Mutex<Option<Arc<T>>>) -> Option<Arc<T>> {
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Makes every facet use test providers instead of devcore. Tests only.
pub fn use_fake_provider() {
    FAKE_MODE.store(true, Ordering::Relaxed);
}

pub(crate) fn is_fake_mode() -> bool {
    FAKE_MODE.load(Ordering::Relaxed)
}

/// Gives the diagnostics surface a provider of the test's own. Tests only.
pub fn install_diagnostics_provider(provider: Arc<dyn DiagnosticsProvider>) {
    use_fake_provider();
    *DIAGNOSTICS_FAKE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

/// Gives the journal a provider of the test's own. Tests only.
pub fn install_operations_provider(provider: Arc<dyn OperationsProvider>) {
    use_fake_provider();
    *OPERATIONS_FAKE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

/// Gives the port cards a provider of the test's own. Tests only.
pub fn install_ports_provider(provider: Arc<dyn PortsProvider>) {
    use_fake_provider();
    *PORTS_FAKE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

/// Gives the Settings screen a provider of the test's own. Tests only.
pub fn install_settings_provider(provider: Arc<dyn SettingsProvider>) {
    use_fake_provider();
    *SETTINGS_FAKE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

// --- Production facade -------------------------------------------------------

#[derive(Clone)]
struct Facade {
    store: Store,
}

impl Facade {
    fn new() -> Self {
        Facade {
            store: Store::new(devcore::state::default_root()),
        }
    }

    fn databases(&self) -> Databases {
        Databases::new(self.store.clone())
    }

    fn diagnostics(&self) -> Diagnostics {
        Diagnostics::new(
            self.store.clone(),
            Engine::new(self.store.root(), Arc::new(SystemCommandRunner::new())),
            Software::new(self.store.clone()),
        )
    }

    fn journal(&self) -> OperationJournal {
        DatabaseActions::new(self.databases()).operation_journal()
    }

    fn app_path(&self) -> String {
        current_app_bundle()
    }

    fn helper_status(&self) -> HelperStatus {
        let status = HelperClient::default_client().status();
        HelperStatus {
            installed: status.state == CoreHelperState::Installed,
            message: status.message,
        }
    }

    fn cli_status(&self) -> CliStatus {
        let dir = devcli::wrapper::default_cli_bin_dir()
            .to_string_lossy()
            .into_owned();
        devcli::wrapper::cli_status(&dir, &self.app_path(), "").into()
    }
}

impl DiagnosticsProvider for Facade {
    fn snapshot(&self) -> Result<BTreeMap<String, String>, String> {
        let state = self.store.read().map_err(|error| error.to_string())?;
        Ok(self.diagnostics().snapshot(&state).states)
    }

    fn diagnose(&self) -> Result<DiagnosticReport, String> {
        self.diagnostics()
            .diagnose()
            .map_err(|error| error.to_string())
    }

    fn export(&self, report: &DiagnosticReport) -> Result<PathBuf, String> {
        self.diagnostics()
            .export_diagnostics(report)
            .map_err(|error| error.to_string())
    }

    fn log_rotation_disabled(&self) -> Result<bool, String> {
        self.diagnostics()
            .log_rotation_disabled()
            .map_err(|error| error.to_string())
    }

    fn set_log_rotation(&self, enabled: bool) -> Result<(), String> {
        self.diagnostics()
            .set_log_rotation(enabled)
            .map_err(|error| error.to_string())
    }
}

impl OperationsProvider for Facade {
    fn operations(&self) -> Result<Vec<OperationRecord>, String> {
        self.journal()
            .operations(&|| false)
            .map_err(|error| error.to_string())
    }

    fn retry(&self, id: &str, revision: &str) -> Result<(), String> {
        self.journal()
            .retry_operation(id, revision, &|| false)
            .map_err(|error| error.to_string())
    }

    fn dismiss(&self, id: &str, revision: &str) -> Result<(), String> {
        self.journal()
            .dismiss_operation(id, revision, &|| false)
            .map_err(|error| error.to_string())
    }
}

impl PortsProvider for Facade {
    fn inspect(&self, port: i64) -> Result<PortInspection, String> {
        self.databases()
            .inspect_port(port, &|| false)
            .map_err(|error| error.to_string())
    }

    fn available_local_port(&self) -> Result<i64, String> {
        self.databases()
            .available_local_port(&|| false)
            .map_err(|error| error.to_string())
    }

    fn listening(&self) -> Result<Vec<ListeningPort>, String> {
        self.databases()
            .listening_user_ports(&|| false)
            .map_err(|error| error.to_string())
    }
}

impl SettingsProvider for Facade {
    fn snapshot(&self) -> Result<SettingsSnapshot, String> {
        let software = Software::new(self.store.clone());
        let cache = software
            .download_cache_info()
            .map_err(|error| error.to_string())?;
        let log_rotation_enabled = !self
            .diagnostics()
            .log_rotation_disabled()
            .map_err(|error| error.to_string())?;
        Ok(SettingsSnapshot {
            helper: self.helper_status(),
            cli: self.cli_status(),
            cache: cache.into(),
            log_rotation_enabled,
            login_at_login: launch_at_login_enabled(&default_login_agent_dir()),
            workspace_root: self.store.root().to_string_lossy().into_owned(),
        })
    }

    fn install_helper(&self) -> HelperChange {
        let result = HelperClient::default_client()
            .install()
            .map_err(|error| error.to_string());
        HelperChange {
            status: self.helper_status(),
            error: result.err(),
        }
    }

    fn uninstall_helper(&self) -> HelperChange {
        let result = HelperClient::default_client()
            .uninstall()
            .map_err(|error| error.to_string());
        HelperChange {
            status: self.helper_status(),
            error: result.err(),
        }
    }

    fn install_cli(&self) -> CliChange {
        let dir = devcli::wrapper::default_cli_bin_dir()
            .to_string_lossy()
            .into_owned();
        let app = self.app_path();
        match devcli::wrapper::install_cli_wrapper(&dir, &app, "") {
            Ok(status) => CliChange {
                status: status.into(),
                error: None,
            },
            Err(error) => CliChange {
                status: devcli::wrapper::cli_status(&dir, &app, "").into(),
                error: Some(error.message().to_string()),
            },
        }
    }

    fn uninstall_cli(&self) -> CliChange {
        let dir = devcli::wrapper::default_cli_bin_dir()
            .to_string_lossy()
            .into_owned();
        let result = devcli::wrapper::uninstall_cli_wrapper(&dir);
        CliChange {
            status: self.cli_status(),
            error: result.err().map(|error| error.message().to_string()),
        }
    }

    fn clear_cache(&self) -> Result<CacheClear, String> {
        let software = Software::new(self.store.clone());
        let removed = software
            .clear_download_cache(&|| false)
            .map_err(|error| error.to_string())?;
        let current = software
            .download_cache_info()
            .map_err(|error| error.to_string())?;
        Ok(CacheClear {
            removed: removed.into(),
            current: current.into(),
        })
    }

    fn set_log_rotation(&self, enabled: bool) -> Result<(), String> {
        self.diagnostics()
            .set_log_rotation(enabled)
            .map_err(|error| error.to_string())
    }

    fn set_login_item(&self, enabled: bool) -> LoginChange {
        let dir = default_login_agent_dir();
        let result = if enabled {
            let app = self.app_path();
            let app = (!app.is_empty()).then(|| Path::new(app.as_str()));
            resolve_login_executable(&SystemHost, app, None)
                .and_then(|exe| enable_launch_at_login(&dir, &exe, &SystemRunner))
                .map_err(|error| error.to_string())
        } else {
            disable_launch_at_login(&dir, &SystemRunner).map_err(|error| error.to_string())
        };
        LoginChange {
            enabled: launch_at_login_enabled(&dir),
            error: result.err(),
        }
    }

    fn open_full_disk_access(&self) -> Result<(), String> {
        open_url(
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles",
        )
    }

    fn open_login_items(&self) -> Result<(), String> {
        open_url("x-apple.systempreferences:com.apple.LoginItems-Settings.extension")
    }

    fn reveal(&self, path: &str) -> Result<(), String> {
        run_open(&["-R", path])
    }

    fn preview_shutdown(&self) -> Result<ShutdownPlan, String> {
        let engine = Engine::new(self.store.root(), Arc::new(SystemCommandRunner::new()));
        Shutdown::new(&self.store, &engine)
            .preview(&|| false)
            .map_err(|error| error.to_string())
    }

    fn shutdown(&self, plan: &ShutdownPlan) -> Result<(), String> {
        let engine = Engine::new(self.store.root(), Arc::new(SystemCommandRunner::new()));
        Shutdown::new(&self.store, &engine)
            .execute(plan, &Default::default(), &|| false)
            .map_err(|error| error.to_string())
    }
}

static FACADE: LazyLock<Facade> = LazyLock::new(Facade::new);

fn diagnostics_provider_of() -> Option<Arc<dyn DiagnosticsProvider>> {
    if let Some(provider) = take(&DIAGNOSTICS_FAKE) {
        return Some(provider);
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return None;
    }
    Some(Arc::new(FACADE.clone()))
}

fn operations_provider_of() -> Option<Arc<dyn OperationsProvider>> {
    if let Some(provider) = take(&OPERATIONS_FAKE) {
        return Some(provider);
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return None;
    }
    Some(Arc::new(FACADE.clone()))
}

fn ports_provider_of() -> Option<Arc<dyn PortsProvider>> {
    if let Some(provider) = take(&PORTS_FAKE) {
        return Some(provider);
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return None;
    }
    Some(Arc::new(FACADE.clone()))
}

fn settings_provider_of() -> Option<Arc<dyn SettingsProvider>> {
    if let Some(provider) = take(&SETTINGS_FAKE) {
        return Some(provider);
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return None;
    }
    Some(Arc::new(FACADE.clone()))
}

// --- Production paths --------------------------------------------------------

/// `launchd.UserAgents().Dir`, or the CLI's isolated override.
pub(crate) fn default_login_agent_dir() -> PathBuf {
    match std::env::var("MSM_DEV_HOME") {
        Ok(home) if !home.is_empty() => PathBuf::from(home).join("login-agents"),
        _ => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Library/LaunchAgents"),
    }
}

/// The app bundle this process runs from, or empty (`guide.go:71-85`).
fn current_app_bundle() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return String::new();
    };
    let Some(macos) = exe.parent() else {
        return String::new();
    };
    let text = macos.to_string_lossy();
    if text.ends_with(".app/Contents/MacOS") {
        macos
            .parent()
            .and_then(Path::parent)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        String::new()
    }
}

fn run_open(args: &[&str]) -> Result<(), String> {
    let status = Command::new("open")
        .args(args)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open exited with {status}"))
    }
}

fn open_url(url: &str) -> Result<(), String> {
    run_open(&[url])
}

// --- State -------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct RuntimeState {
    snapshot: BTreeMap<String, String>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    sequence: u64,
}

#[derive(Debug, Clone, Default)]
struct ReportState {
    report: Option<DiagnosticReport>,
    json: String,
    loading: bool,
    error: Option<String>,
    exporting: bool,
    sequence: u64,
}

#[derive(Debug, Clone, Default)]
struct OperationsState {
    records: Vec<OperationRecord>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    sequence: u64,
    busy: BTreeSet<String>,
    action_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct PortsState {
    inspections: BTreeMap<i64, PortInspection>,
    checking: bool,
    suggesting: bool,
    listening: Vec<ListeningPort>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    sequence: u64,
}

/// Dedicated signal for the Diagnostics port input so typing never invalidates
/// unrelated `Workspace` readers (`runtime`, `checks`, `listening`, `operations`).
#[derive(Debug, Clone)]
struct PortInput(String);

impl Default for PortInput {
    fn default() -> Self {
        PortInput("3000".to_string())
    }
}

fn port_input_signal() -> Signal<PortInput> {
    global::<PortInput>()
}

#[derive(Debug, Clone, Default)]
struct SettingsState {
    snapshot: SettingsSnapshot,
    loaded: bool,
    loading: bool,
    error: Option<String>,
    sequence: u64,
    helper_busy: bool,
    helper_activity: Option<String>,
    cli_busy: bool,
    cli_activity: Option<String>,
    cache_busy: bool,
    log_rotation_busy: bool,
    login_busy: bool,
    action_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Workspace {
    runtime: RuntimeState,
    report: ReportState,
    operations: OperationsState,
    ports: PortsState,
    settings: SettingsState,
}

fn workspace() -> Signal<Workspace> {
    global::<Workspace>()
}

fn task(future: impl Future<Output = ()> + 'static) {
    Owner::detached().run(move || {
        spawn(future);
    });
}

fn set_settings_error(message: String) {
    workspace().update(|state| {
        state.settings.action_error = Some(message);
    });
}

// --- Diagnostics -------------------------------------------------------------

/// Reads the runtime snapshot, journal and ports the first time the screen
/// appears. Diagnostics itself is never run implicitly, as in Go.
pub(crate) fn load() {
    let (runtime, operations, ports) = workspace().with(|state| {
        (
            state.runtime.loaded || state.runtime.loading,
            state.operations.loaded || state.operations.loading,
            state.ports.loaded || state.ports.loading,
        )
    });
    if !runtime {
        refresh_runtime();
    }
    if !operations {
        refresh_operations();
    }
    if !ports {
        refresh_ports();
    }
}

/// Re-reads everything the screen shows; the report stays until run again.
pub(crate) fn refresh() {
    refresh_runtime();
    refresh_operations();
    refresh_ports();
}

pub(crate) fn refresh_runtime() {
    let Some(provider) = diagnostics_provider_of() else {
        workspace().update(|state| {
            state.runtime.loading = false;
            state.runtime.error = Some("no diagnostics provider is available".to_string());
        });
        return;
    };
    let sequence = workspace().with(|state| state.runtime.sequence + 1);
    workspace().update(|state| {
        state.runtime.sequence = sequence;
        state.runtime.loading = true;
        state.runtime.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.snapshot()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the runtime snapshot did not finish: {error}")),
        };
        workspace().update(|state| {
            if state.runtime.sequence != sequence {
                return;
            }
            state.runtime.loading = false;
            state.runtime.loaded = true;
            match answer {
                Ok(snapshot) => {
                    state.runtime.snapshot = snapshot;
                    state.runtime.error = None;
                }
                Err(error) => state.runtime.error = Some(error),
            }
        });
    });
}

pub(crate) fn runtime_snapshot() -> BTreeMap<String, String> {
    workspace().with(|state| state.runtime.snapshot.clone())
}

pub(crate) fn runtime_loading() -> bool {
    workspace().with(|state| state.runtime.loading)
}

pub(crate) fn runtime_error() -> Option<String> {
    workspace().with(|state| state.runtime.error.clone())
}

/// `runDevDiagnostics`: diagnose in a task, then show the report.
pub(crate) fn run_diagnostics() {
    if workspace().with(|state| state.report.loading) {
        return;
    }
    let Some(provider) = diagnostics_provider_of() else {
        workspace().update(|state| {
            state.report.error = Some("no diagnostics provider is available".to_string());
        });
        return;
    };
    let sequence = workspace().with(|state| state.report.sequence + 1);
    workspace().update(|state| {
        state.report.sequence = sequence;
        state.report.loading = true;
        state.report.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.diagnose()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("diagnostics did not finish: {error}")),
        };
        let mut notify: Option<(bool, String)> = None;
        workspace().update(|state| {
            if state.report.sequence != sequence {
                return;
            }
            state.report.loading = false;
            match answer {
                Ok(report) => {
                    state.report.json = report
                        .redacted_json()
                        .ok()
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                        .unwrap_or_default();
                    let failed = report
                        .checks
                        .iter()
                        .filter(|check| check.status == "Failed")
                        .count();
                    let summary = if failed > 0 {
                        format!(
                            "Diagnostics finished with {} failed {}.",
                            failed,
                            if failed == 1 { "check" } else { "checks" }
                        )
                    } else {
                        "Diagnostics finished cleanly.".to_string()
                    };
                    notify = Some((failed > 0, summary));
                    state.report.report = Some(report);
                    state.report.error = None;
                }
                Err(error) => {
                    notify = Some((true, error.clone()));
                    state.report.error = Some(error);
                }
            }
        });
        if let Some((failed, body)) = notify {
            crate::notify_task(
                "devbase.diagnostics",
                "Diagnostics",
                &body,
                Section::Diagnostics,
                failed,
            );
        }
    });
}

pub(crate) fn report() -> Option<DiagnosticReport> {
    workspace().with(|state| state.report.report.clone())
}

pub(crate) fn report_json() -> String {
    workspace().with(|state| state.report.json.clone())
}

pub(crate) fn report_loading() -> bool {
    workspace().with(|state| state.report.loading)
}

pub(crate) fn report_exporting() -> bool {
    workspace().with(|state| state.report.exporting)
}

pub(crate) fn report_error() -> Option<String> {
    workspace().with(|state| state.report.error.clone())
}

/// Copies the redacted JSON and notifies with a toast.
pub(crate) fn copy_redacted_report() {
    let json = report_json();
    if json.is_empty() {
        return;
    }
    set_clipboard(json);
    workspace().update(|state| {
        state.report.error = None;
    });
    crate::toast::info("Copied redacted report");
}

/// `Export redacted report`: devcore writes the private, redacted file.
pub(crate) fn export_report() {
    let Some(report) = report() else {
        return;
    };
    if workspace().with(|state| state.report.exporting) {
        return;
    }
    let Some(provider) = diagnostics_provider_of() else {
        workspace().update(|state| {
            state.report.error = Some("no diagnostics provider is available".to_string());
        });
        return;
    };
    workspace().update(|state| {
        state.report.exporting = true;
        state.report.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.export(&report)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the report was not exported: {error}")),
        };
        let mut toast_message = None;
        workspace().update(|state| {
            state.report.exporting = false;
            match answer {
                Ok(path) => {
                    toast_message = Some(format!("Redacted report saved: {}", path.display()));
                    state.report.error = None;
                }
                Err(error) => {
                    state.report.error = Some(error);
                }
            }
        });
        if let Some(message) = toast_message {
            crate::toast::success(message);
        }
    });
}

// --- Operations --------------------------------------------------------------

pub(crate) fn refresh_operations() {
    let Some(provider) = operations_provider_of() else {
        workspace().update(|state| {
            state.operations.loading = false;
            state.operations.error = Some("no operations provider is available".to_string());
        });
        return;
    };
    let sequence = workspace().with(|state| state.operations.sequence + 1);
    workspace().update(|state| {
        state.operations.sequence = sequence;
        state.operations.loading = true;
        state.operations.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.operations()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the task history did not finish: {error}")),
        };
        workspace().update(|state| {
            if state.operations.sequence != sequence {
                return;
            }
            state.operations.loading = false;
            state.operations.loaded = true;
            match answer {
                Ok(records) => {
                    state.operations.records = records;
                    state.operations.error = None;
                }
                Err(error) => state.operations.error = Some(error),
            }
        });
    });
}

pub(crate) fn operations() -> Vec<OperationRecord> {
    workspace().with(|state| state.operations.records.clone())
}

pub(crate) fn operations_loading() -> bool {
    workspace().with(|state| state.operations.loading)
}

pub(crate) fn operations_error() -> Option<String> {
    workspace().with(|state| state.operations.error.clone())
}

pub(crate) fn operation_action_error() -> Option<String> {
    workspace().with(|state| state.operations.action_error.clone())
}

pub(crate) fn operation_busy(id: &str) -> bool {
    workspace().with(|state| state.operations.busy.contains(id))
}

pub(crate) fn retry_operation(id: &str) {
    let Some(record) = operations().into_iter().find(|record| record.id == id) else {
        return;
    };
    if operation_busy(id) {
        return;
    }
    let Some(provider) = operations_provider_of() else {
        workspace().update(|state| {
            state.operations.action_error = Some("no operations provider is available".to_string());
        });
        return;
    };
    workspace().update(|state| {
        state.operations.busy.insert(id.to_string());
        state.operations.action_error = None;
    });
    let request = (record.id.clone(), record.revision.clone());
    let busy_id = id.to_string();
    task(async move {
        let answer = match blocking(move || provider.retry(&request.0, &request.1)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the retry did not start: {error}")),
        };
        workspace().update(|state| {
            state.operations.busy.remove(&busy_id);
            match answer {
                Ok(()) => {
                    state.operations.action_error = None;
                }
                Err(error) => state.operations.action_error = Some(error),
            }
        });
        refresh_operations();
    });
}

pub(crate) fn dismiss_operation(id: &str) {
    let Some(record) = operations().into_iter().find(|record| record.id == id) else {
        return;
    };
    if operation_busy(id) {
        return;
    }
    let Some(provider) = operations_provider_of() else {
        workspace().update(|state| {
            state.operations.action_error = Some("no operations provider is available".to_string());
        });
        return;
    };
    workspace().update(|state| {
        state.operations.busy.insert(id.to_string());
        state.operations.action_error = None;
    });
    let request = (record.id.clone(), record.revision.clone());
    let busy_id = id.to_string();
    task(async move {
        let answer = match blocking(move || provider.dismiss(&request.0, &request.1)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the task record was not dismissed: {error}")),
        };
        workspace().update(|state| {
            state.operations.busy.remove(&busy_id);
            match answer {
                Ok(()) => {
                    state.operations.action_error = None;
                }
                Err(error) => state.operations.action_error = Some(error),
            }
        });
        refresh_operations();
    });
}

// --- Ports -------------------------------------------------------------------

pub(crate) fn port_input() -> String {
    port_input_signal().with(|state| state.0.clone())
}

pub(crate) fn set_port_input(next: String) {
    port_input_signal().set(PortInput(next));
}

pub(crate) fn checking_port() -> bool {
    workspace().with(|state| state.ports.checking)
}

pub(crate) fn suggesting_port() -> bool {
    workspace().with(|state| state.ports.suggesting)
}

pub(crate) fn port_error() -> Option<String> {
    workspace().with(|state| state.ports.error.clone())
}

/// `checkDevPort`: bind IPv4 loopback, then observe listeners if in use.
pub(crate) fn check_port() {
    let text = port_input();
    let Ok(port) = text.trim().parse::<i64>() else {
        workspace().update(|state| {
            state.ports.error = Some("port must be 1–65535".to_string());
        });
        return;
    };
    if !(1..=65535).contains(&port) {
        workspace().update(|state| {
            state.ports.error = Some("port must be 1–65535".to_string());
        });
        return;
    }
    if workspace().with(|state| state.ports.checking) {
        return;
    }
    let Some(provider) = ports_provider_of() else {
        workspace().update(|state| {
            state.ports.error = Some("no ports provider is available".to_string());
        });
        return;
    };
    workspace().update(|state| {
        state.ports.checking = true;
        state.ports.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.inspect(port)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the port check did not finish: {error}")),
        };
        workspace().update(|state| {
            state.ports.checking = false;
            match answer {
                Ok(inspection) => {
                    state.ports.inspections.insert(port, inspection);
                    state.ports.error = None;
                }
                Err(error) => state.ports.error = Some(error),
            }
        });
    });
}

/// `Find a free port`: fill the field with devcore's suggestion.
pub(crate) fn find_free_port() {
    if workspace().with(|state| state.ports.suggesting) {
        return;
    }
    let Some(provider) = ports_provider_of() else {
        workspace().update(|state| {
            state.ports.error = Some("no ports provider is available".to_string());
        });
        return;
    };
    workspace().update(|state| {
        state.ports.suggesting = true;
        state.ports.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.available_local_port()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("no free port was suggested: {error}")),
        };
        let mut suggested_port = None;
        workspace().update(|state| {
            state.ports.suggesting = false;
            match answer {
                Ok(port) => {
                    suggested_port = Some(port);
                    state.ports.error = None;
                }
                Err(error) => state.ports.error = Some(error),
            }
        });
        if let Some(port) = suggested_port {
            set_port_input(port.to_string());
            crate::toast::info(format!(
                "Suggested port: {port}. An unchanged form is filled automatically. Review, save and retry; this does not reserve the port."
            ));
        }
    });
}

pub(crate) fn port_inspection(port: i64) -> Option<PortInspection> {
    workspace().with(|state| state.ports.inspections.get(&port).cloned())
}

pub(crate) fn refresh_ports() {
    let Some(provider) = ports_provider_of() else {
        workspace().update(|state| {
            state.ports.loading = false;
            state.ports.error = Some("no ports provider is available".to_string());
        });
        return;
    };
    let sequence = workspace().with(|state| state.ports.sequence + 1);
    workspace().update(|state| {
        state.ports.sequence = sequence;
        state.ports.loading = true;
        state.ports.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.listening()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the listening ports did not finish: {error}")),
        };
        workspace().update(|state| {
            if state.ports.sequence != sequence {
                return;
            }
            state.ports.loading = false;
            state.ports.loaded = true;
            match answer {
                Ok(ports) => {
                    state.ports.listening = ports;
                    state.ports.error = None;
                }
                Err(error) => state.ports.error = Some(error),
            }
        });
    });
}

pub(crate) fn listening_ports() -> Vec<ListeningPort> {
    workspace().with(|state| state.ports.listening.clone())
}

pub(crate) fn ports_loading() -> bool {
    workspace().with(|state| state.ports.loading)
}

// --- Settings ----------------------------------------------------------------

/// Reads the helper, CLI, cache, rotation and login-item state.
pub(crate) fn load_settings() {
    let (loaded, loading) =
        workspace().with(|state| (state.settings.loaded, state.settings.loading));
    if !loaded && !loading {
        refresh_settings();
    }
}

pub(crate) fn refresh_settings() {
    if workspace().with(|state| state.settings.loading) {
        return;
    }
    let Some(provider) = settings_provider_of() else {
        workspace().update(|state| {
            state.settings.loading = false;
            state.settings.error = Some("no settings provider is available".to_string());
        });
        return;
    };
    let sequence = workspace().with(|state| state.settings.sequence + 1);
    workspace().update(|state| {
        state.settings.sequence = sequence;
        state.settings.loading = true;
        state.settings.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.snapshot()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("settings did not finish loading: {error}")),
        };
        workspace().update(|state| {
            if state.settings.sequence != sequence {
                return;
            }
            state.settings.loading = false;
            state.settings.loaded = true;
            match answer {
                Ok(snapshot) => {
                    state.settings.snapshot = snapshot;
                    state.settings.error = None;
                }
                Err(error) => state.settings.error = Some(error),
            }
        });
    });
}

pub(crate) fn settings_error() -> Option<String> {
    workspace().with(|state| state.settings.error.clone())
}

pub(crate) fn settings_action_error() -> Option<String> {
    workspace().with(|state| state.settings.action_error.clone())
}

pub(crate) fn helper_status() -> HelperStatus {
    workspace().with(|state| state.settings.snapshot.helper.clone())
}

pub(crate) fn helper_busy() -> bool {
    workspace().with(|state| state.settings.helper_busy)
}

/// The install or removal line while the helper is busy.
pub(crate) fn helper_activity() -> Option<String> {
    workspace().with(|state| state.settings.helper_activity.clone())
}

pub(crate) fn cli_status() -> CliStatus {
    workspace().with(|state| state.settings.snapshot.cli.clone())
}

pub(crate) fn cli_busy() -> bool {
    workspace().with(|state| state.settings.cli_busy)
}

/// The install or removal line while the CLI wrapper is busy.
pub(crate) fn cli_activity() -> Option<String> {
    workspace().with(|state| state.settings.cli_activity.clone())
}

pub(crate) fn cache() -> CacheInfo {
    workspace().with(|state| state.settings.snapshot.cache.clone())
}

pub(crate) fn cache_busy() -> bool {
    workspace().with(|state| state.settings.cache_busy)
}

pub(crate) fn log_rotation_enabled() -> bool {
    workspace().with(|state| state.settings.snapshot.log_rotation_enabled)
}

pub(crate) fn log_rotation_busy() -> bool {
    workspace().with(|state| state.settings.log_rotation_busy)
}

pub(crate) fn login_at_login() -> bool {
    workspace().with(|state| state.settings.snapshot.login_at_login)
}

pub(crate) fn login_busy() -> bool {
    workspace().with(|state| state.settings.login_busy)
}

pub(crate) fn workspace_root() -> String {
    workspace().with(|state| state.settings.snapshot.workspace_root.clone())
}

/// Whether any Settings operation is in flight (`settingsBusy || helperBusy`,
/// plus the CLI, cache, rotation and login work).
pub(crate) fn settings_busy() -> bool {
    workspace().with(|state| {
        let settings = &state.settings;
        settings.loading
            || settings.helper_busy
            || settings.cli_busy
            || settings.cache_busy
            || settings.log_rotation_busy
            || settings.login_busy
    })
}

/// `installHelperThen`: install in a background task; the administrator prompt
/// is the client's, and the status and error are shown verbatim.
pub(crate) fn install_helper() {
    if helper_busy() {
        return;
    }
    let Some(provider) = settings_provider_of() else {
        set_settings_error("no settings provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.settings.helper_busy = true;
        state.settings.helper_activity = Some("Installing helper…".to_string());
        state.settings.action_error = None;
    });
    task(async move {
        let change = match blocking(move || provider.install_helper()).await {
            Ok(change) => change,
            Err(error) => HelperChange {
                status: HelperStatus::default(),
                error: Some(format!("the helper install did not finish: {error}")),
            },
        };
        let mut succeeded = false;
        workspace().update(|state| {
            state.settings.helper_busy = false;
            state.settings.helper_activity = None;
            state.settings.snapshot.helper = change.status;
            match change.error {
                Some(error) => {
                    state.settings.action_error =
                        Some(format!("Could not install privileged helper: {error}"));
                }
                None => {
                    state.settings.action_error = None;
                    succeeded = true;
                }
            }
        });
        if succeeded {
            crate::toast::success("Privileged helper installed");
        }
    });
}

/// `runUninstallHelper`.
pub(crate) fn uninstall_helper() {
    if helper_busy() {
        return;
    }
    let Some(provider) = settings_provider_of() else {
        set_settings_error("no settings provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.settings.helper_busy = true;
        state.settings.helper_activity = Some("Removing helper…".to_string());
        state.settings.action_error = None;
    });
    task(async move {
        let change = match blocking(move || provider.uninstall_helper()).await {
            Ok(change) => change,
            Err(error) => HelperChange {
                status: HelperStatus::default(),
                error: Some(format!("the helper removal did not finish: {error}")),
            },
        };
        let mut succeeded = false;
        workspace().update(|state| {
            state.settings.helper_busy = false;
            state.settings.helper_activity = None;
            state.settings.snapshot.helper = change.status;
            match change.error {
                Some(error) => {
                    state.settings.action_error =
                        Some(format!("Could not remove privileged helper: {error}"));
                }
                None => {
                    state.settings.action_error = None;
                    succeeded = true;
                }
            }
        });
        if succeeded {
            crate::toast::success("Privileged helper removed");
        }
    });
}

/// `installCLI`.
pub(crate) fn install_cli() {
    if cli_busy() {
        return;
    }
    let Some(provider) = settings_provider_of() else {
        set_settings_error("no settings provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.settings.cli_busy = true;
        state.settings.cli_activity = Some("Installing devbase CLI…".to_string());
        state.settings.action_error = None;
    });
    task(async move {
        let change = match blocking(move || provider.install_cli()).await {
            Ok(change) => change,
            Err(error) => CliChange {
                status: CliStatus::default(),
                error: Some(format!("the CLI install did not finish: {error}")),
            },
        };
        let mut toast_message = None;
        workspace().update(|state| {
            state.settings.cli_busy = false;
            state.settings.cli_activity = None;
            let wrapper = change.status.wrapper_path.clone();
            state.settings.snapshot.cli = change.status;
            match change.error {
                Some(error) => {
                    state.settings.action_error =
                        Some(format!("Could not install devbase CLI: {error}"));
                }
                None => {
                    state.settings.action_error = None;
                    toast_message = Some(format!("Installed devbase CLI at {wrapper}"));
                }
            }
        });
        if let Some(message) = toast_message {
            crate::toast::success(message);
        }
    });
}

/// `uninstallCLI`.
pub(crate) fn uninstall_cli() {
    if cli_busy() {
        return;
    }
    let Some(provider) = settings_provider_of() else {
        set_settings_error("no settings provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.settings.cli_busy = true;
        state.settings.cli_activity = Some("Removing devbase CLI…".to_string());
        state.settings.action_error = None;
    });
    task(async move {
        let change = match blocking(move || provider.uninstall_cli()).await {
            Ok(change) => change,
            Err(error) => CliChange {
                status: CliStatus::default(),
                error: Some(format!("the CLI removal did not finish: {error}")),
            },
        };
        let mut succeeded = false;
        workspace().update(|state| {
            state.settings.cli_busy = false;
            state.settings.cli_activity = None;
            state.settings.snapshot.cli = change.status;
            match change.error {
                Some(error) => {
                    state.settings.action_error =
                        Some(format!("Could not remove devbase CLI: {error}"));
                }
                None => {
                    state.settings.action_error = None;
                    succeeded = true;
                }
            }
        });
        if succeeded {
            crate::toast::success("Removed devbase CLI");
        }
    });
}

/// `setLogRotation`.
pub(crate) fn set_log_rotation(enabled: bool) {
    if log_rotation_busy() {
        return;
    }
    let Some(provider) = settings_provider_of() else {
        set_settings_error("no settings provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.settings.log_rotation_busy = true;
        state.settings.action_error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.set_log_rotation(enabled)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("log rotation was not updated: {error}")),
        };
        let mut succeeded = false;
        workspace().update(|state| {
            state.settings.log_rotation_busy = false;
            match answer {
                Ok(()) => {
                    state.settings.snapshot.log_rotation_enabled = enabled;
                    state.settings.action_error = None;
                    succeeded = true;
                }
                Err(error) => {
                    state.settings.action_error = Some(error);
                }
            }
        });
        if succeeded {
            crate::toast::success(if enabled {
                "Automatic log rotation enabled."
            } else {
                "Automatic log rotation disabled."
            });
        }
    });
}

/// `setLaunchAtLogin`.
pub(crate) fn set_login_item(enabled: bool) {
    if login_busy() {
        return;
    }
    let Some(provider) = settings_provider_of() else {
        set_settings_error("no settings provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.settings.login_busy = true;
        state.settings.action_error = None;
    });
    task(async move {
        let change = match blocking(move || provider.set_login_item(enabled)).await {
            Ok(change) => change,
            Err(error) => LoginChange {
                enabled: false,
                error: Some(format!("the login item did not update: {error}")),
            },
        };
        let mut succeeded = false;
        workspace().update(|state| {
            state.settings.login_busy = false;
            state.settings.snapshot.login_at_login = change.enabled;
            match change.error {
                Some(error) => {
                    state.settings.action_error =
                        Some(format!("Could not update login item: {error}"));
                }
                None => {
                    state.settings.action_error = None;
                    succeeded = true;
                }
            }
        });
        if succeeded {
            crate::toast::success(if enabled {
                "Devbase will start at login in the menu bar"
            } else {
                "Start at login disabled"
            });
        }
    });
}

/// Asks before clearing the cache, as the Go screen does.
pub(crate) fn request_clear_cache() {
    if cache_busy() || cache().files == 0 {
        return;
    }
    let answer = nsh_base::alert_dialog()
        .title("Clear download cache?")
        .description(
            "Removes cached installation archives. Installed runtimes and database engines are not changed.",
        )
        .cancel("cancel", "Cancel")
        .destructive("confirm", "Clear Cache")
        .show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if chosen {
            perform_clear_cache();
        }
    });
}

fn perform_clear_cache() {
    let Some(provider) = settings_provider_of() else {
        set_settings_error("no settings provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.settings.cache_busy = true;
        state.settings.action_error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.clear_cache()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the cache was not cleared: {error}")),
        };
        let mut toast_message: Option<(bool, String)> = None;
        workspace().update(|state| {
            state.settings.cache_busy = false;
            match answer {
                Ok(cleared) => {
                    let removed = cleared.removed;
                    state.settings.snapshot.cache = cleared.current;
                    state.settings.action_error = None;
                    if removed.files == 0 {
                        toast_message =
                            Some((false, "Download cache is already empty.".to_string()));
                    } else {
                        toast_message = Some((
                            true,
                            format!(
                                "Cleared {} ({}) from the download cache.",
                                plural(removed.files as usize, "archive"),
                                format_bytes(removed.bytes)
                            ),
                        ));
                    }
                }
                Err(error) => {
                    state.settings.action_error = Some(error);
                }
            }
        });
        if let Some((is_success, message)) = toast_message {
            if is_success {
                crate::toast::success(message);
            } else {
                crate::toast::info(message);
            }
        }
    });
}

pub(crate) fn open_full_disk_access() {
    run_settings_action(
        |provider| provider.open_full_disk_access(),
        "open Full Disk Access",
    );
}

pub(crate) fn open_login_items() {
    run_settings_action(|provider| provider.open_login_items(), "open Login Items");
}

pub(crate) fn reveal_workspace() {
    let root = workspace_root();
    if root.is_empty() {
        return;
    }
    run_settings_action(
        move |provider| provider.reveal(&root),
        "reveal the workspace",
    );
}

fn run_settings_action(
    action: impl Fn(&Arc<dyn SettingsProvider>) -> Result<(), String> + Send + Sync + 'static,
    what: &'static str,
) {
    let Some(provider) = settings_provider_of() else {
        set_settings_error("no settings provider is available".to_string());
        return;
    };
    task(async move {
        let answer = match blocking(move || action(&provider)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("could not {what}: {error}")),
        };
        if let Err(error) = answer {
            set_settings_error(error);
        }
    });
}

/// Copies `devbase schema`, the AI contract command.
pub(crate) fn copy_schema_command() {
    set_clipboard("devbase schema".to_string());
    workspace().update(|state| {
        state.settings.action_error = None;
    });
    crate::toast::info("Copied devbase schema");
}

// --- Quit confirmation -------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct QuitState {
    open: bool,
    loading: bool,
    stopping: bool,
    plan: Option<ShutdownPlan>,
    error: Option<String>,
    sequence: u64,
}

fn quit_state() -> Signal<QuitState> {
    global::<QuitState>()
}

#[derive(Debug, Clone, Default)]
struct QuitHandlerInstalled(bool);

/// Registers the quit-request listener once per surface.
pub(crate) fn install_quit_handler() {
    let installed = global::<QuitHandlerInstalled>();
    if installed.with_untracked(|flag| flag.0) {
        return;
    }
    installed.set(QuitHandlerInstalled(true));
    nsh_core::element::desktop::on_quit_request(request_quit);
    nsh_core::element::desktop::set_quit_request_options(
        nsh_core::desktop::QuitRequestOptions::default().system(true),
    );
}

pub fn quit_open() -> bool {
    quit_state().with(|state| state.open)
}

/// `requestQuit`: read the managed-database scope, then ask.
pub fn request_quit() {
    if quit_open() {
        let window = nsh_core::element::desktop::main_window();
        window.show();
        window.focus();
        return;
    }
    let sequence = quit_state().with(|state| state.sequence + 1);
    quit_state().update(|state| {
        state.open = true;
        state.loading = true;
        state.stopping = false;
        state.plan = None;
        state.error = None;
        state.sequence = sequence;
    });
    let window = nsh_core::element::desktop::main_window();
    window.show();
    window.focus();
    let Some(provider) = settings_provider_of() else {
        quit_state().update(|state| {
            state.error = Some(
                "Could not read the quit scope: no settings provider is available.".to_string(),
            );
            state.loading = false;
        });
        return;
    };
    task(async move {
        let answer = match blocking(move || provider.preview_shutdown()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("Could not read the quit scope: {error}")),
        };
        quit_state().update(|state| {
            if state.sequence != sequence || !state.open {
                return;
            }
            state.loading = false;
            match answer {
                Ok(plan) => state.plan = Some(plan),
                Err(error) => state.error = Some(error),
            }
        });
        let quit_now = quit_state().with(|state| {
            state.open
                && state.sequence == sequence
                && state.error.is_none()
                && state
                    .plan
                    .as_ref()
                    .is_some_and(|plan| plan.instances.is_empty())
        });
        if quit_now {
            finish_quit();
        }
    });
}

fn finish_quit() {
    quit_state().update(|state| {
        state.open = false;
    });
    nsh_core::element::desktop::quit();
}

fn cancel_quit() {
    quit_state().update(|state| {
        state.open = false;
        state.sequence += 1;
        state.stopping = false;
    });
    nsh_core::element::desktop::cancel_quit_request();
}

fn stop_for_quit() {
    let (plan, busy) =
        quit_state().with(|state| (state.plan.clone(), state.stopping || state.loading));
    let Some(plan) = plan else {
        return;
    };
    if busy {
        return;
    }
    let Some(provider) = settings_provider_of() else {
        quit_state().update(|state| {
            state.error = Some(
                "Could not stop managed databases: no settings provider is available.".to_string(),
            );
        });
        return;
    };
    quit_state().update(|state| {
        state.stopping = true;
        state.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.shutdown(&plan)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the shutdown did not finish: {error}")),
        };
        match answer {
            Ok(()) => finish_quit(),
            Err(error) => {
                quit_state().update(|state| {
                    state.stopping = false;
                    state.error = Some(format!(
                        "{error}\nSome services may already have stopped. Review the scope to retry, or quit keeping what remains."
                    ));
                    state.plan = None;
                });
            }
        }
    });
}

/// The quit dialog's body, rendered by the app root while [`quit_open`].
pub(crate) fn quit_dialog() -> impl IntoElement {
    use nsh_base::theme;

    nsh_base::dialog()
        .label("Quit Devbase?")
        .on_cancel(cancel_quit)
        .child(
            view()
                .test_id("quit-dialog-backdrop")
                .size_full()
                .items_center()
                .justify_center()
                .bg(AdaptiveColor::rgb(0x000000, 0x000000).opacity(0.35))
                .child(
                    view()
                        .test_id("quit-dialog")
                        .w(px(520.0))
                        .p(px(20.0))
                        .flex_col()
                        .gap(px(12.0))
                        .rounded(tokens::panel_radius)
                        .bg(tokens::panel)
                        .border_1()
                        .border_color(tokens::hairline)
                        .shadow_xl()
                        .child(
                            text("Quit Devbase?")
                                .text_size(px(18.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || theme().foreground),
                        )
                        .child(dynamic(|| {
                            let state = quit_state().with(|state| state.clone());
                            let instance_ids: Vec<String> = state
                                .plan
                                .as_ref()
                                .map(|plan| {
                                    plan.instances
                                        .iter()
                                        .map(|item| {
                                            if !item.id.is_empty() {
                                                item.id.clone()
                                            } else {
                                                item.name.clone()
                                            }
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            let instances = instance_ids.len();
                            let loading = state.loading;
                            let stopping = state.stopping;
                            let has_instances = instances > 0;
                            let show_instance_list =
                                !loading && state.error.is_none() && has_instances;
                            let stop_disabled = loading || stopping || state.plan.is_none();
                            let body = view()
                                .flex_col()
                                .gap(px(12.0))
                                .child(
                                    view().flex_col().gap(px(6.0)).children(
                                        [
                                            loading.then(|| {
                                                dialog_line(
                                                    "Reading managed service scope…".to_string(),
                                                    false,
                                                )
                                            }),
                                            state
                                                .error
                                                .clone()
                                                .map(|error| dialog_line(error, true)),
                                            (!loading && state.error.is_none()).then(|| {
                                                dialog_line(
                                                    if instances == 0 {
                                                        "No managed databases are running."
                                                            .to_string()
                                                    } else {
                                                        format!(
                                                            "Stop scope: {}",
                                                            plural(instances, "managed database")
                                                        )
                                                    },
                                                    false,
                                                )
                                            }),
                                            stopping.then(|| {
                                                dialog_line(
                                                    "Stopping managed databases…".to_string(),
                                                    false,
                                                )
                                            }),
                                        ]
                                        .into_iter()
                                        .flatten(),
                                    ),
                                )
                                .child(when(show_instance_list, move || {
                                    scroll()
                                        .test_id("quit-instance-list")
                                        .w_full()
                                        .max_h(px(140.0))
                                        .rounded(move || theme().radius)
                                        .bg(tokens::inset)
                                        .border_1()
                                        .border_color(tokens::hairline)
                                        .child(
                                            view()
                                                .w_full()
                                                .flex_col()
                                                .gap(px(4.0))
                                                .px(px(10.0))
                                                .py(px(8.0))
                                                .children(instance_ids.iter().cloned().map(|id| {
                                                    view()
                                                        .test_id(&format!("quit-instance-{id}"))
                                                        .w_full()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap(px(8.0))
                                                        .child(
                                                            view()
                                                                .w(px(6.0))
                                                                .h(px(6.0))
                                                                .flex_none()
                                                                .rounded_full()
                                                                .bg(tokens::success),
                                                        )
                                                        .child(
                                                            text(id)
                                                                .flex_1()
                                                                .min_w(px(0.0))
                                                                .font_family("monospace")
                                                                .text_size(px(tokens::TEXT_SMALL))
                                                                .text_color(move || {
                                                                    theme().foreground
                                                                })
                                                                .truncate(),
                                                        )
                                                })),
                                        )
                                }))
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(action(
                                            "Cancel quit",
                                            "quit-cancel",
                                            false,
                                            stopping,
                                            cancel_quit,
                                        ))
                                        .child(view().flex_1())
                                        .child(when(has_instances, move || {
                                            view()
                                                .flex_row()
                                                .items_center()
                                                .gap(px(8.0))
                                                .child(action(
                                                    "Keep services and quit",
                                                    "quit-keep",
                                                    false,
                                                    loading || stopping,
                                                    finish_quit,
                                                ))
                                                .child(action(
                                                    "Stop managed databases and quit",
                                                    "quit-stop",
                                                    true,
                                                    stop_disabled,
                                                    stop_for_quit,
                                                ))
                                        }))
                                        .child(when(!has_instances, move || {
                                            action(
                                                "Quit",
                                                "quit-confirm",
                                                true,
                                                loading || stopping,
                                                finish_quit,
                                            )
                                        })),
                                );
                            Some(body.into_element())
                        })),
                ),
        )
}

fn dialog_line(message: String, error: bool) -> impl IntoElement {
    use nsh_base::theme;
    text(message).text_size(px(12.0)).text_color(move || {
        if error {
            theme().destructive
        } else {
            theme().muted_foreground
        }
    })
}

fn action(
    label: &'static str,
    test_id: &'static str,
    primary: bool,
    disabled: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    use nsh_base::theme;
    nsh_base::button()
        .test_id(test_id)
        .h(px(28.0))
        .px(px(12.0))
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .bg(move || {
            if primary {
                theme().primary
            } else {
                tokens::control()
            }
        })
        .border_1()
        .border_color(move || {
            if primary {
                theme().primary
            } else {
                tokens::hairline()
            }
        })
        .hover(move |style| {
            if primary {
                style.bg(move || theme().primary.opacity(0.86))
            } else {
                style.bg(tokens::row_hover())
            }
        })
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .disabled(disabled)
        .opacity(if disabled { 0.45 } else { 1.0 })
        .text_color(move || {
            if primary {
                theme().primary_foreground
            } else {
                theme().foreground
            }
        })
        .on_press(on_press)
        .child(text(label).text_size(px(tokens::TEXT_SMALL)))
}
