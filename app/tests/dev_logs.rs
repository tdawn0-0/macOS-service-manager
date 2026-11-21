//! Headless UI tests for the workspace live log drawer, ported and expanded
//! from `986e9b4:dev_logs_ui_test.go`.
//!
//! All tests install an in-memory fake [`DevLogsProvider`]: no test touches
//! host disk, reads real `state.json`, or launches macOS Console.app.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use nsh_app_devbase::dev_logs::{
    self, DevLogsProvider, KnownFile, LogRead, LogSnapshot, Stream, install_provider,
    use_fake_provider,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

/// Headless tests share the fake-provider registry, so they serialize access.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- Test harness root -------------------------------------------------------

#[component]
fn DrawerHarness() -> impl IntoElement {
    view()
        .test_id("drawer-harness")
        .size_full()
        .flex_col()
        .child(
            view()
                .flex_none()
                .flex_row()
                .gap(px(4.0))
                .p(px(8.0))
                .child(
                    nsh_base::button()
                        .test_id("harness-toggle")
                        .on_press(dev_logs::toggle)
                        .child(text("Logs")),
                )
                .child(
                    nsh_base::button()
                        .test_id("harness-open-default")
                        .on_press(dev_logs::open_default)
                        .child(text("Live logs")),
                )
                .child(
                    nsh_base::button()
                        .test_id("harness-open-db")
                        .on_press(|| dev_logs::open("db-atlas-postgres"))
                        .child(text("Open DB log")),
                )
                .child(
                    nsh_base::button()
                        .test_id("harness-open-invalid")
                        .on_press(|| dev_logs::open("../etc/passwd"))
                        .child(text("Open invalid")),
                )
                .child(
                    nsh_base::button()
                        .test_id("harness-select-invalid")
                        .on_press(|| dev_logs::select_target("bad/path"))
                        .child(text("Select invalid")),
                )
                .child(
                    nsh_base::button()
                        .test_id("harness-tick")
                        .on_press(dev_logs::tick)
                        .child(text("Tick")),
                ),
        )
        .child(view().flex_1().min_h(px(0.0)).child(text("Workspace body")))
        .child(dev_logs::drawer())
}

// --- In-memory fake provider -------------------------------------------------

#[derive(Clone)]
struct FakeEntry {
    lines: Vec<String>,
    truncated: bool,
    version: u64,
}

struct FakeProvider {
    root: PathBuf,
    database_ids: Mutex<Vec<String>>,
    files: Mutex<BTreeMap<(String, Stream), FakeEntry>>,
    delays: Mutex<BTreeMap<String, Duration>>,
    read_calls: Mutex<Vec<(String, Stream)>>,
    console_calls: Mutex<Vec<PathBuf>>,
    next_version: AtomicU64,
}

impl FakeProvider {
    fn new(database_ids: Vec<&str>) -> Arc<Self> {
        Arc::new(Self {
            root: PathBuf::from("/virtual/devbase-home"),
            database_ids: Mutex::new(database_ids.into_iter().map(str::to_owned).collect()),
            files: Mutex::new(BTreeMap::new()),
            delays: Mutex::new(BTreeMap::new()),
            read_calls: Mutex::new(Vec::new()),
            console_calls: Mutex::new(Vec::new()),
            next_version: AtomicU64::new(1),
        })
    }

