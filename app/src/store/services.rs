//! The launchd inspector's state: the source shown, its search and filter,
//! the items read from the source, the item chosen and its detail, and the
//! action in flight.
//!
//! The provider's calls do disk and process work, so every one runs on a
//! blocking thread and writes the store when it answers. Render code reads
//! the store and the provider's pure `actions` / `confirm`; it never calls
//! `list`, `detail` or `do_action`.
//!
//! Tests install a provider of their own before the surface starts with
//! [`use_fake_providers`] and [`install_provider`]; production builds the
//! three launchd providers on first use.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use devcore::{
    helper::Client as HelperClient,
    launchd::SystemRunner,
    service::{LogSource, ServiceRegistry},
};
use nsh_base::alert_dialog;
use nsh_core::prelude::*;

use crate::{logs, platform};

pub(crate) use devcore::service::{Action, Item, LogFile, Provider, Restriction, Row, Source};

/// What the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Filter {
    #[default]
    All,
    Running,
    Stopped,
}

impl Filter {
    pub(crate) const ALL: [Filter; 3] = [Filter::All, Filter::Running, Filter::Stopped];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Running => "Running",
            Filter::Stopped => "Stopped",
        }
    }

    fn passes(self, item: &Item) -> bool {
        match self {
            Filter::All => true,
            Filter::Running => item.up(),
            Filter::Stopped => !item.up(),
        }
    }
}

/// The two tabs of the inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DetailTab {
    #[default]
    Info,
    Logs,
}

/// What the log tab's body is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogBodyMode {
    Error,
    Reading,
    Empty,
    Lines,
}

/// Which source scope the services list displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceScope {
    /// Aggregated view combining all five service sources.
    All,
    /// One individual service source.
    Source(Source),
}

impl Default for ServiceScope {
    fn default() -> Self {
        ServiceScope::Source(platform::default_user_source())
    }
}

impl ServiceScope {
    pub(crate) fn title(self) -> &'static str {
        match self {
            ServiceScope::All => "All Items",
            ServiceScope::Source(source) => source_title(source),
        }
    }

    pub(crate) fn source(self) -> Option<Source> {
        match self {
            ServiceScope::All => None,
            ServiceScope::Source(source) => Some(source),
        }
    }
}

/// Human-readable plural label for a service source in the Services rail & header.
pub(crate) const fn source_title(source: Source) -> &'static str {
    match source {
        Source::SystemdUser => "User Units",
        Source::SystemdSystem => "System Units",
        other => other.title(),
    }
}

/// The sources the inspector shows for the current host platform, in order.
pub(crate) const SOURCES: &[Source] = Source::platform_all();

/// One source's last read.
#[derive(Debug, Clone, Default)]
struct SourceLoad {
    items: Vec<Item>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    restriction: Option<Restriction>,
    /// Bumped for every read, so an older answer never replaces a newer one.
    sequence: u64,
}

/// The item chosen and what the provider said about it.
#[derive(Debug, Clone)]
struct DetailState {
    item_id: String,
    rows: Vec<Row>,
    logs: Vec<LogFile>,
    loading: bool,
    error: Option<String>,
    tab: DetailTab,
    sequence: u64,
}

