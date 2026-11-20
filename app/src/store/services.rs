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
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use devcore::launchd::{
    Context, Launchd, PrivilegedHelper, SystemHost, SystemRunner, UnavailableHelper,
};
use nsh_base::alert_dialog;
use nsh_core::prelude::*;

use crate::logs;

pub(crate) use devcore::service::{Action, Item, LogFile, Provider, Row, Source};

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

/// The sources the inspector shows this phase, in order.
pub(crate) const SOURCES: [Source; 3] = [Source::UserAgent, Source::SystemAgent, Source::Daemon];

/// One source's last read.
#[derive(Debug, Clone, Default)]
struct SourceLoad {
    items: Vec<Item>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
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
    log_file: usize,
    log_lines: Option<Vec<String>>,
    log_error: Option<String>,
    log_truncated: bool,
    log_reading: bool,
    log_sequence: u64,
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
            log_file: 0,
            log_lines: None,
            log_error: None,
            log_truncated: false,
            log_reading: false,
            log_sequence: 0,
            sequence: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct Services {
    active: Source,
    query: String,
    filter: Filter,
    loads: BTreeMap<Source, SourceLoad>,
    selected: Option<String>,
    hovered_item: Option<String>,
    hovered_row: Option<String>,
    detail: Option<DetailState>,
    busy: BTreeSet<String>,
    action_error: Option<String>,
    notice: Option<String>,
}

impl Default for Services {
    fn default() -> Self {
        Services {
            active: Source::UserAgent,
            query: String::new(),
            filter: Filter::All,
            loads: BTreeMap::new(),
            selected: None,
            hovered_item: None,
            hovered_row: None,
            detail: None,
            busy: BTreeSet::new(),
            action_error: None,
            notice: None,
        }
    }
}

fn services() -> Signal<Services> {
    global::<Services>()
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

/// Test providers, installed before the surface starts. The store never
/// reaches launchd while a source has one.
static FAKES: LazyLock<Mutex<BTreeMap<Source, Arc<dyn Provider>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static FAKE_MODE: AtomicBool = AtomicBool::new(false);

fn fakes() -> &'static Mutex<BTreeMap<Source, Arc<dyn Provider>>> {
    &FAKES
}

/// The machine's three launchd providers, made once.
static LAUNCHD: LazyLock<Launchd> = LazyLock::new(|| {
    let context = Arc::new(Context::new(
        Arc::new(SystemRunner::new()),
        Arc::new(UnavailableHelper) as Arc<dyn PrivilegedHelper>,
        Arc::new(SystemHost::new()),
    ));
    Launchd::new(context)
});

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
    LAUNCHD
        .provider(source)
        .map(|provider| Arc::clone(provider) as Arc<dyn Provider>)
}

/// Makes the surface use test providers instead of launchd. Call it before
/// starting the headless surface, then [`install_provider`] for each source.
/// Tests only.
pub fn use_fake_providers() {
    FAKE_MODE.store(true, Ordering::Relaxed);
    fakes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// Gives a source a provider of the test's own. Tests only.
pub fn install_provider(source: Source, provider: Arc<dyn Provider>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    fakes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(source, provider);
}

/// The provider of a source, for its pure [`Provider::actions`] and
/// [`Provider::confirm`] (no work happens in them).
pub(crate) fn provider(source: Source) -> Option<Arc<dyn Provider>> {
    provider_of(source)
}

// --- Source and list ---------------------------------------------------------

/// The source the list shows.
pub(crate) fn active_source() -> Source {
    services().with(|state| state.active)
}

/// Shows `source`, clearing the item chosen and reading it when it has not
/// been read yet.
pub(crate) fn select_source(source: Source) {
    services().update(|state| {
        state.active = source;
        state.selected = None;
        state.hovered_item = None;
        state.detail = None;
        state.action_error = None;
        state.notice = None;
    });
    let load = services().with(|state| state.loads.get(&source).cloned());
    if load.is_none_or(|load| !load.loaded && !load.loading) {
        refresh(source);
    }
}

/// Reads the shown source the first time the screen appears.
pub(crate) fn load_active() {
    let source = active_source();
    let load = services().with(|state| state.loads.get(&source).cloned());
    if load.is_none_or(|load| !load.loaded && !load.loading) {
        refresh(source);
    }
}

/// Reads `source` again.
pub(crate) fn refresh(source: Source) {
    let Some(provider) = provider_of(source) else {
        services().update(|state| {
            let load = state.loads.entry(source).or_default();
            load.loading = false;
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
        let answer = match blocking(move || provider.list().map_err(|error| error.to_string())).await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("the source did not answer: {error}")),
        };
        services().update(|state| {
            let load = state.loads.entry(source).or_default();
            if load.sequence != sequence {
                return;
            }
            load.loading = false;
            load.loaded = true;
            match answer {
                Ok(items) => {
                    load.items = items;
                    load.error = None;
                }
                Err(error) => load.error = Some(error),
            }
            if let Some(selected) = &state.selected
                && !load.items.iter().any(|item| &item.id == selected)
            {
                state.selected = None;
                state.detail = None;
            }
        });
    });
}