    fn set_lines(&self, job_id: &str, stream: Stream, lines: Vec<&str>) {
        let version = self.next_version.fetch_add(1, Ordering::Relaxed);
        self.files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                (job_id.to_string(), stream),
                FakeEntry {
                    lines: lines.into_iter().map(str::to_owned).collect(),
                    truncated: false,
                    version,
                },
            );
    }

    fn append_line(&self, job_id: &str, stream: Stream, line: &str) {
        let version = self.next_version.fetch_add(1, Ordering::Relaxed);
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = files
            .entry((job_id.to_string(), stream))
            .or_insert_with(|| FakeEntry {
                lines: Vec::new(),
                truncated: false,
                version,
            });
        entry.lines.push(line.to_string());
        entry.version = version;
    }

    fn set_delay(&self, job_id: &str, delay: Duration) {
        self.delays
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(job_id.to_string(), delay);
    }

    fn read_calls(&self) -> Vec<(String, Stream)> {
        self.read_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn console_calls(&self) -> Vec<PathBuf> {
        self.console_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl DevLogsProvider for FakeProvider {
    fn logs_root(&self) -> PathBuf {
        self.root.clone()
    }

    fn database_ids(&self) -> Result<Vec<String>, String> {
        Ok(self
            .database_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn read_log(
        &self,
        job_id: &str,
        stream: Stream,
        known: Option<KnownFile>,
    ) -> Result<LogRead, String> {
        self.read_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((job_id.to_string(), stream));

        let delay = self
            .delays
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(job_id)
            .copied();
        if let Some(delay) = delay {
            thread::sleep(delay);
        }

        let files = self
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(entry) = files.get(&(job_id.to_string(), stream)) else {
            return Ok(LogRead::Missing);
        };
        let modified = Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000 + entry.version));
        let size: u64 = entry.lines.iter().map(|line| (line.len() + 1) as u64).sum();
        if let Some(prev) = known
            && prev.size == size
            && prev.modified == modified
        {
            return Ok(LogRead::Unchanged);
        }
        Ok(LogRead::Snapshot(LogSnapshot {
            lines: entry.lines.clone(),
            truncated: entry.truncated,
            size,
            modified,
        }))
    }

    fn open_in_console(&self, path: &Path) -> Result<(), String> {
        self.console_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(path.to_path_buf());
        Ok(())
    }
}

fn start(
    provider: Arc<FakeProvider>,
    width: f32,
    height: f32,
) -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    use_fake_provider();
    install_provider(provider);
    let mut app = Headless::start(EngineConfig::new(Root::of::<DrawerHarness>())).unwrap();
    app.set_viewport(Size::new(px(width), px(height))).unwrap();
    app.wait_for("Workspace body").unwrap();
    (turn, app)
}

fn populated_provider() -> Arc<FakeProvider> {
    let provider = FakeProvider::new(vec!["atlas-postgres"]);
    provider.set_lines("proxy", Stream::Stderr, vec!["proxy-err-1", "proxy-err-2"]);
    provider.set_lines("proxy", Stream::Stdout, vec!["proxy-out-1"]);
    provider.set_lines(
        "tunnel",
        Stream::Stderr,
        vec![
            "INFO connected",
            "warn: slow response",
            "WARN: retry stream",
        ],
    );
    provider.set_lines("db-atlas-postgres", Stream::Stderr, vec!["postgres ready"]);
    provider
}

// --- Tests -------------------------------------------------------------------

#[test]
fn drawer_lifecycle_open_close_and_reopen_across_sizes() {
    for (width, height) in [(900.0, 520.0), (1120.0, 720.0)] {
        let provider = populated_provider();
        let (_turn, mut app) = start(provider, width, height);

        assert!(!app.screen().matches("id:dev-log-drawer").unwrap());

        // Toggle opens on default target (proxy, Error stream).
        app.press("id:harness-toggle").unwrap();
        app.wait_for("proxy-err-1").unwrap();
        assert!(app.screen().matches("id:dev-log-drawer").unwrap());
        assert!(app.screen().shows("proxy-err-2"));
        assert!(app.screen().matches("id:dev-log-target-proxy").unwrap());
        assert!(app.screen().matches("id:dev-log-target-tunnel").unwrap());
        assert!(app.screen().matches("id:dev-log-stream-stderr").unwrap());
        assert!(app.screen().matches("id:dev-log-stream-stdout").unwrap());
        assert!(app.screen().matches("id:dev-log-filter").unwrap());
        assert!(app.screen().matches("id:dev-log-follow-switch").unwrap());
        assert!(app.screen().matches("id:dev-log-copy").unwrap());
        assert!(app.screen().matches("id:dev-log-open-console").unwrap());
        assert!(app.screen().matches("id:dev-log-close").unwrap());

        // Close button collapses the drawer.
        app.press("id:dev-log-close").unwrap();
        app.wait_for_gone("proxy-err-1", Duration::from_secs(2))
            .unwrap();
        assert!(!app.screen().matches("id:dev-log-drawer").unwrap());

        // Reopen via Live logs trigger.
        app.press("id:harness-open-default").unwrap();
        app.wait_for("proxy-err-1").unwrap();
        assert!(app.screen().matches("id:dev-log-drawer").unwrap());
    }
}

#[test]
fn switching_targets_streams_filter_and_open_in_console() {
    let provider = populated_provider();
    let (_turn, mut app) = start(Arc::clone(&provider), 1120.0, 720.0);

    app.press("id:harness-toggle").unwrap();
    app.wait_for("proxy-err-1").unwrap();
    app.wait_for("atlas-postgres").unwrap();

    // Switch to Output stream (.log).
    app.press("id:dev-log-stream-stdout").unwrap();
    app.wait_for("proxy-out-1").unwrap();
    assert!(!app.screen().shows("proxy-err-1"));

    // Switch back to Error stream and select tunnel.
    app.press("id:dev-log-stream-stderr").unwrap();
    app.wait_for("proxy-err-1").unwrap();
    app.press("id:dev-log-target-tunnel").unwrap();
    app.wait_for("INFO connected").unwrap();
    assert!(app.screen().shows("warn: slow response"));
    assert!(app.screen().shows("WARN: retry stream"));

    // Case-insensitive query filtering.
    app.type_text("id:dev-log-filter", "WARN").unwrap();
    app.wait_for_gone("INFO connected", Duration::from_secs(2))
        .unwrap();
    assert!(app.screen().shows("warn: slow response"));
    assert!(app.screen().shows("WARN: retry stream"));

    // Non-matching filter shows empty-match message.
    app.clear("id:dev-log-filter").unwrap();
    app.type_text("id:dev-log-filter", "does-not-exist")
        .unwrap();
    app.wait_for("No matching log lines.").unwrap();

    // Clear filter and switch to managed database instance db-atlas-postgres.
    app.clear("id:dev-log-filter").unwrap();
    app.wait_for("INFO connected").unwrap();
    app.press("id:dev-log-target-db-atlas-postgres").unwrap();
    app.wait_for("postgres ready").unwrap();

    // Open in Console sends the exact log path to the provider.
    app.press("id:dev-log-open-console").unwrap();
    let deadline = SystemTime::now() + Duration::from_secs(2);
    while provider.console_calls().is_empty() && SystemTime::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        provider.console_calls(),
        vec![PathBuf::from(
            "/virtual/devbase-home/logs/db-atlas-postgres.err.log"
        )]
    );
}

