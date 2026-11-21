//! The software workspace's state: which runtime or database engine is shown,
//! the catalog versions offered for it, the installed versions read from the
//! development home, the preview of the version chosen, its integrity checks
//! and the package operation in flight.
//!
//! Every provider call does disk, process or network work, so each one runs on
//! a blocking thread and writes the store when it answers. Render code reads
//! the store; it never calls the provider's install, uninstall, verify or
//! repair itself.
//!
//! Tests install a provider of their own before the surface starts with
//! [`use_fake_provider`] and [`install_provider`]; production builds the
//! devcore [`Software`] facade on first use.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use devcore::{
    model::Package,
    software::{
        Software, cache::DownloadCacheInfo, catalog, details::PackageDetails,
        integrity::PackageIntegrityResult,
    },
    state::Store,
};
use nsh_base::alert_dialog;
use nsh_core::prelude::*;

/// The software Devbase manages, in order: runtimes first, then database
/// engines. Java and uv have their own catalog slices on the Runtimes tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftwareKind {
    Node,
    MySql,
    PostgreSql,
    Redis,
    MongoDb,
}

impl SoftwareKind {
    /// The runtimes shown above the database engines.
    pub(crate) const RUNTIMES: [SoftwareKind; 1] = [SoftwareKind::Node];

    /// The database engines.
    pub(crate) const DATABASES: [SoftwareKind; 4] = [
        SoftwareKind::MySql,
        SoftwareKind::PostgreSql,
        SoftwareKind::Redis,
        SoftwareKind::MongoDb,
    ];

    /// The `Package.name` the catalog and the state use.
    pub fn key(self) -> &'static str {
        match self {
            SoftwareKind::Node => "node",
            SoftwareKind::MySql => "mysql",
            SoftwareKind::PostgreSql => "postgresql",
            SoftwareKind::Redis => "redis",
            SoftwareKind::MongoDb => "mongodb",
        }
    }

    /// Its name in the sidebar and in sentences.
    pub fn title(self) -> &'static str {
        match self {
            SoftwareKind::Node => "Node.js",
            SoftwareKind::MySql => "MySQL",
            SoftwareKind::PostgreSql => "PostgreSQL",
            SoftwareKind::Redis => "Redis",
            SoftwareKind::MongoDb => "MongoDB",
        }
    }

    /// The kind a package name belongs to.
    pub fn from_key(name: &str) -> Option<SoftwareKind> {
        [
            SoftwareKind::Node,
            SoftwareKind::MySql,
            SoftwareKind::PostgreSql,
            SoftwareKind::Redis,
            SoftwareKind::MongoDb,
        ]
        .into_iter()
        .find(|kind| kind.key() == name)
    }

    pub(crate) fn is_database(self) -> bool {
        SoftwareKind::DATABASES.contains(&self)
    }
}

/// What the development home holds: the packages registered in `state.json`,
/// the versions marked as defaults, the packages database instances use, the
/// recorded integrity baselines and the download cache.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub installed: Vec<Package>,
    pub node_default: String,
    pub runtime_defaults: BTreeMap<String, String>,
    pub in_use: BTreeSet<String>,
    pub integrity: BTreeMap<String, String>,
    pub cache: DownloadCacheInfo,
}

/// The software operations the screen needs. The production implementation
/// wraps devcore's [`Software`]; a headless test implements it with fixtures
/// and never downloads, runs Homebrew or touches the network.
pub trait Provider: Send + Sync {
    fn snapshot(&self) -> Result<Snapshot, String>;
    fn preview(&self, name: &str, version: &str) -> Result<PackageDetails, String>;
    fn install(&self, package: &Package) -> Result<(), String>;
    fn uninstall(&self, id: &str) -> Result<(), String>;
    fn verify(&self, id: &str) -> Result<PackageIntegrityResult, String>;
    fn repair(&self, id: &str) -> Result<(), String>;
    fn clear_cache(&self) -> Result<DownloadCacheInfo, String>;
}

/// The development home, opened once.
struct Facade {
    store: Store,
}

impl Facade {
    fn new() -> Self {
        Facade {
            store: Store::new(devcore::state::default_root()),
        }
    }

    fn software(&self) -> Software {
        Software::new(self.store.clone())
    }
}