/// Reads the shown source again, on request.
pub(crate) fn refresh_active() {
    refresh(active_source());
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

/// The items the list shows for the active source: the search and filter let
/// them through.
pub(crate) fn visible_items() -> Vec<Item> {
    services().with(|state| {
        let Some(load) = state.loads.get(&state.active) else {
            return Vec::new();
        };
        let query = &state.query;
        let filter = state.filter;
        load.items
            .iter()
            .filter(|item| item.matches(query) && filter.passes(item))
            .cloned()
            .collect()
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

pub(crate) fn active_loading() -> bool {
    services().with(|state| {
        state
            .loads
            .get(&state.active)
            .is_some_and(|load| load.loading)
    })
}

pub(crate) fn active_error() -> Option<String> {
    services().with(|state| {
        state
            .loads
            .get(&state.active)
            .and_then(|load| load.error.clone())
    })
}

// --- Selection and detail ----------------------------------------------------

fn item_by_id(id: &str) -> Option<Item> {
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

pub(crate) fn selected_item() -> Option<Item> {
    services().with(|state| {
        let selected = state.selected.as_deref()?;
        state
            .loads
            .values()
            .flat_map(|load| load.items.iter())
            .find(|item| item.id == selected)
            .cloned()
    })
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
        state.notice = None;
    });
    load_detail(item);
}

/// Closes the inspector.
pub(crate) fn clear_selection() {
    services().update(|state| {
        state.selected = None;
        state.detail = None;
        state.hovered_row = None;
    });
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
    task(async move {
        let answer = match blocking(move || provider.detail(&item).map_err(|error| error.to_string()))
            .await
        {
            Ok(answer) => answer,
            Err(error) => Err(format!("the detail read failed: {error}")),
        };
        services().update(|state| {
            let Some(detail) = state.detail.as_mut() else {
                return;
            };
            if detail.item_id != id || detail.sequence != sequence {
                return;
            }
            detail.loading = false;
            match answer {
                Ok(read) => {
                    detail.rows = read.rows;
                    detail.logs = read.logs;
                    detail.log_lines = None;
                    detail.log_error = None;
                }
                Err(error) => detail.error = Some(error),
            }
        });
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

/// The item chosen and its detail, for the inspector.
pub(crate) fn detail() -> Option<(String, Vec<Row>, Vec<LogFile>, bool, Option<String>)> {
    services().with(|state| {
        let detail = state.detail.as_ref()?;
        Some((
            detail.item_id.clone(),
            detail.rows.clone(),
            detail.logs.clone(),
            detail.loading,
            detail.error.clone(),
        ))
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
    services().with(|state| {
        state
            .detail
            .as_ref()
            .map_or(0, |detail| detail.log_file)
    })
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

/// Chooses which log file the log tab shows, and reads it.
pub(crate) fn set_log_file(index: usize) {
    services().update(|state| {
        if let Some(detail) = state.detail.as_mut() {
            if detail.log_file == index {
                return;
            }
            detail.log_file = index;
            detail.log_lines = None;
            detail.log_error = None;
            detail.log_truncated = false;
        }
    });
    load_log();
}

/// The log tab's current content: the file, lines, error and whether the
/// file holds more before them.
pub(crate) fn log_view() -> Option<(LogFile, Option<Vec<String>>, Option<String>, bool)> {
    services().with(|state| {
        let detail = state.detail.as_ref()?;
        let file = detail.logs.get(detail.log_file)?.clone();
        Some((
            file,
            detail.log_lines.clone(),
            detail.log_error.clone(),
            detail.log_truncated,
        ))
    })
}

/// Reads the shown log's end. Called when the log tab opens and once a
/// second while it is open.
pub(crate) fn load_log() {
    let Some((id, path, sequence)) = services().with(|state| {
        let detail = state.detail.as_ref()?;
        if detail.tab != DetailTab::Logs || detail.log_reading {
            return None;
        }
        let file = detail.logs.get(detail.log_file)?;
        Some((detail.item_id.clone(), file.path.clone(), detail.log_sequence))
    }) else {
        return;
    };
    let next = services().with(|state| {
        let Some(detail) = state.detail.as_ref() else {
            return 0;
        };
        if detail.item_id != id || detail.log_sequence != sequence || detail.log_reading {
            return 0;
        }
        detail.log_sequence + 1
    });
    if next == 0 {
        return;
    }
    services().update(|state| {
        if let Some(detail) = state.detail.as_mut()
            && detail.item_id == id
            && detail.log_sequence + 1 == next
        {
            detail.log_sequence = next;
            detail.log_reading = true;
            detail.log_error = None;
        }
    });
    let sequence = next;
    task(async move {
        let answer = match blocking(move || logs::tail(&path)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("the log read failed: {error}")),
        };
        services().update(|state| {
            let Some(detail) = state.detail.as_mut() else {
                return;
            };
            if detail.item_id != id || detail.log_sequence != sequence {
                return;
            }
            detail.log_reading = false;
            match answer {
                Ok(tail) => {
                    detail.log_lines = Some(tail.lines);
                    detail.log_truncated = tail.truncated;
                }
                Err(error) => detail.log_error = Some(error),
            }
        });
    });
}

/// Keeps the log tab following the file while it is open.
pub(crate) fn tick_log() {
    let active = services().with(|state| {
        state.detail.as_ref().is_some_and(|detail| {
            detail.tab == DetailTab::Logs && !detail.logs.is_empty() && !detail.log_reading
        })
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

pub(crate) fn notice() -> Option<String> {
    services().with(|state| state.notice.clone())
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
        state.notice = None;
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
        state.notice = None;
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
        services().update(|state| {
            state.busy.remove(&id);
            match &answer {
                Ok(()) => {
                    state.action_error = None;
                    state.notice = Some(format!("{} {}", name, action.past()));
                    if action == Action::Remove && state.selected.as_deref() == Some(id.as_str()) {
                        state.selected = None;
                        state.detail = None;
                    }
                }
                Err(error) => {
                    state.action_error = Some(format!("{name}: {error}"));
                    state.notice = None;
                }
            }
        });
        let text = format!("{} {}", name, action.past());
        later(Duration::from_secs(3), move || {
            services().update(|state| {
                if state.notice.as_deref() == Some(text.as_str()) {
                    state.notice = None;
                }
            });
        });
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

pub(crate) fn hovered_item() -> Option<String> {
    services().with(|state| state.hovered_item.clone())
}

pub(crate) fn set_hovered_item(id: Option<String>) {
    services().update(|state| state.hovered_item = id);
}

pub(crate) fn hovered_row() -> Option<String> {
    services().with(|state| state.hovered_row.clone())
}

pub(crate) fn set_hovered_row(key: Option<String>) {
    services().update(|state| state.hovered_row = key);
}

// --- Copy --------------------------------------------------------------------

/// Copies a value and says so in the inspector for a moment.
pub(crate) fn copy(value: &str) {
    set_clipboard(value.to_string());
    let message = "Copied";
    services().update(|state| state.notice = Some(message.to_string()));
    later(Duration::from_secs(2), || {
        services().update(|state| {
            if state.notice.as_deref() == Some(message) {
                state.notice = None;
            }
        });
    });
}