fn scroll_node_id(app: &Headless) -> nsh_core::NodeId {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("dev-log-scroll"))
        .expect("dev-log-scroll should be mounted")
        .id
}

#[test]
fn live_polling_updates_lines_and_pause_switch_halts_polling() {
    let provider = populated_provider();
    let (_turn, mut app) = start(Arc::clone(&provider), 1120.0, 720.0);

    app.press("id:harness-toggle").unwrap();
    app.wait_for("proxy-err-1").unwrap();
    assert!(app.screen().shows("Live"));
    let scroll_id_before = scroll_node_id(&app);

    // Append a line; the 1-second live poller picks it up without recreating the scroll viewport.
    provider.append_line("proxy", Stream::Stderr, "proxy-err-3");
    app.wait_for("proxy-err-3").unwrap();
    assert_eq!(
        scroll_node_id(&app),
        scroll_id_before,
        "scroll container must remain stable across 1-second live updates"
    );
    assert!(app.screen().shows("proxy-err-1"));
    assert!(app.screen().shows("proxy-err-2"));

    // Toggle the live follow switch to Paused.
    app.press("id:dev-log-follow-switch").unwrap();
    app.wait_for("Paused").unwrap();

    let calls_before = provider.read_calls().len();
    provider.append_line("proxy", Stream::Stderr, "proxy-err-while-paused");
    app.press("id:harness-tick").unwrap();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(provider.read_calls().len(), calls_before);
    assert!(!app.screen().shows("proxy-err-while-paused"));

    // Resuming live follow immediately picks up the buffered line on the same scroll container.
    app.press("id:dev-log-follow-switch").unwrap();
    app.wait_for("proxy-err-while-paused").unwrap();
    assert_eq!(scroll_node_id(&app), scroll_id_before);
    assert!(app.screen().shows("Live"));

    // Closing the drawer stops 1-second live polling.
    app.press("id:dev-log-close").unwrap();
    app.wait_for_gone("proxy-err-1", Duration::from_secs(2))
        .unwrap();
    let calls_after_close = provider.read_calls().len();
    thread::sleep(Duration::from_millis(1_150));
    assert_eq!(provider.read_calls().len(), calls_after_close);
}

#[test]
fn open_directly_targets_database_instance() {
    let provider = populated_provider();
    let (_turn, mut app) = start(provider, 1120.0, 720.0);

    app.press("id:harness-open-db").unwrap();
    app.wait_for("postgres ready").unwrap();
    assert!(app.screen().matches("id:dev-log-drawer").unwrap());
    assert!(
        app.screen()
            .matches("id:dev-log-target-db-atlas-postgres")
            .unwrap()
    );
}