impl DetailState {
    fn loading(item_id: String) -> Self {
        DetailState {
            item_id,
            rows: Vec::new(),
            logs: Vec::new(),
            loading: true,
            error: None,
            tab: DetailTab::Info,
            sequence: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct Services {
    active: ServiceScope,
    query: String,
    filter: Filter,
    loads: BTreeMap<Source, SourceLoad>,
    selected: Option<String>,
    detail: Option<DetailState>,
    busy: BTreeSet<String>,
    action_error: Option<String>,
}

impl Default for Services {
    fn default() -> Self {
        Services {
            active: ServiceScope::default(),
            query: String::new(),
            filter: Filter::All,
            loads: BTreeMap::new(),
            selected: None,
            detail: None,
            busy: BTreeSet::new(),
            action_error: None,
        }
    }
}

/// Pointer hover state kept off [`Services`] so moving the pointer across rows
/// never wakes list or inspector bindings.
#[derive(Debug, Clone, Default)]
struct HoverState {
    item: Option<String>,
    row: Option<String>,
}

/// Visible log-tab state kept off [`Services`] so 1-second log polls only wake
/// the log view and never rebuild the inspector or list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LogTailState {
    item_id: Option<String>,
    file_index: usize,
    file: Option<LogFile>,
    lines: Option<Vec<String>>,
    error: Option<String>,
    truncated: bool,
}

#[derive(Debug, Default)]
struct LogPollInner {
    reading: bool,
    sequence: u64,
}

/// Non-reactive in-flight log poll bookkeeping, scoped to the active runtime.
#[derive(Clone, Default)]
struct LogPoll(Rc<RefCell<LogPollInner>>);

/// Passive probe of whether the privileged helper is installed before Settings
/// has loaded its own status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct HelperProbeState(Option<bool>);

/// Persisted width of the resizable Services inspector, kept off [`Services`]
/// so dragging the handle never wakes list or detail bindings.
#[derive(Debug, Clone, Copy, PartialEq)]
struct InspectorLayout {
    width: f32,
}

impl Default for InspectorLayout {
    fn default() -> Self {
        Self { width: 336.0 }
    }
}

fn services() -> Signal<Services> {
    global::<Services>()
}

fn hover_state() -> Signal<HoverState> {
    global::<HoverState>()
}

fn log_tail() -> Signal<LogTailState> {
    global::<LogTailState>()
}

fn log_poll() -> Signal<LogPoll> {
    global::<LogPoll>()
}

fn helper_probe() -> Signal<HelperProbeState> {
    global::<HelperProbeState>()
}

fn inspector_layout() -> Signal<InspectorLayout> {
    global::<InspectorLayout>()
}

pub(crate) fn inspector_width() -> f32 {
    inspector_layout().with(|state| state.width)
}

pub(crate) fn set_inspector_width(width: f32) {
    let clamped = width.clamp(280.0, 560.0);
    if (inspector_layout().with_untracked(|state| state.width) - clamped).abs() > 0.5 {
        inspector_layout().set(InspectorLayout { width: clamped });
    }
}

/// Runs store work outside the view that asked for it: a row or pane the
/// store change rebuilds must not cancel the task that is changing it.
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

// --- Providers ---------------------------------------------------------------

type LoginItemAdder = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// Test providers, installed before the surface starts. The store never
/// reaches launchd, Homebrew or login items while `FAKE_MODE` is on.
static FAKES: LazyLock<Mutex<BTreeMap<Source, Arc<dyn Provider>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static FAKE_LOGIN_ITEM_ADDER: LazyLock<Mutex<Option<LoginItemAdder>>> =
    LazyLock::new(|| Mutex::new(None));
static FAKE_MODE: AtomicBool = AtomicBool::new(false);

fn fakes() -> &'static Mutex<BTreeMap<Source, Arc<dyn Provider>>> {
    &FAKES
}

/// The platform's default service provider registry, initialized once.
static REGISTRY: LazyLock<ServiceRegistry> = LazyLock::new(ServiceRegistry::platform_default);

/// The machine's macOS login items provider, used by `add_login_item_path`.
#[cfg(target_os = "macos")]
static LOGIN_ITEM: LazyLock<Arc<devcore::loginitem::Provider>> =
    LazyLock::new(|| Arc::new(devcore::loginitem::Provider::default_provider()));

/// Passively checks whether the privileged helper is installed without
/// launching it or prompting for a password.
pub(crate) fn refresh_helper_status() {
    if FAKE_MODE.load(Ordering::Relaxed) {
        return;
    }
    task(async move {
        let ready = blocking(|| HelperClient::default_client().ready())
            .await
            .unwrap_or(false);
        if helper_probe().with_untracked(|state| state.0) != Some(ready) {
            helper_probe().set(HelperProbeState(Some(ready)));
        }
    });
}

/// Whether the privileged helper is currently installed.
///
/// Follows Settings' reactive helper status once Settings has loaded or changed
/// it, and falls back to the passive probe from [`HelperClient::default_client`]
/// when Services is opened before Settings.
pub(crate) fn helper_installed() -> bool {
    let system = crate::store::system::helper_status();
    if system.installed || !system.message.is_empty() {
        return system.installed;
    }
    helper_probe()
        .get()
        .0
        .unwrap_or_else(|| !FAKE_MODE.load(Ordering::Relaxed))
}

fn provider_of(source: Source) -> Option<Arc<dyn Provider>> {
    if let Some(provider) = fakes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&source)
        .cloned()
    {
        return Some(provider);
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return None;
    }
    REGISTRY.provider(source)
}

/// Makes the surface use test providers instead of launchd, Homebrew or login
/// items. Call it before starting the headless surface, then
/// [`install_provider`] for each source. Tests only.
pub fn use_fake_providers() {
    FAKE_MODE.store(true, Ordering::Relaxed);
    fakes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    *FAKE_LOGIN_ITEM_ADDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Gives a source a provider of the test's own. Tests only.
pub fn install_provider(source: Source, provider: Arc<dyn Provider>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    fakes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(source, provider);
}

/// Installs a fake handler for adding login items. Tests only.
pub fn install_login_item_adder(adder: LoginItemAdder) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *FAKE_LOGIN_ITEM_ADDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(adder);
}

/// The provider of a source, for its pure [`Provider::actions`] and
/// [`Provider::confirm`] (no work happens in them).
pub(crate) fn provider(source: Source) -> Option<Arc<dyn Provider>> {
    provider_of(source)
}

// --- Source and list ---------------------------------------------------------

/// The scope the list shows (`All Items` or one individual source).
pub(crate) fn active_scope() -> ServiceScope {
    services().with(|state| state.active)
}

/// The individual source the list shows, or `None` when viewing `All Items`.
pub(crate) fn active_source() -> Option<Source> {
    active_scope().source()
}

