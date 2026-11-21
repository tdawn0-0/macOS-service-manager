//! State and off-thread log tailing for the workspace live log drawer, ported
//! from `view_dev_logs.go`.
//!
//! All filesystem and process work (reading `state.json`, tailing log files via
//! [`crate::logs::tail`], and opening files in macOS Console.app) runs on
//! blocking background threads via [`Owner::detached`] + [`spawn`] +
//! [`blocking`]. Fine-grained signals keep the log viewport's `scroll()`
//! container and `each(...)` rows stable across 1-second live polls.

use std::{
    cell::RefCell,
    io::ErrorKind,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use devcore::state::Store;
use nsh_core::prelude::*;

use crate::logs;

/// The default target when none has been selected yet.
pub const DEFAULT_JOB_ID: &str = "proxy";

/// The error message displayed when a requested job ID fails validation.
pub const INVALID_TARGET_ERROR: &str = "Invalid log target";

/// Which log file of a workspace job to tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Stream {
    /// Standard error (`<job_id>.err.log`), matching Go's default stream `0`.
    #[default]
    Stderr,
    /// Standard output (`<job_id>.log`), matching Go's stream `1`.
    Stdout,
}

impl Stream {
    /// Both log streams in display order.
    pub const ALL: [Stream; 2] = [Stream::Stderr, Stream::Stdout];

    /// Short label shown in the drawer's stream switcher.
    pub fn label(self) -> &'static str {
        match self {
            Stream::Stderr => "Error",
            Stream::Stdout => "Output",
        }
    }

    /// File suffix appended to a validated job ID.
    pub fn suffix(self) -> &'static str {
        match self {
            Stream::Stderr => ".err.log",
            Stream::Stdout => ".log",
        }
    }

    /// Log file name for a validated `job_id`.
    pub fn file_name(self, job_id: &str) -> String {
        format!("{job_id}{}", self.suffix())
    }
}

/// One selectable workspace log target.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    pub job_id: String,
    pub title: String,
}

impl Target {
    pub fn new(job_id: impl Into<String>) -> Self {
        let job_id = job_id.into();
        let title = target_title(&job_id);
        Self { job_id, title }
    }
}

/// One visible log line with a stable row index for `each(...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilteredLine {
    pub index: usize,
    pub text: String,
}

/// Metadata from the previous successful read, used to skip redundant reads
/// when a log file's size and modification time have not changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownFile {
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// Tail snapshot returned by [`DevLogsProvider::read_log`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogSnapshot {
    pub lines: Vec<String>,
    pub truncated: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// Outcome of polling a workspace job's log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogRead {
    /// File size and modification time match [`KnownFile`]; no re-read needed.
    Unchanged,
    /// The log file does not exist yet.
    Missing,
    /// A fresh tail snapshot was read.
    Snapshot(LogSnapshot),
}

/// Status shown in the log viewport when no lines are displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmptyState {
    Error(String),
    Reading,
    Missing,
    Empty,
    NoMatches,
}

/// Seams for reading workspace log targets, tailing log files, and opening a
/// log file in Console.app. Headless tests install a fake implementation with
/// [`use_fake_provider`] + [`install_provider`] and never touch host disk.
pub trait DevLogsProvider: Send + Sync {
    /// Workspace root whose `logs/` subdirectory stores job log files.
    fn logs_root(&self) -> PathBuf;

    /// Sorted managed database instance IDs (without the `db-` prefix).
    fn database_ids(&self) -> Result<Vec<String>, String>;

    /// Reads the tail of the log file for `job_id` and `stream`.
    /// Runs on a blocking thread; never called on the UI thread.
    fn read_log(
        &self,
        job_id: &str,
        stream: Stream,
        known: Option<KnownFile>,
    ) -> Result<LogRead, String>;

    /// Opens `path` in macOS Console.app. Runs on a blocking thread.
    fn open_in_console(&self, path: &Path) -> Result<(), String>;
}

/// Production [`DevLogsProvider`] backed by `devcore::state::Store`,
/// [`crate::logs::tail`], and `/usr/bin/open -a Console`.
pub struct SystemDevLogsProvider {
    root: PathBuf,
}