impl Provider for Facade {
    fn snapshot(&self) -> Result<Snapshot, String> {
        let state = self.store.read().map_err(|error| error.to_string())?;
        let cache = self
            .software()
            .download_cache_info()
            .map_err(|error| error.to_string())?;
        let mut installed: Vec<Package> = state.packages.values().cloned().collect();
        installed.sort_by_key(Package::id);
        Ok(Snapshot {
            installed,
            node_default: state.node_default,
            runtime_defaults: state.runtime_defaults,
            in_use: state
                .instances
                .values()
                .map(|instance| instance.package.clone())
                .collect(),
            integrity: state.package_integrity,
            cache,
        })
    }

    fn preview(&self, name: &str, version: &str) -> Result<PackageDetails, String> {
        let software = self.software();
        let package = software
            .resolve_installed_or_catalog(name, version)
            .map_err(|error| error.to_string())?;
        let cancelled = || false;
        software
            .package_details(&cancelled, &package)
            .map_err(|error| error.to_string())
    }

    fn install(&self, package: &Package) -> Result<(), String> {
        let software = self.software();
        let cancelled = || false;
        software
            .install(package, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn uninstall(&self, id: &str) -> Result<(), String> {
        let software = self.software();
        let cancelled = || false;
        software
            .uninstall(id, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn verify(&self, id: &str) -> Result<PackageIntegrityResult, String> {
        let software = self.software();
        let cancelled = || false;
        software
            .verify_package(id, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn repair(&self, id: &str) -> Result<(), String> {
        let software = self.software();
        let cancelled = || false;
        software
            .repair_package(id, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn clear_cache(&self) -> Result<DownloadCacheInfo, String> {
        let software = self.software();
        let cancelled = || false;
        software
            .clear_download_cache(&cancelled)
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

/// One read of the development home.
#[derive(Debug, Clone, Default)]
struct LoadState {
    snapshot: Snapshot,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    /// Bumped for every read, so an older answer never replaces a newer one.
    sequence: u64,
}

/// The version chosen and what the provider said about it.
#[derive(Debug, Clone)]
struct PreviewState {
    name: String,
    version: String,
    details: Option<PackageDetails>,
    loading: bool,
    error: Option<String>,
    sequence: u64,
}

/// The preview as the screen reads it.
#[derive(Debug, Clone)]
pub(crate) struct Preview {
    pub details: Option<PackageDetails>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct VersionDrafts {
    /// The version in each software's field, so switching does not lose it.
    versions: BTreeMap<&'static str, String>,
    /// Bumped on every edit so debounced quiet reviews only run after typing settles.
    generation: u64,
}

impl Default for VersionDrafts {
    fn default() -> Self {
        let mut versions = BTreeMap::new();
        for kind in SoftwareKind::RUNTIMES
            .into_iter()
            .chain(SoftwareKind::DATABASES)
        {
            versions.insert(
                kind.key(),
                catalog::available_versions(kind.key())
                    .first()
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        VersionDrafts {
            versions,
            generation: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct PreviewSlot {
    preview: Option<PreviewState>,
}

#[derive(Debug, Clone, Default)]
struct Feedback {
    action_error: Option<String>,
}

#[derive(Debug, Clone)]
struct Workspace {
    active: SoftwareKind,
    load: LoadState,
    /// Checks made this session, by package id.
    checks: BTreeMap<String, PackageIntegrityResult>,
    /// Package operations in flight, by package id, with their latest line.
    busy: BTreeMap<String, String>,
    cache_busy: bool,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            active: SoftwareKind::Node,
            load: LoadState::default(),
            checks: BTreeMap::new(),
            busy: BTreeMap::new(),
            cache_busy: false,
        }
    }
}

fn workspace() -> Signal<Workspace> {
    global::<Workspace>()
}

fn version_drafts() -> Signal<VersionDrafts> {
    global::<VersionDrafts>()
}

fn preview_slot() -> Signal<PreviewSlot> {
    global::<PreviewSlot>()
}

fn feedback() -> Signal<Feedback> {
    global::<Feedback>()
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

fn normalize_version(version: &str) -> String {
    version
        .trim()
        .strip_prefix('v')
        .unwrap_or(version.trim())
        .to_string()
}

fn clear_preview() {
    if preview_slot().with_untracked(|slot| slot.preview.is_some()) {
        preview_slot().update(|slot| slot.preview = None);
    }
}

fn clear_feedback() {
    if feedback().with_untracked(|fb| fb.action_error.is_some()) {
        feedback().update(|fb| {
            fb.action_error = None;
        });
    }
}

/// Whether the current preview matches the active software and version draft.
pub(crate) fn preview_matches() -> bool {
    let active = active_software();
    let version = version_drafts().with(|drafts| {
        drafts
            .versions
            .get(active.key())
            .cloned()
            .unwrap_or_default()
    });
    preview_slot().with(|slot| {
        let Some(preview) = &slot.preview else {
            return false;
        };
        preview.name == active.key()
            && preview.version == normalize_version(&version)
            && preview.details.is_some()
            && !preview.loading
    })
}

/// Whether the currently previewed package can be installed right now.
pub(crate) fn can_install() -> bool {
    if !preview_matches() || operation_busy() {
        return false;
    }
    preview_slot().with(|slot| {
        let Some(details) = slot
            .preview
            .as_ref()
            .and_then(|preview| preview.details.as_ref())
        else {
            return false;
        };
        let needs_repair = details.installed
            && details.integrity == "Needs repair"
            && details.package.formula.is_empty();
        !details.installed && details.compatible && !needs_repair
    })
}

// --- Selection and catalog ---------------------------------------------------

/// The software shown.
pub(crate) fn active_software() -> SoftwareKind {
    workspace().with(|state| state.active)
}

/// Shows `kind`, offering its first catalog version and clearing the preview
/// of the software shown before.
pub(crate) fn select_software(kind: SoftwareKind) {
    if workspace().with_untracked(|state| state.active == kind) {
        return;
    }
    workspace().update(|state| {
        state.active = kind;
    });
    clear_preview();
    clear_feedback();
    if version_drafts().with_untracked(|drafts| !drafts.versions.contains_key(kind.key())) {
        version_drafts().update(|drafts| {
            drafts.versions.entry(kind.key()).or_insert_with(|| {
                catalog::available_versions(kind.key())
                    .first()
                    .cloned()
                    .unwrap_or_default()
            });
        });
    }
    if workspace().with_untracked(|state| !state.load.loaded && !state.load.loading) {
        refresh();
    }
}

/// The catalog versions the field offers for `kind`.
pub(crate) fn versions(kind: SoftwareKind) -> Vec<String> {
    catalog::available_versions(kind.key())
}

/// The version chosen for the software shown.
pub(crate) fn version() -> String {
    let active = active_software();
    version_drafts().with(|drafts| {
        drafts
            .versions
            .get(active.key())
            .cloned()
            .unwrap_or_default()
    })
}

/// Changes the version field and reviews it shortly after typing stops.
pub(crate) fn set_version(next: String) {
    let active = workspace().with_untracked(|state| state.active);
    let mut generation = 0;
    version_drafts().update(|drafts| {
        drafts.versions.insert(active.key(), next);
        drafts.generation += 1;
        generation = drafts.generation;
    });
    later(Duration::from_millis(400), move || {
        review_quietly(generation);
    });
}

/// Reviews the version in the field: what Install acts on. Called by the
/// Review button and after the field changes.
pub(crate) fn review() {
    fetch_preview(true);
}

fn review_quietly(generation: u64) {
    if version_drafts().with_untracked(|drafts| drafts.generation != generation) {
        return;
    }
    let active = workspace().with_untracked(|state| state.active);
    let current_version = version_drafts().with_untracked(|drafts| {
        normalize_version(
            drafts
                .versions
                .get(active.key())
                .map(String::as_str)
                .unwrap_or_default(),
        )
    });
    let current = preview_slot().with_untracked(|slot| {
        slot.preview.as_ref().is_some_and(|preview| {
            preview.name == active.key()
                && preview.version == current_version
                && (preview.loading || preview.details.is_some())
        })
    });
    if !current {
        fetch_preview(false);
    }
}

fn fetch_preview(force: bool) {
    let active = workspace().with_untracked(|state| state.active);
    let name = active.key().to_string();
    let version = version_drafts().with_untracked(|drafts| {
        normalize_version(
            drafts
                .versions
                .get(active.key())
                .map(String::as_str)
                .unwrap_or_default(),
        )
    });
    if version.is_empty() {
        return;
    }
    let already_matches = preview_slot().with_untracked(|slot| {
        slot.preview.as_ref().is_some_and(|preview| {
            preview.name == name
                && preview.version == version
                && preview.details.is_some()
                && !preview.loading
        })
    });
    if !force && already_matches {
        return;
    }
    let Some(provider) = provider_of() else {
        set_action_error("no software provider is available".to_string());
        return;
    };
    let sequence = preview_slot().with_untracked(|slot| {
        slot.preview
            .as_ref()
            .map_or(1, |preview| preview.sequence + 1)
    });
    preview_slot().update(|slot| {
        slot.preview = Some(PreviewState {
            name: name.clone(),
            version: version.clone(),
            details: None,
            loading: true,
            error: None,
            sequence,
        });
    });
    if feedback().with_untracked(|fb| fb.action_error.is_some()) {
        feedback().update(|fb| fb.action_error = None);
    }
    task(async move {
        let request = (name.clone(), version.clone());
        let answer = match blocking(move || provider.preview(&request.0, &request.1)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the review did not answer: {error}")),
        };
        let mut resolved = None;
        preview_slot().update(|slot| {
            let Some(preview) = slot.preview.as_mut() else {
                return;
            };
            if preview.sequence != sequence || preview.name != name || preview.version != version {
                return;
            }
            preview.loading = false;
            match answer {
                Ok(details) => {
                    if !details.package.formula.is_empty() || details.package.name == "node" {
                        resolved = Some((
                            details.package.name.clone(),
                            details.package.version.clone(),
                        ));
                    }
                    if let Some((_name, version)) = &resolved {
                        preview.version = version.clone();
                    }
                    preview.details = Some(details);
                    preview.error = None;
                }
                Err(error) => {
                    preview.details = None;
                    preview.error = Some(error);
                }
            }
        });
        if let (Some((_name, version)), Some(kind)) = (resolved, SoftwareKind::from_key(&name)) {
            version_drafts().update(|drafts| {
                drafts.versions.insert(kind.key(), version);
            });
        }
    });
}

/// The preview of the version chosen, if one was asked for.
pub(crate) fn preview() -> Option<Preview> {
    preview_slot().with(|slot| {
        let preview = slot.preview.as_ref()?;
        Some(Preview {
            details: preview.details.clone(),
            loading: preview.loading,
            error: preview.error.clone(),
        })
    })
}

// --- The development home ----------------------------------------------------

/// Reads the development home the first time the screen appears.
pub(crate) fn load() {
    let (loaded, loading) = workspace().with(|state| (state.load.loaded, state.load.loading));
    if !loaded && !loading {
        refresh();
    }
}

/// Reads the development home again.
pub(crate) fn refresh() {
    let Some(provider) = provider_of() else {
        workspace().update(|state| {
            state.load.loading = false;
            state.load.error = Some("no software provider is available".to_string());
        });
        return;
    };
    let sequence = workspace().with(|state| state.load.sequence + 1);
    workspace().update(|state| {
        state.load.sequence = sequence;
        state.load.loading = true;
        state.load.error = None;
    });
    task(async move {
        let answer = match blocking(move || provider.snapshot()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the software workspace did not answer: {error}")),
        };
        workspace().update(|state| {
            if state.load.sequence != sequence {
                return;
            }
            state.load.loading = false;
            state.load.loaded = true;
            match answer {
                Ok(snapshot) => {
                    state.load.snapshot = snapshot;
                    state.load.error = None;
                }
                Err(error) => state.load.error = Some(error),
            }
        });
    });
}

pub(crate) fn loading() -> bool {
    workspace().with(|state| state.load.loading)
}

pub(crate) fn load_error() -> Option<String> {
    workspace().with(|state| state.load.error.clone())
}

/// The installed versions of `kind`.
pub(crate) fn installed(kind: SoftwareKind) -> Vec<Package> {
    workspace().with(|state| {
        state
            .load
            .snapshot
            .installed
            .iter()
            .filter(|package| package.name == kind.key())
            .cloned()
            .collect()
    })
}

/// How many versions of `kind` are installed.
pub(crate) fn installed_count(kind: SoftwareKind) -> usize {
    workspace().with(|state| {
        state
            .load
            .snapshot
            .installed
            .iter()
            .filter(|package| package.name == kind.key())
            .count()
    })
}

/// The version `kind` runs by default, when one is chosen.
pub(crate) fn default_version(kind: SoftwareKind) -> Option<String> {
    workspace().with(|state| {
        let snapshot = &state.load.snapshot;
        let id = if kind == SoftwareKind::Node {
            snapshot.node_default.as_str()
        } else {
            snapshot
                .runtime_defaults
                .get(kind.key())
                .map(String::as_str)
                .unwrap_or_default()
        };
        if id.is_empty() {
            return None;
        }
        snapshot
            .installed
            .iter()
            .find(|package| package.id() == id)
            .map(|package| package.version.clone())
    })
}

/// Whether `package` is the default version of its software.
pub(crate) fn is_default(package: &Package) -> bool {
    workspace().with(|state| {
        let snapshot = &state.load.snapshot;
        let id = package.id();
        match SoftwareKind::from_key(&package.name) {
            Some(SoftwareKind::Node) => snapshot.node_default == id,
            Some(kind) => snapshot
                .runtime_defaults
                .get(kind.key())
                .is_some_and(|chosen| chosen == &id),
            None => false,
        }
    })
}

/// Whether no database instance uses this installed version. Other software
/// is never bound to an instance.
pub(crate) fn is_unused(id: &str) -> bool {
    workspace().with(|state| {
        let Some(package) = state
            .load
            .snapshot
            .installed
            .iter()
            .find(|package| package.id() == id)
        else {
            return false;
        };
        SoftwareKind::from_key(&package.name).is_some_and(SoftwareKind::is_database)
            && !state.load.snapshot.in_use.contains(id)
    })
}

/// An installed version by id.
pub(crate) fn installed_package(id: &str) -> Option<Package> {
    workspace().with(|state| {
        state
            .load
            .snapshot
            .installed
            .iter()
            .find(|package| package.id() == id)
            .cloned()
    })
}

/// What is known of an installed version's files, and the check that says so
/// when it still belongs to the recorded baseline.
pub(crate) fn integrity(id: &str) -> (String, Option<PackageIntegrityResult>) {
    workspace().with(|state| {
        let snapshot = &state.load.snapshot;
        let baseline = snapshot
            .integrity
            .get(id)
            .map(String::as_str)
            .unwrap_or_default();
        let formula = snapshot
            .installed
            .iter()
            .find(|package| package.id() == id)
            .is_some_and(|package| !package.formula.is_empty());
        let status = if formula {
            "Homebrew managed".to_string()
        } else if baseline.is_empty() {
            "Not recorded".to_string()
        } else {
            "Not checked".to_string()
        };
        match state
            .checks
            .get(id)
            .filter(|result| result.baseline == baseline)
        {
            Some(result) => (result.status.clone(), Some(result.clone())),
            None => (status, None),
        }
    })
}

/// The download cache's contents.
pub(crate) fn cache() -> DownloadCacheInfo {
    workspace().with(|state| state.load.snapshot.cache.clone())
}

pub(crate) fn cache_busy() -> bool {
    workspace().with(|state| state.cache_busy)
}

// --- Operations --------------------------------------------------------------

pub(crate) fn is_busy(id: &str) -> bool {
    workspace().with(|state| state.busy.contains_key(id))
}

/// Whether any package or cache operation is in flight.
pub(crate) fn operation_busy() -> bool {
    workspace().with(|state| !state.busy.is_empty() || state.cache_busy)
}

/// The latest progress line of a package operation.
pub(crate) fn progress(id: &str) -> Option<String> {
    workspace().with(|state| state.busy.get(id).cloned())
}

pub(crate) fn action_error() -> Option<String> {
    feedback().with(|fb| fb.action_error.clone())
}

fn set_action_error(message: String) {
    feedback().update(|fb| {
        fb.action_error = Some(message);
    });
}

fn show_notice(message: String) {
    clear_feedback();
    crate::toast::success(message);
}

fn show_info_notice(message: impl Into<String>) {
    clear_feedback();
    crate::toast::info(message);
}

/// Installs the previewed version after the review found it compatible.
pub(crate) fn install() {
    if !preview_matches() {
        return;
    }
    let Some((package, installed, compatible, needs_repair)) =
        preview_slot().with_untracked(|slot| {
            let details = slot.preview.as_ref()?.details.as_ref()?;
            Some((
                details.package.clone(),
                details.installed,
                details.compatible,
                details.installed
                    && details.integrity == "Needs repair"
                    && details.package.formula.is_empty(),
            ))
        })
    else {
        return;
    };
    if operation_busy() || installed || !compatible || needs_repair {
        return;
    }
    let Some(provider) = provider_of() else {
        set_action_error("no software provider is available".to_string());
        return;
    };
    let title = title_of(&package.name);
    let version = package.version.clone();
    perform_install(provider, package, title, version);
}

fn perform_install(provider: Arc<dyn Provider>, package: Package, title: String, version: String) {
    let id = package.id();
    workspace().update(|state| {
        state.busy.insert(id.clone(), format!("Installing {id}…"));
    });
    clear_feedback();
    let install_id = id.clone();
    task(async move {
        let answer = match blocking(move || provider.install(&package)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the install did not finish: {error}")),
        };
        let message = match &answer {
            Ok(()) => format!("Installed {title} {version}"),
            Err(_) => String::new(),
        };
        workspace().update(|state| {
            state.busy.remove(&install_id);
        });
        match &answer {
            Ok(()) => {
                clear_preview();
                crate::notify_task(
                    &format!("software-install-{install_id}"),
                    &format!("Installed {title} {version}"),
                    &message,
                    crate::store::Section::Development,
                    false,
                );
                show_notice(message);
                refresh();
            }
            Err(error) => {
                crate::notify_task(
                    &format!("software-install-{install_id}"),
                    &format!("Install failed: {title} {version}"),
                    error,
                    crate::store::Section::Development,
                    true,
                );
                set_action_error(error.clone());
            }
        }
    });
}

/// Asks before removing an installed version, as the Go screen does. A
/// Homebrew keg is only unregistered from the workspace.
pub(crate) fn request_uninstall(id: &str) {
    let Some(package) = installed_package(id) else {
        return;
    };
    let name = format!("{} {}", title_of(&package.name), package.version);
    let (title, description, button) = if package.formula.is_empty() {
        (
            format!("Uninstall {name}?"),
            format!("Removes {name} from Devbase. You can install it again later."),
            "Uninstall Version",
        )
    } else {
        (
            format!("Remove {name} from the workspace?"),
            "Devbase stops managing it. The shared Homebrew installation is kept.".to_string(),
            "Remove Version",
        )
    };
    let answer = alert_dialog()
        .title(title)
        .description(description)
        .cancel("cancel", "Cancel")
        .destructive("confirm", button)
        .show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if chosen {
            perform_uninstall(package);
        }
    });
}

fn perform_uninstall(package: Package) {
    if operation_busy() {
        return;
    }
    let Some(provider) = provider_of() else {
        set_action_error("no software provider is available".to_string());
        return;
    };
    let id = package.id();
    let label = format!("{} {}", title_of(&package.name), package.version);
    let formula = !package.formula.is_empty();
    workspace().update(|state| {
        state.busy.insert(id.clone(), format!("Removing {id}"));
    });
    clear_feedback();
    let remove_id = id.clone();
    let message = if formula {
        "Removed workspace registration. The shared Homebrew installation is kept.".to_string()
    } else {
        format!("Uninstalled {label}")
    };
    task(async move {
        let request = remove_id.clone();
        let answer = match blocking(move || provider.uninstall(&request)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the uninstall did not finish: {error}")),
        };
        workspace().update(|state| {
            state.busy.remove(&remove_id);
        });
        match &answer {
            Ok(()) => {
                clear_preview();
                crate::notify_task(
                    &format!("software-uninstall-{remove_id}"),
                    &format!("Removed {label}"),
                    &message,
                    crate::store::Section::Development,
                    false,
                );
                show_notice(message);
                refresh();
            }
            Err(error) => {
                let err_message = format!("{label}: {error}");
                crate::notify_task(
                    &format!("software-uninstall-{remove_id}"),
                    &format!("Remove failed: {label}"),
                    &err_message,
                    crate::store::Section::Development,
                    true,
                );
                set_action_error(err_message);
            }
        }
    });
}

/// Checks an installed version's files against its recorded index.
pub(crate) fn check_files(id: &str) {
    if operation_busy() || installed_package(id).is_none() {
        return;
    }
    let Some(provider) = provider_of() else {
        set_action_error("no software provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state
            .busy
            .insert(id.to_string(), "Checking files…".to_string());
    });
    clear_feedback();
    let check_id = id.to_string();
    task(async move {
        let request = check_id.clone();
        let answer = match blocking(move || provider.verify(&request)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the check did not finish: {error}")),
        };
        let label = installed_package(&check_id)
            .map(|package| format!("{} {}", title_of(&package.name), package.version))
            .unwrap_or_else(|| check_id.clone());
        workspace().update(|state| {
            state.busy.remove(&check_id);
            if let Ok(result) = &answer {
                state.checks.insert(check_id.clone(), result.clone());
            }
        });
        match answer {
            Ok(_) => {
                if feedback().with_untracked(|fb| fb.action_error.is_some()) {
                    feedback().update(|fb| fb.action_error = None);
                }
            }
            Err(error) => set_action_error(format!("{label}: {error}")),
        }
    });
}

/// Restores an installed archive package from its recorded source and
/// checksum, then checks the restored files.
pub(crate) fn repair(id: &str) {
    if operation_busy() {
        return;
    }
    let Some(package) = installed_package(id) else {
        return;
    };
    if !package.formula.is_empty() {
        set_action_error(format!(
            "Homebrew owns this shared installation; stop its users and run brew reinstall {}, then check files again",
            package.formula
        ));
        return;
    }
    let Some(provider) = provider_of() else {
        set_action_error("no software provider is available".to_string());
        return;
    };
    let repair_id = id.to_string();
    workspace().update(|state| {
        state
            .busy
            .insert(repair_id.clone(), format!("Repairing {repair_id}…"));
    });
    clear_feedback();
    task(async move {
        let request = repair_id.clone();
        let verify_provider = Arc::clone(&provider);
        let repaired = match blocking(move || provider.repair(&request)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the repair did not finish: {error}")),
        };
        let check = match &repaired {
            Ok(()) => {
                let request = repair_id.clone();
                match blocking(move || verify_provider.verify(&request)).await {
                    Ok(answer) => answer.ok(),
                    Err(_) => None,
                }
            }
            Err(_) => None,
        };
        let label = installed_package(&repair_id)
            .map(|package| format!("{} {}", title_of(&package.name), package.version))
            .unwrap_or_else(|| repair_id.clone());
        workspace().update(|state| {
            state.busy.remove(&repair_id);
            if repaired.is_ok()
                && let Some(result) = check
            {
                state.checks.insert(repair_id.clone(), result);
            }
        });
        match repaired {
            Ok(()) => {
                clear_preview();
                crate::notify_task(
                    &format!("software-repair-{repair_id}"),
                    &format!("Repaired {label}"),
                    "Restored the same exact version.",
                    crate::store::Section::Development,
                    false,
                );
                show_notice("Restored the same exact version.".to_string());
                refresh();
            }
            Err(error) => {
                let err_message = format!("{label}: Repair did not complete: {error}");
                crate::notify_task(
                    &format!("software-repair-{repair_id}"),
                    &format!("Repair failed: {label}"),
                    &err_message,
                    crate::store::Section::Development,
                    true,
                );
                set_action_error(err_message);
            }
        }
    });
}

/// Asks before clearing the download cache, then removes its archives.
pub(crate) fn request_clear_cache() {
    if operation_busy() {
        return;
    }
    let answer = alert_dialog()
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
    let Some(provider) = provider_of() else {
        set_action_error("no software provider is available".to_string());
        return;
    };
    workspace().update(|state| {
        state.cache_busy = true;
    });
    clear_feedback();
    task(async move {
        let answer = match blocking(move || provider.clear_cache()).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the cache was not cleared: {error}")),
        };
        workspace().update(|state| {
            state.cache_busy = false;
        });
        match answer {
            Ok(removed) => {
                let notice = if removed.files == 0 {
                    "Download cache is already empty.".to_string()
                } else {
                    format!(
                        "Cleared {} ({}) from the download cache.",
                        plural(removed.files as usize, "archive"),
                        format_bytes(removed.bytes)
                    )
                };
                show_notice(notice);
                preview_slot().update(|slot| {
                    if let Some(preview) = slot.preview.as_mut()
                        && let Some(details) = preview.details.as_mut()
                    {
                        details.cached = false;
                    }
                });
            }
            Err(error) => {
                set_action_error(error);
            }
        }
        refresh();
    });
}

/// Copies a value and says so for a moment.
pub(crate) fn copy(value: &str) {
    set_clipboard(value.to_string());
    show_info_notice("Copied");
}

// --- Formatting --------------------------------------------------------------

/// `name` as the screen writes it: the catalog's title, or the state's name.
pub(crate) fn title_of(name: &str) -> String {
    SoftwareKind::from_key(name)
        .map(SoftwareKind::title)
        .unwrap_or(name)
        .to_string()
}

/// `n` and its noun, singular when one.
pub(crate) fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// A byte count in the Go app's style: `0 B`, `12.3 MiB`.
pub(crate) fn format_bytes(bytes: i64) -> String {
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