fn load_source_if_needed(source: Source) {
    let load = services().with(|state| state.loads.get(&source).cloned());
    if load.is_none_or(|load| !load.loaded && !load.loading) {
        refresh(source);
    }
}

fn load_all_missing() {
    for source in SOURCES.iter().copied() {
        if provider_of(source).is_some() {
            load_source_if_needed(source);
        }
    }
}

/// Shows `scope`, clearing the item chosen and reading any unloaded sources.
pub(crate) fn select_scope(scope: ServiceScope) {
    services().update(|state| {
        state.active = scope;
        state.selected = None;
        state.detail = None;
        state.action_error = None;
    });
    clear_hover();
    reset_log_tail(None, None);
    match scope {
        ServiceScope::All => {
            refresh_helper_status();
            load_all_missing();
        }
        ServiceScope::Source(source) => {
            if source.is_privileged() || source == Source::SystemAgent {
                refresh_helper_status();
            }
            load_source_if_needed(source);
        }
    }
}

/// Shows `source`, clearing the item chosen and reading it when it has not
/// been read yet.
pub(crate) fn select_source(source: Source) {
    select_scope(ServiceScope::Source(source));
}

/// Reads the shown source(s) the first time the screen appears.
pub(crate) fn load_active() {
    refresh_helper_status();
    match active_scope() {
        ServiceScope::All => load_all_missing(),
        ServiceScope::Source(source) => load_source_if_needed(source),
    }
}

/// Reads `source` again.
pub(crate) fn refresh(source: Source) {
    if source.is_privileged() || matches!(source, Source::SystemAgent | Source::Brew) {
        refresh_helper_status();
    }
    let Some(provider) = provider_of(source) else {
        services().update(|state| {
            let load = state.loads.entry(source).or_default();
            load.loading = false;
            load.restriction = None;
            load.error = Some("no provider is available for this source".to_string());
        });
        return;
    };
    let sequence =
        services().with(|state| state.loads.get(&source).map_or(1, |load| load.sequence + 1));
    services().update(|state| {
        let load = state.loads.entry(source).or_default();
        load.sequence = sequence;
        load.loading = true;
        load.error = None;
    });
    task(async move {
        let (answer, restriction) = match blocking(move || {
            let answer = provider.list().map_err(|error| error.to_string());
            let restriction = provider
                .as_restricted()
                .and_then(|restricted| restricted.restriction());
            (answer, restriction)
        })
        .await
        {
            Ok(pair) => pair,
            Err(error) => (Err(format!("the source did not answer: {error}")), None),
        };
        let mut selection_cleared = false;
        services().update(|state| {
            let mut outdated = false;
            {
                let load = state.loads.entry(source).or_default();
                if load.sequence != sequence {
                    outdated = true;
                } else {
                    load.loading = false;
                    load.loaded = true;
                    load.restriction = restriction;
                    match answer {
                        Ok(items) => {
                            load.items = items;
                            load.error = None;
                        }
                        Err(error) => load.error = Some(error),
                    }
                }
            }
            if outdated {
                return;
            }
            let drop_selection = match state.active {
                ServiceScope::Source(active_source) => {
                    active_source == source
                        && state.selected.as_ref().is_some_and(|selected| {
                            state.loads.get(&source).is_none_or(|load| {
                                !load.items.iter().any(|item| &item.id == selected)
                            })
                        })
                }
                ServiceScope::All => state.selected.as_ref().is_some_and(|selected| {
                    !state
                        .loads
                        .values()
                        .any(|load| load.items.iter().any(|item| &item.id == selected))
                }),
            };
            if drop_selection {
                state.selected = None;
                state.detail = None;
                selection_cleared = true;
            }
        });
        if selection_cleared {
            reset_log_tail(None, None);
        }
    });
}

/// Reads the shown source(s) again, on request, and refreshes the selected
/// item's detail rows when one is open.
pub(crate) fn refresh_active() {
    let scope = active_scope();
    match scope {
        ServiceScope::All => {
            refresh_helper_status();
            for source in SOURCES.iter().copied() {
                if provider_of(source).is_some() {
                    refresh(source);
                }
            }
        }
        ServiceScope::Source(source) => refresh(source),
    }
    if let Some(id) = selected_id()
        && let Some(item) = item_by_id(&id)
        && (scope == ServiceScope::All || scope == ServiceScope::Source(item.source))
    {
        load_detail(item);
    }
}

/// Reads `source` again only if it has already been loaded in this session.
pub(crate) fn refresh_if_loaded(source: Source) {
    let loaded = services().with(|state| state.loads.get(&source).is_some_and(|load| load.loaded));
    if loaded {
        refresh(source);
    }
}

/// Items for `source` if it has been loaded, or an empty vector otherwise.
pub(crate) fn loaded_items(source: Source) -> Vec<Item> {
    services().with(|state| {
        state
            .loads
            .get(&source)
            .filter(|load| load.loaded)
            .map(|load| load.items.clone())
            .unwrap_or_default()
    })
}