impl SystemDevLogsProvider {
    /// Builds a provider rooted at [`devcore::state::default_root`].
    pub fn new() -> Self {
        Self::with_root(devcore::state::default_root())
    }

    /// Builds a provider rooted at an explicit development home directory.
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    fn log_path(&self, job_id: &str, stream: Stream) -> Result<PathBuf, String> {
        if !is_valid_job_id(job_id) {
            return Err(INVALID_TARGET_ERROR.to_string());
        }
        Ok(self.root.join("logs").join(stream.file_name(job_id)))
    }
}

impl Default for SystemDevLogsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl DevLogsProvider for SystemDevLogsProvider {
    fn logs_root(&self) -> PathBuf {
        self.root.clone()
    }

    fn database_ids(&self) -> Result<Vec<String>, String> {
        let state = Store::new(self.root.clone())
            .read()
            .map_err(|error| error.to_string())?;
        Ok(state.instances.keys().cloned().collect())
    }

    fn read_log(
        &self,
        job_id: &str,
        stream: Stream,
        known: Option<KnownFile>,
    ) -> Result<LogRead, String> {
        let path = self.log_path(job_id, stream)?;
        match std::fs::metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(LogRead::Missing),
            Err(error) => return Err(error.to_string()),
            Ok(meta) => {
                if let Some(prev) = known {
                    let modified = meta.modified().ok();
                    if prev.modified.is_some()
                        && meta.len() == prev.size
                        && modified == prev.modified
                    {
                        return Ok(LogRead::Unchanged);
                    }
                }
            }
        }
        match logs::tail(&path) {
            Ok(tail) => Ok(LogRead::Snapshot(LogSnapshot {
                lines: tail.lines,
                truncated: tail.truncated,
                size: tail.size,
                modified: tail.modified,
            })),
            Err(error) => {
                if !path.exists() {
                    Ok(LogRead::Missing)
                } else {
                    Err(error)
                }
            }
        }
    }

    fn open_in_console(&self, path: &Path) -> Result<(), String> {
        let status = std::process::Command::new("open")
            .arg("-a")
            .arg("Console")
            .arg(path)
            .status()
            .map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("open -a Console exited with {status}"))
        }
    }
}

// --- Validation & formatting -------------------------------------------------

/// Validates a workspace log job ID against `^[a-zA-Z0-9][a-zA-Z0-9._-]{0,239}$`,
/// matching Go's `devLogIDRE` and preventing path traversal or separator injection.
pub fn is_valid_job_id(job_id: &str) -> bool {
    let bytes = job_id.as_bytes();
    if bytes.is_empty() || bytes.len() > 240 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Human-readable display title for a workspace log target, matching Go's
/// `devLogTitle`.
pub fn target_title(job_id: &str) -> String {
    match job_id {
        "proxy" => "HTTPS proxy".to_string(),
        "tunnel" => "Cloudflare tunnel".to_string(),
        other => match other.strip_prefix("db-") {
            Some(db_id) if !db_id.is_empty() => db_id.to_string(),
            _ => other.to_string(),
        },
    }
}

/// Case-insensitive substring filter over log lines.
pub fn filter_lines(lines: &[String], query: &str) -> Vec<FilteredLine> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return lines
            .iter()
            .enumerate()
            .map(|(index, line)| FilteredLine {
                index,
                text: line.clone(),
            })
            .collect();
    }
    let needle = trimmed.to_lowercase();
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.to_lowercase().contains(&needle))
        .map(|(index, line)| FilteredLine {
            index,
            text: line.clone(),
        })
        .collect()
}

fn resolve_log_path(
    provider: Option<&Arc<dyn DevLogsProvider>>,
    job_id: &str,
    stream: Stream,
) -> Option<PathBuf> {
    if !is_valid_job_id(job_id) {
        return None;
    }
    let root = provider.map(|p| p.logs_root())?;
    Some(root.join("logs").join(stream.file_name(job_id)))
}

// --- Provider registry -------------------------------------------------------

static FAKE_PROVIDER: LazyLock<Mutex<Option<Arc<dyn DevLogsProvider>>>> =
    LazyLock::new(|| Mutex::new(None));