#[test]
fn invalid_job_ids_are_rejected_without_calling_provider() {
    let provider = populated_provider();
    let (_turn, mut app) = start(Arc::clone(&provider), 1120.0, 720.0);

    // Open with a path traversal attempt.
    app.press("id:harness-open-invalid").unwrap();
    app.wait_for("Invalid log target").unwrap();
    assert!(app.screen().matches("id:dev-log-drawer").unwrap());
    assert!(!app.screen().enabled("id:dev-log-open-console").unwrap());
    assert!(!app.screen().enabled("id:dev-log-copy").unwrap());
    assert!(
        provider
            .read_calls()
            .iter()
            .all(|(id, _)| id != "../etc/passwd"),
        "invalid job_id must never reach provider.read_log: {:?}",
        provider.read_calls()
    );

    // Recover onto a valid target via open_default, then attempt another invalid select_target.
    app.press("id:harness-open-default").unwrap();
    app.wait_for("proxy-err-1").unwrap();
    app.press("id:harness-select-invalid").unwrap();
    app.wait_for("Invalid log target").unwrap();
    assert!(
        provider.read_calls().iter().all(|(id, _)| id != "bad/path"),
        "invalid job_id must never reach provider.read_log: {:?}",
        provider.read_calls()
    );
}

#[test]
fn empty_missing_and_newly_created_log_states() {
    let provider = FakeProvider::new(vec![]);
    provider.set_lines("proxy", Stream::Stdout, vec![]);
    let (_turn, mut app) = start(Arc::clone(&provider), 1120.0, 720.0);

    // Missing proxy.err.log shows "No logs yet.".
    app.press("id:harness-toggle").unwrap();
    app.wait_for("No logs yet.").unwrap();
    assert!(!app.screen().enabled("id:dev-log-copy").unwrap());

    // Creating the missing log file makes the next poll display its content.
    provider.set_lines("proxy", Stream::Stderr, vec!["booted"]);
    app.press("id:harness-tick").unwrap();
    app.wait_for("booted").unwrap();
    assert!(app.screen().enabled("id:dev-log-copy").unwrap());

    // Switching to an empty 0-byte log shows "The log is empty.".
    app.press("id:dev-log-stream-stdout").unwrap();
    app.wait_for("The log is empty.").unwrap();
    assert!(!app.screen().enabled("id:dev-log-copy").unwrap());
}

#[test]
fn copy_action_copies_visible_filtered_lines_to_clipboard() {
    let provider = populated_provider();
    let (_turn, mut app) = start(provider, 1120.0, 720.0);

    app.press("id:harness-toggle").unwrap();
    app.wait_for("proxy-err-1").unwrap();

    // Copy all visible lines of proxy.err.log.
    app.press("id:dev-log-copy").unwrap();
    app.wait_for("Copied 2 lines").unwrap();
    assert_eq!(app.screen().clipboard(), Some("proxy-err-1\nproxy-err-2"));

    // Switch to tunnel, filter to WARN, and copy only filtered lines.
    app.press("id:dev-log-target-tunnel").unwrap();
    app.wait_for("INFO connected").unwrap();
    app.type_text("id:dev-log-filter", "WARN").unwrap();
    app.wait_for_gone("INFO connected", Duration::from_secs(2))
        .unwrap();

    app.press("id:dev-log-copy").unwrap();
    app.wait_for("Copied 2 lines").unwrap();
    assert_eq!(
        app.screen().clipboard(),
        Some("warn: slow response\nWARN: retry stream")
    );
}

#[test]
fn slow_read_never_overwrites_newer_target_selection() {
    let provider = populated_provider();
    provider.set_delay("proxy", Duration::from_millis(250));
    let (_turn, mut app) = start(provider, 1120.0, 720.0);

    // Opening proxy starts a slow 250ms read; immediately switch to tunnel.
    app.press("id:harness-toggle").unwrap();
    app.wait_for("Reading…").unwrap();
    app.press("id:dev-log-target-tunnel").unwrap();
    app.wait_for("INFO connected").unwrap();

    // Wait longer than the slow proxy read to prove its stale response is discarded.
    thread::sleep(Duration::from_millis(350));
    assert!(app.screen().shows("INFO connected"));
    assert!(!app.screen().shows("proxy-err-1"));
}