/// Selects `source` and focuses `id` if it is already loaded.
pub(crate) fn open_item(source: Source, id: &str) {
    if active_source() != Some(source) {
        select_source(source);
    }
    select_item(id);
}

/// Refreshes the source modified by the plist editor and shows a notice.
pub(crate) fn handle_editor_saved(saved: crate::editor::SavedEvent) {
    let source = saved.source;
    let label = saved.label.clone();
    let saved_path = saved.path.clone();
    let message = if saved.loaded {
        format!("{label} saved and loaded")
    } else {
        format!("{label} saved")
    };
    services().update(|state| {
        state.action_error = None;
    });
    crate::toast::success(message);
    refresh(source);
    if let Some(selected) = selected_id()
        && let Some(item) = item_by_id(&selected)
        && item.source == source
        && item.path == saved_path
    {
        later(Duration::from_millis(250), move || {
            if is_selected(&item.id) {
                load_detail(item.clone());
            }
        });
    }
}

pub(crate) fn query() -> String {
    services().with(|state| state.query.clone())
}

/// Filters the list by name, owner and path.
pub(crate) fn set_query(next: String) {
    services().update(|state| state.query = next);
}

pub(crate) fn clear_search() {
    services().update(|state| {
        state.query.clear();
        state.filter = Filter::All;
    });
}

pub(crate) fn filter() -> Filter {
    services().with(|state| state.filter)
}

pub(crate) fn set_filter(next: Filter) {
    services().update(|state| state.filter = next);
}

/// The items the list shows for the active scope: the search and filter let
/// them through. In `All Items`, items from all loaded sources are combined
/// and sorted deterministically (running first, then alphabetical by name and
/// source).
pub(crate) fn visible_items() -> Vec<Item> {
    services().with(|state| {
        let query = &state.query;
        let filter = state.filter;
        match state.active {
            ServiceScope::Source(source) => {
                let Some(load) = state.loads.get(&source) else {
                    return Vec::new();
                };
                load.items
                    .iter()
                    .filter(|item| item.matches(query) && filter.passes(item))
                    .cloned()
                    .collect()
            }
            ServiceScope::All => {
                let mut items: Vec<Item> = SOURCES
                    .iter()
                    .filter_map(|source| state.loads.get(source))
                    .flat_map(|load| load.items.iter())
                    .filter(|item| item.matches(query) && filter.passes(item))
                    .cloned()
                    .collect();
                items.sort_by(|a, b| {
                    b.up()
                        .cmp(&a.up())
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                        .then_with(|| a.name.cmp(&b.name))
                        .then_with(|| a.source.cmp(&b.source))
                        .then_with(|| a.id.cmp(&b.id))
                });
                items
            }
        }
    })
}

/// How many items of `source` there are, and how many run.
pub(crate) fn counts(source: Source) -> (usize, usize) {
    services().with(|state| {
        let Some(load) = state.loads.get(&source) else {
            return (0, 0);
        };
        let running = load.items.iter().filter(|item| item.up()).count();
        (load.items.len(), running)
    })
}

/// How many items exist across all loaded sources, and how many run.
pub(crate) fn total_counts() -> (usize, usize) {
    services().with(|state| {
        let mut total = 0;
        let mut running = 0;
        for source in SOURCES {
            if let Some(load) = state.loads.get(source) {
                total += load.items.len();
                running += load.items.iter().filter(|item| item.up()).count();
            }
        }
        (total, running)
    })
}

pub(crate) fn active_loading() -> bool {
    services().with(|state| match state.active {
        ServiceScope::Source(source) => state.loads.get(&source).is_some_and(|load| load.loading),
        ServiceScope::All => SOURCES
            .iter()
            .any(|source| state.loads.get(source).is_some_and(|load| load.loading)),
    })
}

pub(crate) fn active_error() -> Option<String> {
    services().with(|state| match state.active {
        ServiceScope::Source(source) => {
            state.loads.get(&source).and_then(|load| load.error.clone())
        }
        ServiceScope::All => {
            let errors: Vec<String> = SOURCES
                .iter()
                .filter(|source| provider_of(**source).is_some())
                .filter_map(|source| {
                    state
                        .loads
                        .get(source)
                        .and_then(|load| load.error.as_deref())
                        .map(|error| format!("{}: {error}", source.title()))
                })
                .collect();
            (!errors.is_empty()).then(|| errors.join(" · "))
        }
    })
}

pub(crate) fn active_restriction() -> Option<Restriction> {
    services().with(|state| match state.active {
        ServiceScope::Source(source) => state
            .loads
            .get(&source)
            .and_then(|load| load.restriction.clone()),
        ServiceScope::All => None,
    })
}