static FAKE_MODE: AtomicBool = AtomicBool::new(false);
static SYSTEM_PROVIDER: LazyLock<Arc<dyn DevLogsProvider>> =
    LazyLock::new(|| Arc::new(SystemDevLogsProvider::new()));

fn active_provider() -> Option<Arc<dyn DevLogsProvider>> {
    if let Some(provider) = FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return Some(provider);
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return None;
    }
    Some(Arc::clone(&SYSTEM_PROVIDER))
}

/// Switches the log drawer into fake-provider mode for headless tests.
pub fn use_fake_provider() {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Installs a custom [`DevLogsProvider`] for headless tests.
pub fn install_provider(provider: Arc<dyn DevLogsProvider>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

// --- Fine-grained reactive store ---------------------------------------------

#[derive(Debug, Default)]
struct Coordination {
    known: Option<KnownFile>,
    reading: bool,
    read_sequence: u64,
    targets_sequence: u64,
    notice_sequence: u64,
}

#[derive(Clone)]
struct DevLogsStore {
    open: Signal<bool>,
    job_id: Signal<String>,
    stream: Signal<Stream>,
    query: Signal<String>,
    paused: Signal<bool>,
    database_ids: Signal<Vec<String>>,
    path: Signal<Option<PathBuf>>,
    lines: Signal<Option<Vec<String>>>,
    missing: Signal<bool>,
    truncated: Signal<bool>,
    last_updated: Signal<Option<SystemTime>>,
    error: Signal<Option<String>>,
    notice: Signal<Option<String>>,
    lines_revision: Signal<u64>,
    coord: Rc<RefCell<Coordination>>,
}

impl Default for DevLogsStore {
    fn default() -> Self {
        Self {
            open: signal(false),
            job_id: signal(DEFAULT_JOB_ID.to_string()),
            stream: signal(Stream::Stderr),
            query: signal(String::new()),
            paused: signal(false),
            database_ids: signal(Vec::new()),
            path: signal(None),
            lines: signal(None),
            missing: signal(false),
            truncated: signal(false),
            last_updated: signal(None),
            error: signal(None),
            notice: signal(None),
            lines_revision: signal(0),
            coord: Rc::new(RefCell::new(Coordination::default())),
        }
    }
}

fn store() -> DevLogsStore {
    global::<DevLogsStore>().get_untracked()
}

fn task(future: impl Future<Output = ()> + 'static) {
    Owner::detached().run(move || {
        spawn(future);
    });
}

fn later(delay: Duration, f: impl FnOnce() + 'static) {
    Owner::detached().run(move || {
        set_timeout(delay, f);
    });
}

fn set_transient_notice(message: impl Into<String>) {
    let msg = message.into();
    if crate::toast::is_mounted() {
        crate::toast::info(msg);
        return;
    }
    let s = store();
    let seq = {
        let mut coord = s.coord.borrow_mut();
        coord.notice_sequence += 1;
        coord.notice_sequence
    };
    s.notice.set(Some(msg));
    later(Duration::from_secs(2), move || {
        let s = store();
        if s.coord.borrow().notice_sequence == seq {
            s.notice.set(None);
        }
    });
}

// --- Reads (reactive) --------------------------------------------------------

/// Whether the live log drawer is currently open.
pub fn is_open() -> bool {
    store().open.get()
}

/// Whether live polling and auto-follow are paused.
pub fn is_paused() -> bool {
    store().paused.get()
}

/// Currently selected job ID (may be empty/invalid only when an error is shown).
pub fn active_job_id() -> String {
    store().job_id.get()
}

/// Human-readable title for the active job target.
pub fn active_title() -> String {
    target_title(&active_job_id())
}

/// Currently selected log stream (`Error` or `Output`).
pub fn active_stream() -> Stream {
    store().stream.get()
}

/// Active filter query string.
pub fn query() -> String {
    store().query.get()
}

/// Resolved log file path for the current valid target and stream, if any.
pub fn active_path() -> Option<PathBuf> {
    store().path.get()
}

/// Active error text, if any.
pub fn error() -> Option<String> {
    store().error.get()
}

/// Transient notice text (e.g. copy confirmation or Console error).
pub fn notice() -> Option<String> {
    store().notice.get()
}

/// Raw tail lines currently loaded (`None` while the initial read is pending).
pub fn lines() -> Option<Vec<String>> {
    store().lines.get()
}

/// Visible log lines after applying the case-insensitive filter query.
pub fn filtered_lines() -> Vec<FilteredLine> {
    let s = store();
    if s.error.with(Option::is_some) {
        return Vec::new();
    }
    let query = s.query.get();
    s.lines.with(|maybe_lines| match maybe_lines {
        Some(lines) => filter_lines(lines, &query),
        None => Vec::new(),
    })
}

/// Whether there is at least one visible log line available to copy.
pub fn can_copy() -> bool {
    !filtered_lines().is_empty()
}

/// Empty or error state to show in the log viewport when no lines are visible.
pub fn empty_state() -> Option<EmptyState> {
    let s = store();
    if let Some(err) = s.error.get() {
        return Some(EmptyState::Error(err));
    }
    let query = s.query.get();
    let missing = s.missing.get();
    s.lines.with(|maybe_lines| match maybe_lines {
        None => Some(EmptyState::Reading),
        Some(lines) if missing => Some(EmptyState::Missing),
        Some(lines) if lines.is_empty() => Some(EmptyState::Empty),
        Some(lines) => {
            if filter_lines(lines, &query).is_empty() {
                Some(EmptyState::NoMatches)
            } else {
                None
            }
        }
    })
}

/// Selectable targets in display order: `proxy`, `tunnel`, sorted `db-<id>`
/// instances, plus the active valid target if not already present.
pub fn targets() -> Vec<Target> {
    let s = store();
    let mut ids = vec!["proxy".to_string(), "tunnel".to_string()];
    s.database_ids.with(|dbs| {
        for db_id in dbs {
            let candidate = format!("db-{db_id}");
            if is_valid_job_id(&candidate) && !ids.contains(&candidate) {
                ids.push(candidate);
            }
        }
    });
    let current = s.job_id.get();
    if is_valid_job_id(&current) && !ids.contains(&current) {
        ids.push(current);
    }
    ids.into_iter().map(Target::new).collect()
}

/// Monotonic revision bumped whenever lines change or follow resumes.
pub(crate) fn lines_revision() -> u64 {
    store().lines_revision.get()
}

/// Compact metadata summary shown above the log viewport.
pub(crate) fn metadata_summary() -> Option<String> {
    let s = store();
    let path = s.path.get()?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let paused = s.paused.get();
    let truncated = s.truncated.get();
    let query = s.query.get();
    let has_updated = s.last_updated.with(Option::is_some);

    let summary = s.lines.with(|maybe_lines| match maybe_lines {
        Some(lines) if !lines.is_empty() => {
            if !query.trim().is_empty() {
                let matched = filter_lines(lines, &query).len();
                format!("{file_name} · {matched}/{} lines", lines.len())
            } else if paused {
                format!("{file_name} · {} lines · paused", lines.len())
            } else if truncated {
                format!("{file_name} · last {} lines", lines.len())
            } else if has_updated {
                format!("{file_name} · {} lines", lines.len())
            } else {
                file_name
            }
        }
        _ => {
            if paused {
                format!("{file_name} · paused")
            } else {
                file_name
            }
        }
    });
    Some(summary)
}

// --- Commands & off-thread work ----------------------------------------------

/// Opens the drawer on `job_id`. Invalid job IDs are rejected safely: the
/// drawer displays [`INVALID_TARGET_ERROR`] and never calls the provider.
pub fn open(job_id: &str) {
    let s = store();
    let same_valid = is_valid_job_id(job_id) && s.job_id.get_untracked() == job_id;
    let stream = if same_valid {
        s.stream.get_untracked()
    } else {
        Stream::Stderr
    };
    s.open.set(true);
    apply_target_and_stream(job_id, stream, true);
    refresh_targets();
}

/// Opens the drawer on the active valid target, or [`DEFAULT_JOB_ID`] (`"proxy"`)
/// if the current target is empty or invalid.
pub fn open_default() {
    let s = store();
    let current = s.job_id.get_untracked();
    let target = if is_valid_job_id(&current) {
        current
    } else {
        DEFAULT_JOB_ID.to_string()
    };
    let stream = s.stream.get_untracked();
    s.open.set(true);
    apply_target_and_stream(&target, stream, true);
    refresh_targets();
}

/// Toggles the drawer open or closed.
pub fn toggle() {
    if store().open.get_untracked() {
        close();
    } else {
        open_default();
    }
}

/// Closes the drawer and invalidates any in-flight reads.
pub fn close() {
    let s = store();
    {
        let mut coord = s.coord.borrow_mut();
        coord.read_sequence += 1;
        coord.reading = false;
    }
    s.open.set(false);
}

/// Switches the active log target to `job_id`. Invalid job IDs are rejected
/// safely with [`INVALID_TARGET_ERROR`] and never reach the provider.
pub fn select_target(job_id: &str) {
    let s = store();
    let stream = s.stream.get_untracked();
    apply_target_and_stream(job_id, stream, false);
}

/// Switches between [`Stream::Stderr`] (`.err.log`) and [`Stream::Stdout`] (`.log`).
pub fn set_stream(stream: Stream) {
    let s = store();
    let job_id = s.job_id.get_untracked();
    apply_target_and_stream(&job_id, stream, false);
}

/// Updates the case-insensitive filter query.
pub fn set_query(query: String) {
    store().query.set(query);
}

/// Pauses or resumes live log polling and auto-follow.
pub fn set_paused(paused: bool) {
    let s = store();
    let was_paused = s.paused.get_untracked();
    s.paused.set(paused);
    if was_paused && !paused && s.open.get_untracked() {
        s.lines_revision.update(|rev| *rev += 1);
        poll_now(false);
    }
}

/// Forces an immediate refresh of the target list and active log file.
pub fn refresh() {
    let s = store();
    if !s.open.get_untracked() {
        return;
    }
    refresh_targets();
    poll_now(true);
}

/// Periodic 1-second tick. Safe no-op when the drawer is closed, paused, or
/// already reading.
pub fn tick() {
    let s = store();
    if !s.open.get_untracked() || s.paused.get_untracked() {
        return;
    }
    poll_now(false);
}

/// Copies the currently visible (filtered) log lines to the clipboard.
pub fn copy_logs() {
    let visible = filtered_lines();
    if visible.is_empty() {
        return;
    }
    let count = visible.len();
    let joined = visible
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    set_clipboard(joined);
    let label = if count == 1 {
        "Copied 1 line".to_string()
    } else {
        format!("Copied {count} lines")
    };
    set_transient_notice(label);
}

/// Opens the active log file in macOS Console.app via the provider.
pub fn open_in_console() {
    let s = store();
    let Some(path) = s.path.get_untracked() else {
        return;
    };
    let Some(provider) = active_provider() else {
        return;
    };
    task(async move {
        let result = match blocking(move || provider.open_in_console(&path)).await {
            Ok(answer) => answer,
            Err(error) => Err(format!("open in Console failed: {error}")),
        };
        if let Err(error) = result {
            if crate::toast::is_mounted() {
                crate::toast::error(error);
            } else {
                set_transient_notice(error);
            }
        }
    });
}

fn apply_target_and_stream(job_id: &str, stream: Stream, force_reload: bool) {
    let s = store();
    if !is_valid_job_id(job_id) {
        {
            let mut coord = s.coord.borrow_mut();
            coord.read_sequence += 1;
            coord.reading = false;
            coord.known = None;
        }
        batch(|| {
            s.job_id.set(job_id.to_string());
            s.stream.set(stream);
            s.path.set(None);
            s.lines.set(None);
            s.missing.set(false);
            s.truncated.set(false);
            s.last_updated.set(None);
            s.error.set(Some(INVALID_TARGET_ERROR.to_string()));
        });
        return;
    }

    let changed = s.job_id.get_untracked() != job_id
        || s.stream.get_untracked() != stream
        || s.error.with_untracked(Option::is_some);
    let provider = active_provider();
    let next_path = resolve_log_path(provider.as_ref(), job_id, stream);

    if changed || force_reload {
        {
            let mut coord = s.coord.borrow_mut();
            coord.read_sequence += 1;
            coord.reading = false;
            coord.known = None;
        }
        batch(|| {
            s.job_id.set(job_id.to_string());
            s.stream.set(stream);
            s.path.set(next_path);
            if changed {
                s.lines.set(None);
                s.missing.set(false);
                s.truncated.set(false);
                s.last_updated.set(None);
            }
            s.error.set(None);
        });
    }
    if s.open.get_untracked() {
        poll_now(true);
    }
}

fn refresh_targets() {
    let Some(provider) = active_provider() else {
        return;
    };
    let s = store();
    let seq = {
        let mut coord = s.coord.borrow_mut();
        coord.targets_sequence += 1;
        coord.targets_sequence
    };
    task(async move {
        let answer = blocking(move || provider.database_ids()).await;
        let Ok(Ok(mut ids)) = answer else {
            return;
        };
        ids.sort();
        ids.dedup();
        let s = store();
        if s.coord.borrow().targets_sequence != seq {
            return;
        }
        s.database_ids.set(ids);
    });
}

fn poll_now(force: bool) {
    let s = store();
    if !s.open.get_untracked() {
        return;
    }
    let job_id = s.job_id.get_untracked();
    let stream = s.stream.get_untracked();
    if !is_valid_job_id(&job_id) {
        {
            let mut coord = s.coord.borrow_mut();
            coord.read_sequence += 1;
            coord.reading = false;
            coord.known = None;
        }
        batch(|| {
            s.path.set(None);
            s.lines.set(None);
            s.missing.set(false);
            s.truncated.set(false);
            s.error.set(Some(INVALID_TARGET_ERROR.to_string()));
        });
        return;
    }

    let Some(provider) = active_provider() else {
        return;
    };
    let path = resolve_log_path(Some(&provider), &job_id, stream);
    s.path.set(path);

    let (seq, known) = {
        let mut coord = s.coord.borrow_mut();
        if coord.reading && !force {
            return;
        }
        coord.read_sequence += 1;
        coord.reading = true;
        let known = if force { None } else { coord.known };
        (coord.read_sequence, known)
    };

    let req_job_id = job_id.clone();
    task(async move {
        let answer = match blocking(move || provider.read_log(&req_job_id, stream, known)).await {
            Ok(result) => result,
            Err(error) => Err(format!("the log read failed: {error}")),
        };
        let s = store();
        {
            let mut coord = s.coord.borrow_mut();
            if coord.read_sequence != seq
                || !s.open.get_untracked()
                || s.job_id.get_untracked() != job_id
                || s.stream.get_untracked() != stream
            {
                return;
            }
            coord.reading = false;
            match &answer {
                Ok(LogRead::Unchanged) => {}
                Ok(LogRead::Missing) => {
                    coord.known = None;
                }
                Ok(LogRead::Snapshot(snapshot)) => {
                    coord.known = Some(KnownFile {
                        size: snapshot.size,
                        modified: snapshot.modified,
                    });
                }
                Err(_) => {
                    coord.known = None;
                }
            }
        }
        let now = SystemTime::now();
        match answer {
            Ok(LogRead::Unchanged) => {
                s.last_updated.set(Some(now));
            }
            Ok(LogRead::Missing) => {
                batch(|| {
                    s.missing.set(true);
                    s.error.set(None);
                    s.lines.set(Some(Vec::new()));
                    s.truncated.set(false);
                    s.last_updated.set(Some(now));
                });
            }
            Ok(LogRead::Snapshot(snapshot)) => {
                let changed = s
                    .lines
                    .with_untracked(|prev| prev.as_deref() != Some(snapshot.lines.as_slice()));
                batch(|| {
                    s.missing.set(false);
                    s.error.set(None);
                    s.lines.set(Some(snapshot.lines));
                    s.truncated.set(snapshot.truncated);
                    s.last_updated.set(Some(now));
                    if changed {
                        s.lines_revision.update(|rev| *rev += 1);
                    }
                });
            }
            Err(message) => {
                batch(|| {
                    s.missing.set(false);
                    s.error.set(Some(message));
                    s.last_updated.set(Some(now));
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_job_ids_like_go_regex() {
        assert!(is_valid_job_id("proxy"));
        assert!(is_valid_job_id("tunnel"));
        assert!(is_valid_job_id("db-atlas-postgres"));
        assert!(is_valid_job_id("db-pg_16.4"));
        assert!(is_valid_job_id("A"));
        assert!(is_valid_job_id(&"a".repeat(240)));

        // Reject empty, over-long, leading punctuation, path traversal, spaces.
        assert!(!is_valid_job_id(""));
        assert!(!is_valid_job_id(&"a".repeat(241)));
        assert!(!is_valid_job_id("."));
        assert!(!is_valid_job_id(".."));
        assert!(!is_valid_job_id(".hidden"));
        assert!(!is_valid_job_id("-leading-dash"));
        assert!(!is_valid_job_id("_leading_underscore"));
        assert!(!is_valid_job_id("../etc/passwd"));
        assert!(!is_valid_job_id("proxy/../secret"));
        assert!(!is_valid_job_id("foo/bar"));
        assert!(!is_valid_job_id("foo\\bar"));
        assert!(!is_valid_job_id("proxy log"));
        assert!(!is_valid_job_id("proxy\0"));
    }

    #[test]
    fn formats_target_titles_like_go() {
        assert_eq!(target_title("proxy"), "HTTPS proxy");
        assert_eq!(target_title("tunnel"), "Cloudflare tunnel");
        assert_eq!(target_title("db-atlas-postgres"), "atlas-postgres");
        assert_eq!(target_title("db-"), "db-");
        assert_eq!(target_title("custom-job"), "custom-job");
    }

    #[test]
    fn filters_lines_case_insensitively() {
        let lines = vec![
            "INFO connected".to_string(),
            "warn: slow response".to_string(),
            "WARN: retry stream".to_string(),
        ];
        let matched = filter_lines(&lines, "  WARN  ");
        assert_eq!(
            matched,
            vec![
                FilteredLine {
                    index: 1,
                    text: "warn: slow response".to_string(),
                },
                FilteredLine {
                    index: 2,
                    text: "WARN: retry stream".to_string(),
                },
            ]
        );
    }

    #[test]
    fn system_provider_reads_tails_missing_unchanged_and_rejects_traversal() {
        let root = std::env::temp_dir().join(format!(
            "devbase-dev-logs-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let logs_dir = root.join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();

        let provider = SystemDevLogsProvider::with_root(root.clone());

        // Invalid target rejected before touching disk.
        assert_eq!(
            provider.read_log("../etc/passwd", Stream::Stderr, None),
            Err(INVALID_TARGET_ERROR.to_string())
        );

        // Missing file reports LogRead::Missing.
        assert_eq!(
            provider.read_log("proxy", Stream::Stderr, None),
            Ok(LogRead::Missing)
        );

        // Empty file returns empty lines.
        let out_path = logs_dir.join("proxy.log");
        std::fs::write(&out_path, "").unwrap();
        let empty = provider.read_log("proxy", Stream::Stdout, None).unwrap();
        match empty {
            LogRead::Snapshot(snap) => {
                assert!(snap.lines.is_empty());
                assert!(!snap.truncated);
            }
            other => panic!("expected empty snapshot, got {other:?}"),
        }

        // Populated file returns lines and short-circuits when unchanged.
        let err_path = logs_dir.join("proxy.err.log");
        std::fs::write(&err_path, "line-1\nline-2\n").unwrap();
        let snap = match provider.read_log("proxy", Stream::Stderr, None).unwrap() {
            LogRead::Snapshot(snap) => snap,
            other => panic!("expected snapshot, got {other:?}"),
        };
        assert_eq!(snap.lines, vec!["line-1", "line-2"]);
        let known = KnownFile {
            size: snap.size,
            modified: snap.modified,
        };
        assert_eq!(
            provider.read_log("proxy", Stream::Stderr, Some(known)),
            Ok(LogRead::Unchanged)
        );

        // Missing state.json returns empty database_ids cleanly.
        assert_eq!(provider.database_ids(), Ok(Vec::new()));

        std::fs::remove_dir_all(&root).ok();
    }
}
