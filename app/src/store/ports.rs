//! Ports feature store: search, category/starred filtering, favorite & watched
//! port persistence, watched-port change notifications, low-frequency auto-refresh,
//! smart Devbase vs. external process termination, and HTTPS rule handoff.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use devcore::databases::{ListeningPort, PortCategory, PortOwner, SignalOutcome};
use nsh_core::prelude::*;

use crate::store::{self, Section, databases, proxy, system};

/// Auto-refresh cadence when active on the Ports page or when ports are watched.
const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// Filter chips on the Ports screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PortFilter {
    #[default]
    All,
    Dev,
    Database,
    Web,
    System,
    Starred,
}

impl PortFilter {
    pub(crate) const ALL: [PortFilter; 6] = [
        PortFilter::All,
        PortFilter::Dev,
        PortFilter::Database,
        PortFilter::Web,
        PortFilter::System,
        PortFilter::Starred,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            PortFilter::All => "All",
            PortFilter::Dev => "Dev",
            PortFilter::Database => "Database",
            PortFilter::Web => "Web",
            PortFilter::System => "System",
            PortFilter::Starred => "Starred",
        }
    }

    pub(crate) fn test_id(self) -> &'static str {
        match self {
            PortFilter::All => "ports-filter-all",
            PortFilter::Dev => "ports-filter-dev",
            PortFilter::Database => "ports-filter-database",
            PortFilter::Web => "ports-filter-web",
            PortFilter::System => "ports-filter-system",
            PortFilter::Starred => "ports-filter-starred",
        }
    }

    pub(crate) fn matches(self, entry: &ListeningPort) -> bool {
        match self {
            PortFilter::All => true,
            PortFilter::Dev => entry.category == PortCategory::Dev,
            PortFilter::Database => entry.category == PortCategory::Database,
            PortFilter::Web => entry.category == PortCategory::Web,
            PortFilter::System => entry.category == PortCategory::System,
            PortFilter::Starred => entry.starred,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchedObservation {
    pid: Option<i64>,
    process: String,
}

#[derive(Debug, Clone, Default)]
struct PortsUiState {
    search: String,
    filter: PortFilter,
    auto_refresh: bool,
    poller_running: bool,
    favorite_ports: BTreeSet<i64>,
    watched_ports: BTreeSet<i64>,
    stopping_ports: BTreeSet<i64>,
    /// `(port, pid)` promoted to Force Kill (SIGKILL) after SIGTERM left the process alive.
    force_kill_candidate: Option<(i64, i64)>,
    /// Previous listening observation for each watched port (to detect transitions).
    watched_baseline: BTreeMap<i64, Option<WatchedObservation>>,
    /// Recent human-readable watched-port transition messages (for tests & UI inspection).
    watched_events: Vec<String>,
    /// Last action error surfaced in the Ports page header.
    action_error: Option<String>,
}

fn ui_state() -> Signal<PortsUiState> {
    global::<PortsUiState>()
}

fn run_task(future: impl Future<Output = ()> + 'static) {
    Owner::detached().run(move || {
        spawn(future);
    });
}

fn later(delay: Duration, f: impl FnOnce() + 'static) {
    Owner::detached().run(move || {
        set_timeout(delay, f);
    });
}

/// Ensures ports have been loaded at least once.
pub(crate) fn load() {
    if !system::ports_loaded() && !system::ports_loading() {
        system::refresh_ports();
    }
}

/// Refreshes the listening port inventory immediately.
pub(crate) fn refresh() {
    ui_state().update(|state| {
        state.action_error = None;
    });
    system::refresh_ports();
}

pub(crate) fn search() -> String {
    ui_state().with(|state| state.search.clone())
}

pub(crate) fn set_search(next: String) {
    ui_state().update(|state| state.search = next);
}

pub(crate) fn filter() -> PortFilter {
    ui_state().with(|state| state.filter)
}

pub(crate) fn set_filter(next: PortFilter) {
    ui_state().update(|state| state.filter = next);
}

pub(crate) fn auto_refresh_enabled() -> bool {
    ui_state().with(|state| state.auto_refresh)
}

pub(crate) fn toggle_auto_refresh() {
    let enabled = ui_state().with(|state| !state.auto_refresh);
    ui_state().update(|state| state.auto_refresh = enabled);
    if enabled {
        ensure_poller();
    }
}

pub(crate) fn is_stopping(port: i64) -> bool {
    ui_state().with(|state| state.stopping_ports.contains(&port))
}

pub(crate) fn force_kill_pid_for(port: i64) -> Option<i64> {
    ui_state().with(|state| match state.force_kill_candidate {
        Some((p, pid)) if p == port => Some(pid),
        _ => None,
    })
}

pub(crate) fn action_error() -> Option<String> {
    ui_state().with(|state| state.action_error.clone())
}

#[allow(dead_code)]
pub(crate) fn watched_events() -> Vec<String> {
    ui_state().with(|state| state.watched_events.clone())
}

/// Returns current provider's `(current_pid, current_uid)` for safe-kill gating.
pub(crate) fn current_identity() -> (i64, i64) {
    if let Some(provider) = system::ports_provider_of() {
        (provider.current_pid(), provider.current_uid())
    } else {
        let sig = devcore::databases::SystemProcessSignaler;
        (
            devcore::databases::ProcessSignaler::current_pid(&sig),
            devcore::databases::ProcessSignaler::current_uid(&sig),
        )
    }
}

/// Active listening ports with live favorite/watched flags merged in, pinned
/// starred-first then sorted ascending by port number.
pub(crate) fn all_ports() -> Vec<ListeningPort> {
    let favs = ui_state().with(|s| s.favorite_ports.clone());
    let watched = ui_state().with(|s| s.watched_ports.clone());
    let mut ports = system::listening_ports();
    for entry in &mut ports {
        if favs.contains(&entry.port) {
            entry.starred = true;
        }
        if watched.contains(&entry.port) {
            entry.watched = true;
        }
    }
    ports.sort_by(|a, b| b.starred.cmp(&a.starred).then(a.port.cmp(&b.port)));
    ports
}

/// Ports matching the current search query and category/starred filter.
pub(crate) fn filtered_ports() -> Vec<ListeningPort> {
    let query = search().trim().to_ascii_lowercase();
    let active_filter = filter();
    all_ports()
        .into_iter()
        .filter(|entry| active_filter.matches(entry))
        .filter(|entry| matches_query(entry, &query))
        .collect()
}

/// Saved (starred or watched) ports that are currently not listening.
pub(crate) fn offline_saved_ports() -> Vec<(i64, bool, bool)> {
    let active: BTreeSet<i64> = system::listening_ports()
        .into_iter()
        .map(|entry| entry.port)
        .collect();
    ui_state().with(|state| {
        let mut union = BTreeSet::new();
        union.extend(state.favorite_ports.iter().copied());
        union.extend(state.watched_ports.iter().copied());
        union
            .into_iter()
            .filter(|port| !active.contains(port))
            .map(|port| {
                (
                    port,
                    state.favorite_ports.contains(&port),
                    state.watched_ports.contains(&port),
                )
            })
            .collect()
    })
}

/// Count of active ports matching `filter` (for filter tab badges).
pub(crate) fn count_for_filter(filter: PortFilter) -> usize {
    all_ports()
        .into_iter()
        .filter(|entry| filter.matches(entry))
        .count()
}

fn matches_query(entry: &ListeningPort, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let port_str = entry.port.to_string();
    if port_str.contains(query) || format!(":{port_str}").contains(query) {
        return true;
    }
    if entry.process.to_ascii_lowercase().contains(query)
        || entry.command.to_ascii_lowercase().contains(query)
        || entry.address.to_ascii_lowercase().contains(query)
        || entry.category.as_str().contains(query)
    {
        return true;
    }
    if let Some(pid) = entry.pid
        && pid.to_string().contains(query)
    {
        return true;
    }
    match &entry.owner {
        PortOwner::DevbaseDatabase { id, engine } => {
            id.to_ascii_lowercase().contains(query) || engine.to_ascii_lowercase().contains(query)
        }
        PortOwner::DevbaseProxy => "devbase proxy https".contains(query),
        PortOwner::DevbaseRule { domain } => domain.to_ascii_lowercase().contains(query),
        PortOwner::External => false,
    }
}

/// Called by `store::system::refresh_ports` whenever a fresh snapshot arrives.
/// Merges persisted favorites/watched lists and detects watched-port transitions.
pub(crate) fn on_ports_refreshed(ports: &[ListeningPort], favorites: Vec<i64>, watched: Vec<i64>) {
    let mut fav_set: BTreeSet<i64> = favorites.into_iter().collect();
    let mut watch_set: BTreeSet<i64> = watched.into_iter().collect();
    for entry in ports {
        if entry.starred {
            fav_set.insert(entry.port);
        }
        if entry.watched {
            watch_set.insert(entry.port);
        }
    }

    let current_by_port: BTreeMap<i64, WatchedObservation> = ports
        .iter()
        .map(|entry| {
            (
                entry.port,
                WatchedObservation {
                    pid: entry.pid,
                    process: entry.process.clone(),
                },
            )
        })
        .collect();

    let mut notifications: Vec<(i64, String, String)> = Vec::new();

    ui_state().update(|state| {
        if !fav_set.is_empty() || !state.favorite_ports.is_empty() {
            state.favorite_ports = fav_set;
        }
        if !watch_set.is_empty() || !state.watched_ports.is_empty() {
            state.watched_ports = watch_set.clone();
        }

        // Clear stale force-kill candidate if that port/PID is gone.
        if let Some((cand_port, cand_pid)) = state.force_kill_candidate {
            let still_same = current_by_port
                .get(&cand_port)
                .is_some_and(|obs| obs.pid == Some(cand_pid));
            if !still_same {
                state.force_kill_candidate = None;
            }
        }

        // Prune baselines for ports that are no longer watched.
        state
            .watched_baseline
            .retain(|port, _| state.watched_ports.contains(port));

        for &port in &state.watched_ports {
            let now = current_by_port.get(&port).cloned();
            if let Some(prev) = state.watched_baseline.get(&port)
                && prev != &now
            {
                let (title, body) = describe_watch_transition(port, prev.as_ref(), now.as_ref());
                notifications.push((port, title, body));
            }
            state.watched_baseline.insert(port, now);
        }

        for (_, _, body) in &notifications {
            state.watched_events.push(body.clone());
            if state.watched_events.len() > 32 {
                let drop = state.watched_events.len() - 32;
                state.watched_events.drain(..drop);
            }
        }
    });

    for (port, title, body) in notifications {
        crate::toast::info(body.clone());
        crate::notify_task(
            &format!("port-watch-{port}"),
            &title,
            &body,
            Section::Ports,
            false,
        );
    }

    if !ui_state().with(|s| s.watched_ports.is_empty()) {
        ensure_poller();
    }
}

fn describe_watch_transition(
    port: i64,
    prev: Option<&WatchedObservation>,
    now: Option<&WatchedObservation>,
) -> (String, String) {
    match (prev, now) {
        (None, Some(after)) => {
            let proc_name = if after.process.is_empty() {
                "process"
            } else {
                &after.process
            };
            let pid_suffix = after.pid.map(|p| format!(" (PID {p})")).unwrap_or_default();
            (
                format!("Port {port} opened"),
                format!("Port {port} is now listening via {proc_name}{pid_suffix}."),
            )
        }
        (Some(before), None) => {
            let proc_name = if before.process.is_empty() {
                "previous listener"
            } else {
                &before.process
            };
            (
                format!("Port {port} closed"),
                format!("Port {port} is no longer listening ({proc_name} exited)."),
            )
        }
        (Some(before), Some(after)) => {
            let from = if before.process.is_empty() {
                "unknown"
            } else {
                &before.process
            };
            let to = if after.process.is_empty() {
                "unknown"
            } else {
                &after.process
            };
            let pid_suffix = after.pid.map(|p| format!(" (PID {p})")).unwrap_or_default();
            (
                format!("Port {port} changed"),
                format!("Port {port} switched from {from} to {to}{pid_suffix}."),
            )
        }
        (None, None) => (
            format!("Port {port} updated"),
            format!("Port {port} state changed."),
        ),
    }
}

/// Starts a single lightweight 5-second refresh timer while auto-refresh is on
/// (and viewing Ports) OR while any watched ports exist. Stops scheduling when
/// neither condition holds so idle CPU/battery usage is zero.
fn ensure_poller() {
    if system::is_fake_mode() {
        return;
    }
    let already_running = ui_state().with(|s| s.poller_running);
    if already_running {
        return;
    }
    ui_state().update(|s| s.poller_running = true);
    schedule_poll_tick();
}

fn schedule_poll_tick() {
    later(AUTO_REFRESH_INTERVAL, || {
        let (auto, has_watched) =
            ui_state().with(|s| (s.auto_refresh, !s.watched_ports.is_empty()));
        let on_ports_page = store::active_section() == Section::Ports;
        if !(has_watched || (auto && on_ports_page)) {
            ui_state().update(|s| s.poller_running = false);
            return;
        }
        if !system::ports_loading() {
            system::refresh_ports();
        }
        schedule_poll_tick();
    });
}

/// Toggles starred state for `port` and persists it via `PortsProvider`.
pub(crate) fn toggle_starred(port: i64) {
    let Some(provider) = system::ports_provider_of() else {
        return;
    };
    run_task(async move {
        let answer = match blocking(move || provider.toggle_starred(port)).await {
            Ok(res) => res,
            Err(error) => Err(format!("could not update starred port: {error}")),
        };
        match answer {
            Ok((starred, list)) => {
                ui_state().update(|state| {
                    state.favorite_ports = list.into_iter().collect();
                    state.action_error = None;
                });
                system::update_port_flags(port, Some(starred), None);
            }
            Err(error) => {
                ui_state().update(|state| state.action_error = Some(error.clone()));
                crate::toast::error(error);
            }
        }
    });
}

/// Toggles watched state for `port` and persists it via `PortsProvider`.
pub(crate) fn toggle_watched(port: i64) {
    let Some(provider) = system::ports_provider_of() else {
        return;
    };
    run_task(async move {
        let answer = match blocking(move || provider.toggle_watched(port)).await {
            Ok(res) => res,
            Err(error) => Err(format!("could not update watched port: {error}")),
        };
        match answer {
            Ok((watched, list)) => {
                let current_obs = system::listening_ports()
                    .into_iter()
                    .find(|entry| entry.port == port)
                    .map(|entry| WatchedObservation {
                        pid: entry.pid,
                        process: entry.process,
                    });
                ui_state().update(|state| {
                    state.watched_ports = list.into_iter().collect();
                    if watched {
                        state.watched_baseline.insert(port, current_obs);
                    } else {
                        state.watched_baseline.remove(&port);
                    }
                    state.action_error = None;
                });
                system::update_port_flags(port, None, Some(watched));
                if watched {
                    crate::toast::info(format!(
                        "Watching port {port} for open, close, or process changes."
                    ));
                    ensure_poller();
                } else {
                    crate::toast::info(format!("Stopped watching port {port}."));
                }
            }
            Err(error) => {
                ui_state().update(|state| state.action_error = Some(error.clone()));
                crate::toast::error(error);
            }
        }
    });
}

/// Smart stop/kill for a port row:
/// * Devbase database → stops the instance cleanly through the database engine.
/// * Devbase HTTPS proxy → stops the proxy job cleanly.
/// * External user-owned PID → sends single-PID `SIGTERM` (or `SIGKILL` when `force == true`).
pub(crate) fn stop_port(entry: ListeningPort, force: bool) {
    let port = entry.port;
    match entry.owner {
        PortOwner::DevbaseDatabase { id, .. } => {
            databases::stop_instance(&id);
            crate::toast::info(format!("Stopping database {id} on port {port}…"));
            if system::is_fake_mode() {
                system::refresh_ports();
            } else {
                later(Duration::from_millis(600), system::refresh_ports);
            }
            return;
        }
        PortOwner::DevbaseProxy => {
            proxy::set_proxy_enabled(false);
            crate::toast::info(format!("Stopping HTTPS proxy on port {port}…"));
            if system::is_fake_mode() {
                system::refresh_ports();
            } else {
                later(Duration::from_millis(600), system::refresh_ports);
            }
            return;
        }
        PortOwner::DevbaseRule { .. } | PortOwner::External => {}
    }

    let Some(pid) = entry.pid else {
        let msg = format!(
            "Cannot stop port {port}: process PID is unavailable (use Services for system daemons)."
        );
        ui_state().update(|s| s.action_error = Some(msg.clone()));
        crate::toast::error(msg);
        return;
    };

    let Some(provider) = system::ports_provider_of() else {
        return;
    };

    ui_state().update(|state| {
        state.stopping_ports.insert(port);
        state.action_error = None;
    });

    let process_label = if entry.process.is_empty() {
        format!("PID {pid}")
    } else {
        format!("{} (PID {pid})", entry.process)
    };

    run_task(async move {
        let answer = match blocking(move || provider.terminate_pid(pid, force)).await {
            Ok(res) => res,
            Err(error) => Err(format!("could not stop {process_label}: {error}")),
        };
        ui_state().update(|state| {
            state.stopping_ports.remove(&port);
            match &answer {
                Ok(SignalOutcome::Stopped | SignalOutcome::AlreadyExited) => {
                    if state.force_kill_candidate == Some((port, pid)) {
                        state.force_kill_candidate = None;
                    }
                    state.action_error = None;
                }
                Ok(SignalOutcome::StillRunning) => {
                    state.force_kill_candidate = Some((port, pid));
                    state.action_error = None;
                }
                Err(error) => {
                    state.action_error = Some(error.clone());
                }
            }
        });
        match answer {
            Ok(SignalOutcome::Stopped) => {
                let sig = if force { "SIGKILL" } else { "SIGTERM" };
                crate::toast::success(format!("Stopped {process_label} on port {port} ({sig})."));
                system::refresh_ports();
            }
            Ok(SignalOutcome::AlreadyExited) => {
                crate::toast::info(format!("{process_label} had already exited."));
                system::refresh_ports();
            }
            Ok(SignalOutcome::StillRunning) => {
                crate::toast::info(format!(
                    "{process_label} did not exit after SIGTERM — click Force kill to send SIGKILL."
                ));
            }
            Err(error) => {
                crate::toast::error(error);
            }
        }
    });
}

/// Stops an external user-owned process blocking a database port and invokes
/// `on_stopped` so the caller can immediately re-check the database port.
pub(crate) fn stop_conflict_pid(
    port: i64,
    pid: i64,
    name: String,
    on_stopped: impl FnOnce() + 'static,
) {
    let Some(provider) = system::ports_provider_of() else {
        return;
    };
    let label = if name.is_empty() {
        format!("PID {pid}")
    } else {
        format!("{name} (PID {pid})")
    };
    run_task(async move {
        let answer = match blocking(move || provider.terminate_pid(pid, false)).await {
            Ok(res) => res,
            Err(error) => Err(format!("could not stop {label}: {error}")),
        };
        match answer {
            Ok(SignalOutcome::Stopped | SignalOutcome::AlreadyExited) => {
                crate::toast::success(format!("Stopped {label} on port {port}."));
                system::refresh_ports();
                on_stopped();
            }
            Ok(SignalOutcome::StillRunning) => {
                crate::toast::info(format!(
                    "{label} did not exit after SIGTERM — open Ports to force kill it."
                ));
            }
            Err(error) => {
                crate::toast::error(error);
            }
        }
    });
}

/// Navigates to HTTPS Proxy with a pre-filled draft rule for `port`.
pub(crate) fn expose_via_https(port: i64, process: &str) {
    proxy::open_rule_draft_for_port(process, port);
}

/// Formats the browser URL for a listening port (local HTTPS domain if proxied,
/// otherwise `http://127.0.0.1:<port>`).
pub(crate) fn display_url(entry: &ListeningPort) -> String {
    match &entry.owner {
        PortOwner::DevbaseRule { domain } => {
            let listener_port = proxy::listener_port();
            if listener_port == 443 || listener_port == 0 {
                format!("https://{domain}")
            } else {
                format!("https://{domain}:{listener_port}")
            }
        }
        _ => format!("http://127.0.0.1:{}", entry.port),
    }
}

/// Opens the port's local URL in the default browser.
pub(crate) fn open_port_url(entry: &ListeningPort) {
    let url = display_url(entry);
    crate::menubar::open_url_from_app(url);
}

/// Copies the port's local URL to the clipboard.
pub(crate) fn copy_port_url(entry: &ListeningPort) {
    let url = display_url(entry);
    set_clipboard(url.clone());
    crate::toast::success(format!("Copied {url}"));
}

/// Navigates to the Ports screen filtered to a specific port number.
pub(crate) fn open_ports_for(port: i64) {
    set_filter(PortFilter::All);
    set_search(port.to_string());
    store::select_section(Section::Ports);
    load();
}