/// Opens System Settings for the active restricted source (e.g. Full Disk
/// Access for Login Items). Never touches host TCC settings in `FAKE_MODE`
/// unless a fake restricted provider is installed.
pub(crate) fn open_restriction_settings() {
    let Some(source) = active_source() else {
        return;
    };
    let Some(provider) = provider_of(source) else {
        return;
    };
    task(async move {
        let answer = match blocking(move || match provider.as_restricted() {
            Some(restricted) => restricted.open_settings().map_err(|e| e.to_string()),
            None => Ok(()),
        })
        .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("could not open settings: {error}")),
        };
        if let Err(error) = answer {
            set_action_error(&error);
        }
    });
}

/// Runs a one-time authenticated read for the active restricted source (e.g.
/// `sfltool dumpbtm` for Login Items) and refreshes the list when done.
pub(crate) fn read_restricted_once() {
    let Some(source) = active_source() else {
        return;
    };
    let Some(provider) = provider_of(source) else {
        return;
    };
    task(async move {
        let answer = match blocking(move || match provider.as_restricted() {
            Some(restricted) => restricted.read_once().map_err(|e| e.to_string()),
            None => Ok(()),
        })
        .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("could not read login items: {error}")),
        };
        match answer {
            Ok(()) => refresh(source),
            Err(error) => set_action_error(&error),
        }
    });
}

/// Opens a file dialog to choose an `.app` bundle and adds it to Login Items.
pub(crate) fn add_login_item() {
    task(async move {
        let dialog = nsh_core::file_dialog::OpenDialog::files()
            .title("Add Login Item")
            .button_label("Add")
            .directory("/Applications")
            .filter(nsh_core::file_dialog::FileFilter::new(
                "Applications",
                ["app"],
            ));
        let answer = pick_paths(dialog).await;
        if let Ok(Some(paths)) = answer
            && let Some(path) = paths.into_iter().next()
        {
            add_login_item_path(path);
        }
    });
}

/// Adds the `.app` bundle at `path` to Login Items and refreshes the list.
pub(crate) fn add_login_item_path(path: PathBuf) {
    let fake_mode = FAKE_MODE.load(Ordering::Relaxed);
    let fake_adder = FAKE_LOGIN_ITEM_ADDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if fake_mode && fake_adder.is_none() {
        set_action_error("no login item provider is available");
        return;
    }
    let display_name = path
        .file_name()
        .map(|name| name.to_string_lossy().trim_end_matches(".app").to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string());
    task(async move {
        let answer = match blocking(move || {
            if let Some(adder) = fake_adder {
                adder(&path)
            } else {
                #[cfg(target_os = "macos")]
                {
                    LOGIN_ITEM.add(&path).map_err(|error| error.to_string())
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = &path;
                    Err("macOS Login Items are not supported on Linux".to_string())
                }
            }
        })
        .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("could not add login item: {error}")),
        };
        match answer {
            Ok(()) => {
                services().update(|state| {
                    state.action_error = None;
                });
                crate::toast::success(format!("{display_name} added"));
                refresh(Source::LoginItem);
            }
            Err(error) => set_action_error(&error),
        }
    });
}

// --- Selection and detail ----------------------------------------------------

pub(crate) fn item_by_id(id: &str) -> Option<Item> {
    services().with(|state| {
        state
            .loads
            .values()
            .flat_map(|load| load.items.iter())
            .find(|item| item.id == id)
            .cloned()
    })
}

pub(crate) fn selected_id() -> Option<String> {
    services().with(|state| state.selected.clone())
}

pub(crate) fn is_selected(id: &str) -> bool {
    services().with(|state| state.selected.as_deref() == Some(id))
}

/// Chooses the item with `id` and reads its detail.
pub(crate) fn select_item(id: &str) {
    if is_selected(id) {
        return;
    }
    let Some(item) = item_by_id(id) else {
        return;
    };
    services().update(|state| {
        state.selected = Some(item.id.clone());
        state.detail = Some(DetailState::loading(item.id.clone()));
        state.action_error = None;
    });
    set_hovered_row(None);
    reset_log_tail(Some(item.id.clone()), None);
    load_detail(item);
}

/// Closes the inspector.
pub(crate) fn clear_selection() {
    services().update(|state| {
        state.selected = None;
        state.detail = None;
    });
    set_hovered_row(None);
    reset_log_tail(None, None);
}

fn load_detail(item: Item) {
    let Some(provider) = provider_of(item.source) else {
        set_detail_error(&item.id, "no provider is available for this source");
        return;
    };
    let id = item.id.clone();
    let sequence = services().with(|state| {
        state
            .detail
            .as_ref()
            .map_or(1, |detail| detail.sequence + 1)
    });
    services().update(|state| {
        let Some(detail) = state.detail.as_mut() else {
            return;
        };
        detail.sequence = sequence;
        detail.loading = true;
        detail.error = None;
    });
    let journal_fallback = item
        .journal_unit()
        .map(|(unit, system)| LogFile::journal("Systemd journal", unit, system));
    task(async move {
        let answer =
            match blocking(move || provider.detail(&item).map_err(|error| error.to_string())).await
            {
                Ok(answer) => answer,
                Err(error) => Err(format!("the detail read failed: {error}")),
            };
        let mut log_update: Option<(Option<LogFile>, bool)> = None;
        services().update(|state| {
            let Some(detail) = state.detail.as_mut() else {
                return;
            };
            if detail.item_id != id || detail.sequence != sequence {
                return;
            }
            detail.loading = false;
            match answer {
                Ok(mut read) => {
                    if read.logs.is_empty()
                        && let Some(journal) = journal_fallback.clone()
                    {
                        read.logs.push(journal);
                    }
                    let logs_changed = detail.logs != read.logs;
                    detail.rows = read.rows;
                    detail.logs = read.logs;
                    if logs_changed {
                        let first = detail.logs.first().cloned();
                        log_update = Some((first, detail.tab == DetailTab::Logs));
                    }
                }
                Err(error) => detail.error = Some(error),
            }
        });
        if let Some((first_file, on_logs_tab)) = log_update {
            reset_log_tail(Some(id), first_file);
            if on_logs_tab {
                load_log();
            }
        }
    });
}

fn set_detail_error(id: &str, message: &str) {
    services().update(|state| {
        let Some(detail) = state.detail.as_mut() else {
            return;
        };
        if detail.item_id == id {
            detail.loading = false;
            detail.error = Some(message.to_string());
        }
    });
}

pub(crate) fn detail_rows(id: &str) -> Vec<Row> {
    services().with(|state| match state.detail.as_ref() {
        Some(detail) if detail.item_id == id => detail.rows.clone(),
        _ => Vec::new(),
    })
}

pub(crate) fn detail_logs(id: &str) -> Vec<LogFile> {
    services().with(|state| match state.detail.as_ref() {
        Some(detail) if detail.item_id == id => detail.logs.clone(),
        _ => Vec::new(),
    })
}

pub(crate) fn detail_loading(id: &str) -> bool {
    services().with(|state| {
        state
            .detail
            .as_ref()
            .is_some_and(|detail| detail.item_id == id && detail.loading && detail.rows.is_empty())
    })
}

pub(crate) fn detail_error(id: &str) -> Option<String> {
    services().with(|state| {
        state
            .detail
            .as_ref()
            .filter(|detail| detail.item_id == id)
            .and_then(|detail| detail.error.clone())
    })
}

pub(crate) fn has_logs(id: &str) -> bool {
    services().with(|state| {
        state
            .detail
            .as_ref()
            .is_some_and(|detail| detail.item_id == id && !detail.logs.is_empty())
    })
}

pub(crate) fn detail_tab() -> DetailTab {
    services().with(|state| {
        state
            .detail
            .as_ref()
            .map_or(DetailTab::Info, |detail| detail.tab)
    })
}

pub(crate) fn detail_log_file() -> usize {
    log_tail().with(|state| state.file_index)
}

pub(crate) fn set_detail_tab(tab: DetailTab) {
    services().update(|state| {
        if let Some(detail) = state.detail.as_mut() {
            detail.tab = tab;
        }
    });
    if tab == DetailTab::Logs {
        load_log();
    }
}

fn reset_log_tail(item_id: Option<String>, file: Option<LogFile>) {
    log_poll().with_untracked(|poll| {
        let mut inner = poll.0.borrow_mut();
        inner.sequence += 1;
        inner.reading = false;
    });
    let next = LogTailState {
        item_id,
        file_index: 0,
        file,
        lines: None,
        error: None,
        truncated: false,
    };
    if log_tail().with_untracked(|state| *state != next) {
        log_tail().set(next);
    }
}

/// Chooses which log file the log tab shows, and reads it.
pub(crate) fn set_log_file(index: usize) {
    let target = services().with_untracked(|state| {
        let detail = state.detail.as_ref()?;
        let file = detail.logs.get(index)?.clone();
        Some((detail.item_id.clone(), file))
    });
    let Some((item_id, file)) = target else {
        return;
    };
    let unchanged = log_tail().with_untracked(|state| {
        state.item_id.as_deref() == Some(item_id.as_str()) && state.file_index == index
    });
    if unchanged {
        return;
    }
    log_poll().with_untracked(|poll| {
        let mut inner = poll.0.borrow_mut();
        inner.sequence += 1;
        inner.reading = false;
    });
    log_tail().set(LogTailState {
        item_id: Some(item_id),
        file_index: index,
        file: Some(file),
        lines: None,
        error: None,
        truncated: false,
    });
    load_log();
}

pub(crate) fn active_log_file() -> Option<LogFile> {
    log_tail().with(|state| state.file.clone())
}

pub(crate) fn active_log_file_untracked() -> Option<LogFile> {
    log_tail().with_untracked(|state| state.file.clone())
}

pub(crate) fn log_body_mode() -> LogBodyMode {
    log_tail().with(|state| {
        if state.error.is_some() {
            LogBodyMode::Error
        } else {
            match state.lines.as_deref() {
                None => LogBodyMode::Reading,
                Some([]) => LogBodyMode::Empty,
                Some(_) => LogBodyMode::Lines,
            }
        }
    })
}

pub(crate) fn log_error() -> Option<String> {
    log_tail().with(|state| state.error.clone())
}

pub(crate) fn log_truncated() -> bool {
    log_tail().with(|state| state.truncated)
}

pub(crate) fn log_line_count() -> usize {
    log_tail().with(|state| state.lines.as_ref().map_or(0, Vec::len))
}

pub(crate) fn indexed_log_lines() -> Vec<(usize, String)> {
    log_tail().with(|state| {
        state
            .lines
            .as_ref()
            .map(|lines| lines.iter().cloned().enumerate().collect())
            .unwrap_or_default()
    })
}

fn read_journal_tail(unit: &str, system: bool) -> Result<logs::Tail, String> {
    let raw = devcore::systemd::read_journal_logs(
        &SystemRunner::new(),
        unit,
        system,
        devcore::systemd::DEFAULT_JOURNAL_LINES,
    )
    .map_err(|error| error.to_string())?;
    let trimmed = raw.trim_end_matches('\n').replace("\r\n", "\n");
    if trimmed.is_empty() {
        return Ok(logs::Tail::default());
    }
    let lines: Vec<String> = trimmed.split('\n').map(str::to_owned).collect();
    let truncated = lines.len() >= devcore::systemd::DEFAULT_JOURNAL_LINES;
    Ok(logs::Tail {
        lines,
        truncated,
        size: raw.len() as u64,
        modified: None,
    })
}

/// Reads the shown log's end (file tail or `journalctl` output). Called when
/// the log tab opens and once a second while it is open.
pub(crate) fn load_log() {
    let Some((id, file_index, file)) = services().with_untracked(|state| {
        let detail = state.detail.as_ref()?;
        if detail.tab != DetailTab::Logs {
            return None;
        }
        let file_index = log_tail().with_untracked(|tail| tail.file_index);
        let file = detail.logs.get(file_index)?.clone();
        Some((detail.item_id.clone(), file_index, file))
    }) else {
        return;
    };
    let sequence = log_poll().with_untracked(|poll| {
        let mut inner = poll.0.borrow_mut();
        if inner.reading {
            return None;
        }
        inner.sequence += 1;
        inner.reading = true;
        Some(inner.sequence)
    });
    let Some(sequence) = sequence else {
        return;
    };
    task(async move {
        let answer = match blocking(move || match file.source() {
            LogSource::File(path) => logs::tail(path),
            LogSource::Journal { unit, system } => read_journal_tail(unit, system),
        })
        .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("the log read failed: {error}")),
        };
        let valid = log_poll().with_untracked(|poll| {
            let mut inner = poll.0.borrow_mut();
            if inner.sequence != sequence {
                return false;
            }
            inner.reading = false;
            true
        });
        if !valid {
            return;
        }
        let still_active = log_tail().with_untracked(|tail| {
            tail.item_id.as_deref() == Some(id.as_str()) && tail.file_index == file_index
        });
        if !still_active {
            return;
        }
        match answer {
            Ok(tail) => {
                let changed = log_tail().with_untracked(|state| {
                    state.error.is_some()
                        || state.truncated != tail.truncated
                        || state.lines.as_ref() != Some(&tail.lines)
                });
                if changed {
                    log_tail().update(|state| {
                        state.error = None;
                        state.lines = Some(tail.lines);
                        state.truncated = tail.truncated;
                    });
                }
            }
            Err(error) => {
                let changed = log_tail()
                    .with_untracked(|state| state.error.as_deref() != Some(error.as_str()));
                if changed {
                    log_tail().update(|state| {
                        state.error = Some(error);
                    });
                }
            }
        }
    });
}

/// Keeps the log tab following the file while it is open.
pub(crate) fn tick_log() {
    let reading = log_poll().with_untracked(|poll| poll.0.borrow().reading);
    if reading {
        return;
    }
    let active = services().with_untracked(|state| {
        state
            .detail
            .as_ref()
            .is_some_and(|detail| detail.tab == DetailTab::Logs && !detail.logs.is_empty())
    });
    if active {
        load_log();
    }
}

// --- Actions -----------------------------------------------------------------

pub(crate) fn is_busy(id: &str) -> bool {
    services().with(|state| state.busy.contains(id))
}

pub(crate) fn action_error() -> Option<String> {
    services().with(|state| state.action_error.clone())
}

/// Asks first when the provider says an action needs a question; otherwise
/// runs it at once.
pub(crate) fn request_action(item: Item, action: Action) {
    let Some(provider) = provider_of(item.source) else {
        set_action_error("no provider is available for this source");
        return;
    };
    if let Some(message) = provider.confirm(&item, action) {
        let title = format!("{} {}?", action.label(), item.name);
        let mut dialog = alert_dialog()
            .title(title)
            .description(message)
            .cancel("cancel", "Cancel");
        dialog = if action == Action::Remove {
            dialog.destructive("confirm", action.label())
        } else {
            dialog.action("confirm", action.label())
        };
        let answer = dialog.show();
        task(async move {
            let chosen = matches!(
                answer.await,
                Ok(result) if result.action.as_deref() == Some("confirm")
            );
            if chosen {
                perform_action(item, action);
            }
        });
        return;
    }
    perform_action(item, action);
}

fn set_action_error(message: &str) {
    services().update(|state| {
        state.action_error = Some(message.to_string());
    });
}

fn perform_action(item: Item, action: Action) {
    if is_busy(&item.id) {
        return;
    }
    let Some(provider) = provider_of(item.source) else {
        set_action_error("no provider is available for this source");
        return;
    };
    let source = item.source;
    let id = item.id.clone();
    let name = item.name.clone();
    services().update(|state| {
        state.busy.insert(id.clone());
        state.action_error = None;
    });
    let again = item.clone();
    task(async move {
        let answer = match blocking(move || {
            provider
                .do_action(&item, action)
                .map_err(|error| error.to_string())
        })
        .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("the action did not finish: {error}")),
        };
        let mut selection_cleared = false;
        services().update(|state| {
            state.busy.remove(&id);
            match &answer {
                Ok(()) => {
                    state.action_error = None;
                    if action == Action::Remove && state.selected.as_deref() == Some(id.as_str()) {
                        state.selected = None;
                        state.detail = None;
                        selection_cleared = true;
                    }
                }
                Err(error) => {
                    state.action_error = Some(format!("{name}: {error}"));
                }
            }
        });
        if selection_cleared {
            set_hovered_row(None);
            reset_log_tail(None, None);
        }
        if answer.is_ok() {
            crate::toast::success(format!("{} {}", name, action.past()));
        }
        // A process takes a moment to go, and launchd a moment to notice.
        refresh(source);
        if answer.is_ok() && action != Action::Remove {
            if is_selected(&again.id) {
                later(Duration::from_millis(500), move || {
                    if is_selected(&again.id) {
                        load_detail(again.clone());
                    }
                });
            }
            later(Duration::from_millis(900), move || refresh(source));
        }
    });
}

// --- Hover -------------------------------------------------------------------

fn clear_hover() {
    let needs_clear =
        hover_state().with_untracked(|state| state.item.is_some() || state.row.is_some());
    if needs_clear {
        hover_state().update(|state| {
            state.item = None;
            state.row = None;
        });
    }
}

pub(crate) fn hovered_item() -> Option<String> {
    hover_state().with(|state| state.item.clone())
}

pub(crate) fn set_hovered_item(id: Option<String>) {
    if hover_state().with_untracked(|state| state.item != id) {
        hover_state().update(|state| state.item = id);
    }
}

pub(crate) fn hovered_row() -> Option<String> {
    hover_state().with(|state| state.row.clone())
}

pub(crate) fn set_hovered_row(key: Option<String>) {
    if hover_state().with_untracked(|state| state.row != key) {
        hover_state().update(|state| state.row = key);
    }
}

// --- Copy --------------------------------------------------------------------

/// Copies a value and notifies the user with a toast.
pub(crate) fn copy(value: &str) {
    set_clipboard(value.to_string());
    crate::toast::info("Copied");
}

#[cfg(test)]
mod tests {
    use super::*;
    use devcore::helper::{
        BinaryInfo, ClientConfig, DefaultHelper, build_helper_plist, expected_staged_executable,
    };
    use devcore::launchd::PrivilegedHelper;

    #[test]
    fn default_helper_dynamically_tracks_installed_helper_state() {
        let dir = std::env::temp_dir().join(format!(
            "devbase-services-helper-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let helper_dir = dir.join("helper");
        let plist_path = dir.join("xyz.jyeu.devbase.helper.plist");
        let socket_path = dir.join("xyz.jyeu.devbase.helper.sock");
        let expected = BinaryInfo {
            executable: dir.join("Devbase"),
            bundle_path: None,
            owner_uid: 501,
        };
        let staged = expected_staged_executable(&helper_dir, &expected);
        let client = Arc::new(HelperClient::new(ClientConfig {
            socket_path: socket_path.clone(),
            plist_path: plist_path.clone(),
            helper_dir: helper_dir.clone(),
            expected: expected.clone(),
            ..ClientConfig::default()
        }));
        let _guard = HelperClient::set_default_for_test(client);

        // Before the plist and staged binary exist, DefaultHelper reports missing.
        assert!(DefaultHelper.require().is_err());

        // Staging the binary + plist makes DefaultHelper ready immediately with no restart.
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"stub").unwrap();
        let plist = build_helper_plist(&staged, expected.owner_uid, &socket_path.to_string_lossy())
            .unwrap();
        std::fs::write(&plist_path, plist).unwrap();
        assert!(DefaultHelper.require().is_ok());

        // Removing the plist immediately reverts DefaultHelper to not installed.
        std::fs::remove_file(&plist_path).unwrap();
        assert!(DefaultHelper.require().is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
