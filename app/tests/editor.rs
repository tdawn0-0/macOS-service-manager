//! Unit and headless UI tests for the LaunchAgent / LaunchDaemon plist
//! editor. Every test installs an in-memory fake [`EditorProvider`] so no test
//! ever touches real `~/Library/LaunchAgents`, `/Library/LaunchAgents`,
//! `/Library/LaunchDaemons`, `plutil`, or `launchctl`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use devcore::{
    launchd::{Calendar, Spec},
    service::Source,
};
use nsh_app_devbase::{
    editor::store::{FormSnapshot, change_warning, create_warning, log_default_path, tilde_path},
    testing::{
        EditorCommandMode, EditorProvider, EditorRestartPolicy, EditorSaveResult, EditorSavedEvent,
        EditorScheduleMode, clear_editor_saved_callbacks, editor_close, editor_is_open,
        editor_last_saved, editor_notice, editor_on_saved, editor_open_edit,
        editor_open_edit_with_apply, editor_open_new, editor_panel, editor_revision,
        install_editor_provider, use_fake_editor,
    },
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

/// All headless editor tests share the process-wide fake provider registry.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- In-memory fake EditorProvider -------------------------------------------

#[derive(Debug, Clone)]
struct SaveCall {
    source: Source,
    spec: Spec,
    existing: Option<PathBuf>,
    load: bool,
    marshaled: Vec<u8>,
}

struct FakeEditorProvider {
    home: Option<PathBuf>,
    /// Stored raw plist files keyed by virtual path.
    files: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
    /// Recorded save calls in order.
    saves: Mutex<Vec<SaveCall>>,
    /// Optional error returned by `save`.
    save_error: Mutex<Option<String>>,
    /// Optional load_error returned alongside a successful write.
    load_error: Mutex<Option<String>>,
}

impl FakeEditorProvider {
    fn new() -> Arc<Self> {
        Arc::new(FakeEditorProvider {
            home: Some(PathBuf::from("/Users/tester")),
            files: Mutex::new(BTreeMap::new()),
            saves: Mutex::new(Vec::new()),
            save_error: Mutex::new(None),
            load_error: Mutex::new(None),
        })
    }

    fn insert_plist(&self, path: impl Into<PathBuf>, xml: &str) {
        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path.into(), xml.as_bytes().to_vec());
    }

    fn set_save_error(&self, err: impl Into<String>) {
        *self.save_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(err.into());
    }

    fn set_load_error(&self, err: impl Into<String>) {
        *self.load_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(err.into());
    }

    fn saves(&self) -> Vec<SaveCall> {
        self.saves.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn file_bytes(&self, path: &Path) -> Option<Vec<u8>> {
        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(path)
            .cloned()
    }

    fn dir_for(&self, source: Source) -> PathBuf {
        match source {
            Source::UserAgent => self
                .home
                .as_ref()
                .map(|h| h.join("Library/LaunchAgents"))
                .unwrap_or_else(|| PathBuf::from("/tmp/fake-agents")),
            Source::SystemAgent => PathBuf::from("/Library/LaunchAgents"),
            Source::Daemon => PathBuf::from("/Library/LaunchDaemons"),
            _ => PathBuf::from("/tmp/fake-agents"),
        }
    }
}

impl EditorProvider for FakeEditorProvider {
    fn load(&self, path: &Path) -> Result<Spec, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let bytes = self.read_raw(path)?;
        // Parse via an ephemeral temp file so we exercise devcore's real Spec::load.
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!(
            "devbase-editor-test-{}-{n}.plist",
            std::process::id()
        ));
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        let loaded = Spec::load(&tmp).map_err(|e| e.to_string());
        let _ = std::fs::remove_file(&tmp);
        loaded
    }

    fn read_raw(&self, path: &Path) -> Result<Vec<u8>, String> {
        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(path)
            .cloned()
            .ok_or_else(|| format!("plist not found: {}", path.display()))
    }

    fn save(
        &self,
        source: Source,
        spec: Spec,
        existing: Option<PathBuf>,
        load: bool,
    ) -> Result<EditorSaveResult, String> {
        if let Some(err) = self
            .save_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            return Err(err);
        }

        let current = match &existing {
            Some(path) => Some(self.read_raw(path)?),
            None => None,
        };
        let marshaled = spec
            .marshal(current.as_deref())
            .map_err(|e| e.to_string())?;

        let path = match &existing {
            Some(p) => p.clone(),
            None => self.dir_for(source).join(format!("{}.plist", spec.label)),
        };

        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path.clone(), marshaled.clone());
        self.saves
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(SaveCall {
                source,
                spec,
                existing,
                load,
                marshaled,
            });

        let load_error = self
            .load_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        Ok(EditorSaveResult { path, load_error })
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

// --- Test harness component --------------------------------------------------

static EDIT_TARGET: Mutex<Option<(Source, PathBuf, bool)>> = Mutex::new(None);

fn set_edit_target(source: Source, path: impl Into<PathBuf>, apply: bool) {
    *EDIT_TARGET.lock().unwrap_or_else(|p| p.into_inner()) = Some((source, path.into(), apply));
}

#[component]
fn EditorHarness() -> impl IntoElement {
    view()
        .test_id("editor-harness")
        .size_full()
        .flex_col()
        .child(
            view()
                .flex_row()
                .gap(px(4.0))
                .child(
                    nsh_base::button()
                        .test_id("open-new-user")
                        .on_press(|| editor_open_new(Source::UserAgent))
                        .child(text("New User Agent")),
                )
                .child(
                    nsh_base::button()
                        .test_id("open-new-system")
                        .on_press(|| editor_open_new(Source::SystemAgent))
                        .child(text("New System Agent")),
                )
                .child(
                    nsh_base::button()
                        .test_id("open-new-daemon")
                        .on_press(|| editor_open_new(Source::Daemon))
                        .child(text("New Daemon")),
                )
                .child(
                    nsh_base::button()
                        .test_id("open-edit-target")
                        .on_press(|| {
                            let target = EDIT_TARGET
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .clone();
                            if let Some((source, path, apply)) = target {
                                if apply {
                                    editor_open_edit(source, path);
                                } else {
                                    editor_open_edit_with_apply(source, path, false);
                                }
                            }
                        })
                        .child(text("Edit Target")),
                )
                .child(
                    nsh_base::button()
                        .test_id("close-editor")
                        .on_press(editor_close)
                        .child(text("Force Close")),
                ),
        )
        .child(
            text(move || {
                let open = editor_is_open();
                let rev = editor_revision();
                let last = editor_last_saved()
                    .map(|e| format!("{}/{}/{}", e.source.as_str(), e.label, e.loaded))
                    .unwrap_or_else(|| "none".to_string());
                let notice = editor_notice().unwrap_or_default();
                format!("status open={open} rev={rev} last={last} notice={notice}")
            })
            .test_id("harness-status"),
        )
        .child(when(editor_is_open, editor_panel))
}

fn start_harness(provider: Arc<FakeEditorProvider>) -> (MutexGuard<'static, ()>, Headless) {
    let guard = serial();
    use_fake_editor();
    install_editor_provider(provider);
    let mut app = Headless::start(EngineConfig::new(Root::of::<EditorHarness>())).unwrap();
    app.wait_for("id:editor-harness").unwrap();
    (guard, app)
}

// --- Pure unit tests ---------------------------------------------------------

#[test]
fn form_snapshot_validates_label_command_schedule_env_and_default_logs() {
    let home = Path::new("/Users/tester");

    // Empty form reports label guidance first.
    let mut form = FormSnapshot {
        source: Source::UserAgent,
        run_at_load: true,
        log_default: true,
        ..FormSnapshot::default()
    };
    let err = form.build_spec(Some(home)).unwrap_err();
    assert!(err.contains("the label may hold letters"), "err = {err}");

    // Empty shell command fails once label is valid.
    form.label = "com.example.unit".to_string();
    let err = form.build_spec(Some(home)).unwrap_err();
    assert_eq!(err, "give the command to run");

    // Shell mode wraps command in /bin/sh -c and applies default log path.
    form.args = "echo hello".to_string();
    let spec = form.build_spec(Some(home)).unwrap();
    assert_eq!(spec.args, vec!["/bin/sh", "-c", "echo hello"]);
    assert_eq!(
        spec.stdout,
        "/Users/tester/Library/Logs/com.example.unit.log"
    );
    assert_eq!(spec.stderr, spec.stdout);

    // Preserves custom shell path (e.g. `sh`) when editing an existing plist.
    form.existing = Some(PathBuf::from(
        "/Users/tester/Library/LaunchAgents/com.example.unit.plist",
    ));
    form.shell = "sh".to_string();
    let spec = form.build_spec(Some(home)).unwrap();
    assert_eq!(spec.args, vec!["sh", "-c", "echo hello"]);
    form.existing = None;
    form.shell.clear();

    // Program mode requires absolute executable path on new jobs.
    form.command_mode = EditorCommandMode::Program;
    form.args = "relative-bin\n--flag".to_string();
    let err = form.build_spec(Some(home)).unwrap_err();
    assert!(err.contains("absolute path"), "err = {err}");

    form.args = "/opt/homebrew/bin/node\n/Users/tester/server.js".to_string();
    let spec = form.build_spec(Some(home)).unwrap();
    assert_eq!(
        spec.args,
        vec!["/opt/homebrew/bin/node", "/Users/tester/server.js"]
    );

    // Interval mode rejects empty / zero / non-numeric intervals.
    form.schedule_mode = EditorScheduleMode::Interval;
    form.interval = "0".to_string();
    assert_eq!(
        form.build_spec(Some(home)).unwrap_err(),
        "the interval is a number of seconds"
    );
    form.interval = "120".to_string();
    let spec = form.build_spec(Some(home)).unwrap();
    assert_eq!(spec.start_interval, 120);

    // Calendar mode rejects empty calendar unless custom, and validates field bounds.
    form.schedule_mode = EditorScheduleMode::Calendar;
    assert_eq!(
        form.build_spec(Some(home)).unwrap_err(),
        "give at least one field of the calendar"
    );
    form.cal_minute = "61".to_string();
    assert!(form.build_spec(Some(home)).unwrap_err().contains("0-59"));
    form.cal_minute = "30".to_string();
    form.cal_hour = "2".to_string();
    let spec = form.build_spec(Some(home)).unwrap();
    assert_eq!(
        spec.calendar,
        Calendar {
            minute: "30".to_string(),
            hour: "2".to_string(),
            ..Calendar::default()
        }
    );

    // Environment parses KEY=value lines and rejects malformed lines.
    form.env = "NOT_A_PAIR".to_string();
    assert!(
        form.build_spec(Some(home))
            .unwrap_err()
            .contains("KEY=value")
    );
    form.env = "A=1\nB=two=three".to_string();
    let spec = form.build_spec(Some(home)).unwrap();
    assert_eq!(spec.env.get("A").map(String::as_str), Some("1"));
    assert_eq!(spec.env.get("B").map(String::as_str), Some("two=three"));

    // Group without user is rejected.
    form.group_name = "staff".to_string();
    assert!(
        form.build_spec(Some(home))
            .unwrap_err()
            .contains("needs a user")
    );
    form.user_name = "_daemon".to_string();
    let spec = form.build_spec(Some(home)).unwrap();
    assert_eq!(spec.user_name, "_daemon");
    assert_eq!(spec.group_name, "staff");

    // Restart policy mapping.
    form.schedule_mode = EditorScheduleMode::Service;
    form.restart = EditorRestartPolicy::OnCrash;
    let spec = form.build_spec(Some(home)).unwrap();
    assert!(spec.restart_on_crash);
    assert!(!spec.keep_alive);
}

#[test]
fn default_log_paths_tilde_formatting_and_root_warnings() {
    let home = Path::new("/Users/tester");
    assert_eq!(
        log_default_path(Source::UserAgent, "", "com.example.a", Some(home)).as_deref(),
        Some("/Users/tester/Library/Logs/com.example.a.log")
    );
    assert_eq!(
        log_default_path(Source::Daemon, "", "com.example.d", Some(home)).as_deref(),
        Some("/Library/Logs/com.example.d.log")
    );
    assert_eq!(
        log_default_path(Source::Daemon, "_backup", "com.example.d", Some(home)),
        None
    );
    assert_eq!(
        log_default_path(Source::SystemAgent, "", "com.example.s", Some(home)),
        None
    );

    assert_eq!(
        tilde_path("/Users/tester/Library/Logs/a.log", Some(home)),
        "~/Library/Logs/a.log"
    );
    assert_eq!(
        tilde_path("/Library/Logs/a.log", Some(home)),
        "/Library/Logs/a.log"
    );

    let warn = create_warning(Source::Daemon, true);
    assert!(warn.contains("/Library/LaunchDaemons"));
    assert!(warn.contains("loads the job now"));
    assert!(warn.contains("without a macOS password prompt"));

    let chg = change_warning(&["RunAtLoad".to_string(), "UserName".to_string()], true);
    assert!(chg.contains("This changes RunAtLoad, UserName."));
    assert!(chg.contains("restarts it if it runs"));
    assert!(chg.contains("without a macOS password prompt"));
}

// --- Headless UI tests -------------------------------------------------------

#[test]
fn new_user_agent_validates_saves_closes_and_invokes_callback() {
    let provider = FakeEditorProvider::new();
    let (_guard, mut app) = start_harness(Arc::clone(&provider));

    let saved_events: Arc<Mutex<Vec<EditorSavedEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&saved_events);
    editor_on_saved(move |event| {
        sink.lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(event.clone());
    });

    app.press("id:open-new-user").unwrap();
    app.wait_for("New Job").unwrap();
    assert!(app.screen().shows("User Agent"));
    assert!(app.screen().shows("System Agent"));
    assert!(app.screen().shows("Daemon"));
    assert!(app.screen().shows("As a service"));
    assert!(
        app.screen()
            .shows("Write to ~/Library/Logs/<label>.log if no log file is set")
    );

    // Invalid empty form surfaces error and writes nothing.
    app.press("id:editor-save").unwrap();
    app.wait_for(
        "the label may hold letters, digits, dots, dashes and underscores, as com.example.job",
    )
    .unwrap();
    assert!(provider.saves().is_empty());

    // Fill label and shell command without losing focus.
    app.type_text("id:editor-label", "com.example.job").unwrap();
    app.type_text("id:editor-args", "sleep 1 && echo done")
        .unwrap();
    app.press("id:editor-save").unwrap();

    // Editor closes, revision increments, callback fires.
    app.wait_for(
        "status open=false rev=1 last=user-agent/com.example.job/true notice=com.example.job saved",
    )
    .unwrap();

    let saves = provider.saves();
    assert_eq!(saves.len(), 1);
    let call = &saves[0];
    assert_eq!(call.source, Source::UserAgent);
    assert!(call.existing.is_none());
    assert!(call.load);
    assert_eq!(call.spec.label, "com.example.job");
    assert_eq!(
        call.spec.args,
        vec!["/bin/sh", "-c", "sleep 1 && echo done"]
    );
    assert!(call.spec.run_at_load);
    assert_eq!(
        call.spec.stdout,
        "/Users/tester/Library/Logs/com.example.job.log"
    );
    assert_eq!(call.spec.stderr, call.spec.stdout);

    let events = saved_events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].label, "com.example.job");
    assert_eq!(events[0].source, Source::UserAgent);
    assert!(events[0].loaded);

    clear_editor_saved_callbacks();
}

#[test]
fn new_daemon_asks_confirmation_and_handles_runs_as_and_default_log() {
    let provider = FakeEditorProvider::new();
    let (_guard, mut app) = start_harness(Arc::clone(&provider));

    let seen_dialog: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen_dialog);
    app.on_dialog(move |dialog| {
        *seen_clone.lock().unwrap() = Some((
            dialog.title.clone().unwrap_or_default(),
            dialog.message.clone().unwrap_or_default(),
        ));
        DialogAnswer::Press("Create Job".to_string())
    });

    app.press("id:open-new-daemon").unwrap();
    app.wait_for("Starts at boot").unwrap();
    assert!(app.screen().shows("Runs as"));
    assert!(app.screen().shows("User"));
    assert!(app.screen().shows("Group"));
    assert!(app.screen().shows("Root directory"));
    assert!(
        app.screen()
            .shows("Asks for in-app confirmation before saving")
    );
    assert!(
        app.screen()
            .shows("Write to /Library/Logs/<label>.log if no log file is set")
    );

    app.type_text("id:editor-label", "com.example.nightly")
        .unwrap();
    app.type_text("id:editor-args", "/usr/local/bin/backup")
        .unwrap();
    app.type_text("id:editor-user", "_backup").unwrap();
    // Once a non-root user is entered, the /Library/Logs default option disappears.
    app.wait_for_gone(
        "Write to /Library/Logs/<label>.log if no log file is set",
        Duration::from_secs(2),
    )
    .unwrap();

    app.press("id:editor-save").unwrap();
    app.wait_for("status open=false rev=1 last=daemon/com.example.nightly/true notice=com.example.nightly saved")
        .unwrap();

    let (title, message) = seen_dialog.lock().unwrap().clone().expect("dialog shown");
    assert_eq!(title, "Create system job com.example.nightly?");
    assert!(message.contains("/Library/LaunchDaemons"));
    assert!(message.contains("without a macOS password prompt"));

    let saves = provider.saves();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].source, Source::Daemon);
    assert_eq!(saves[0].spec.user_name, "_backup");
    assert!(saves[0].spec.stdout.is_empty());
    assert!(saves[0].load);
}

#[test]
fn system_agent_hides_runs_as_and_default_log() {
    let provider = FakeEditorProvider::new();
    let (_guard, mut app) = start_harness(Arc::clone(&provider));

    app.press("id:open-new-system").unwrap();
    app.wait_for("Starts at login").unwrap();
    assert!(
        app.screen()
            .shows("Asks for in-app confirmation before saving")
    );
    assert!(!app.screen().shows("Runs as"));
    assert!(!app.screen().shows("Root directory"));
    assert!(!app.screen().shows("if no log file is set"));
}

#[test]
fn schedule_modes_drop_service_only_fields_and_validate_inputs() {
    let provider = FakeEditorProvider::new();
    let (_guard, mut app) = start_harness(Arc::clone(&provider));

    app.press("id:open-new-user").unwrap();
    app.wait_for("New Job").unwrap();

    app.type_text("id:editor-label", "com.example.sched")
        .unwrap();
    app.type_text("id:editor-args", "/bin/date").unwrap();
    app.press("id:editor-restart-crash").unwrap();
    app.press("id:editor-start-on-mount").unwrap();
    app.type_text("id:editor-queue-dirs", "/tmp/a, /tmp/b")
        .unwrap();

    // Switch to interval mode: service-only controls disappear.
    app.press("id:editor-when-interval").unwrap();
    app.wait_for("Run every (seconds)").unwrap();
    assert!(app.screen().shows("Also starts at login"));
    assert!(!app.screen().shows("Queue directories"));
    assert!(!app.screen().shows("Restarts"));

    app.press("id:editor-save").unwrap();
    app.wait_for("the interval is a number of seconds").unwrap();

    // Switch to calendar mode.
    app.press("id:editor-when-calendar").unwrap();
    app.wait_for("Weekday").unwrap();
    assert!(app.screen().shows("Minute"));
    assert!(app.screen().shows("Hour"));
    assert!(app.screen().shows("Day"));
    assert!(app.screen().shows("Month"));

    app.press("id:editor-save").unwrap();
    app.wait_for("give at least one field of the calendar")
        .unwrap();

    app.type_text("id:editor-cal-minute", "61").unwrap();
    app.press("id:editor-save").unwrap();
    app.wait_for("the minute 61 is outside 0-59").unwrap();

    app.clear("id:editor-cal-minute").unwrap();
    app.type_text("id:editor-cal-minute", "15").unwrap();
    app.type_text("id:editor-cal-hour", "9-17").unwrap();
    app.type_text("id:editor-cal-weekday", "1-5").unwrap();
    app.press("id:editor-save").unwrap();

    app.wait_for("status open=false rev=1 last=user-agent/com.example.sched/true notice=com.example.sched saved")
        .unwrap();

    let saves = provider.saves();
    assert_eq!(saves.len(), 1);
    let spec = &saves[0].spec;
    assert_eq!(spec.start_interval, 0);
    assert!(!spec.restart_on_crash);
    assert!(!spec.keep_alive);
    assert!(!spec.start_on_mount);
    assert!(spec.queue_dirs.is_empty());
    assert_eq!(spec.calendar.minute, "15");
    assert_eq!(spec.calendar.hour, "9-17");
    assert_eq!(spec.calendar.weekday, "1-5");
}

#[test]
fn editing_existing_plist_preserves_custom_keep_alive_calendar_and_unknown_keys() {
    let provider = FakeEditorProvider::new();
    let path = PathBuf::from("/Users/tester/Library/LaunchAgents/com.example.custom.plist");
    provider.insert_plist(
        &path,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.example.custom</string>
<key>ProgramArguments</key><array><string>/usr/bin/env</string><string>node</string><string>app.js</string></array>
<key>KeepAlive</key><dict><key>NetworkState</key><true/></dict>
<key>StartCalendarInterval</key><array>
  <dict><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
  <dict><key>Hour</key><integer>18</integer><key>Minute</key><integer>30</integer></dict>
</array>
<key>EnvironmentVariables</key><dict><key>FOO</key><string>bar</string></dict>
<key>Nice</key><integer>5</integer>
</dict></plist>"#,
    );

    let (_guard, mut app) = start_harness(Arc::clone(&provider));
    set_edit_target(Source::UserAgent, &path, true);

    app.press("id:open-edit-target").unwrap();
    app.wait_for("Edit Job").unwrap();
    app.wait_for("A calendar of its own, which stays as it is")
        .unwrap();
    assert!(app.screen().shows("Reload it after saving"));
    // Kind tabs are hidden when editing an existing job.
    assert!(!app.screen().matches("id:editor-kind-tabs").unwrap());

    // Update working directory and save.
    app.type_text("id:editor-workdir", "/tmp/work").unwrap();
    app.press("id:editor-save").unwrap();
    app.wait_for("status open=false rev=1 last=user-agent/com.example.custom/true notice=com.example.custom saved")
        .unwrap();

    let saves = provider.saves();
    assert_eq!(saves.len(), 1);
    assert!(saves[0].spec.keep_alive_custom);
    assert!(saves[0].spec.calendar_custom);
    assert_eq!(saves[0].spec.working_dir, "/tmp/work");

    let xml = String::from_utf8(provider.file_bytes(&path).unwrap()).unwrap();
    assert!(
        xml.contains("NetworkState"),
        "custom KeepAlive lost:\n{xml}"
    );
    assert!(
        xml.contains("<integer>18</integer>") && xml.contains("<integer>30</integer>"),
        "custom StartCalendarInterval lost:\n{xml}"
    );
    assert!(xml.contains("<key>Nice</key>"), "unknown key lost:\n{xml}");
    assert!(xml.contains("/tmp/work"), "working dir missing:\n{xml}");
}

#[test]
fn editing_mixed_plist_without_changing_schedule_keeps_hidden_service_fields() {
    let provider = FakeEditorProvider::new();
    let path = PathBuf::from("/Users/tester/Library/LaunchAgents/com.example.mixed.plist");
    provider.insert_plist(
        &path,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.example.mixed</string>
<key>ProgramArguments</key><array><string>/bin/sh</string><string>-c</string><string>date</string></array>
<key>StartInterval</key><integer>60</integer>
<key>KeepAlive</key><true/>
<key>StartOnMount</key><true/>
<key>QueueDirectories</key><array><string>/tmp/q</string></array>
<key>Nice</key><integer>5</integer>
</dict></plist>"#,
    );

    let (_guard, mut app) = start_harness(Arc::clone(&provider));
    set_edit_target(Source::UserAgent, &path, false);

    app.press("id:open-edit-target").unwrap();
    app.wait_for("Edit Job").unwrap();
    app.wait_for("Run every (seconds)").unwrap();

    // Save without touching schedule mode: hidden service keys must survive.
    app.press("id:editor-save").unwrap();
    app.wait_for("status open=false rev=1 last=user-agent/com.example.mixed/false notice=com.example.mixed saved")
        .unwrap();

    let saves = provider.saves();
    assert_eq!(saves.len(), 1);
    let spec = &saves[0].spec;
    assert!(!saves[0].load);
    assert_eq!(spec.start_interval, 60);
    assert!(spec.keep_alive);
    assert!(spec.start_on_mount);
    assert_eq!(spec.queue_dirs, vec!["/tmp/q"]);
    let xml = String::from_utf8(saves[0].marshaled.clone()).unwrap();
    assert!(xml.contains("<key>Nice</key>"));
}

#[test]
fn editing_root_job_handles_noop_cancel_and_confirmed_diff() {
    let provider = FakeEditorProvider::new();
    let path = PathBuf::from("/Library/LaunchDaemons/com.vendor.helper.plist");
    provider.insert_plist(
        &path,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.vendor.helper</string>
<key>Program</key><string>/opt/vendor/helper</string>
<key>UserName</key><string>_vendor</string>
<key>MachServices</key><dict><key>com.vendor.helper</key><true/></dict>
</dict></plist>"#,
    );

    let (_guard, mut app) = start_harness(Arc::clone(&provider));
    set_edit_target(Source::Daemon, &path, true);

    // 1) No-op save closes immediately without confirmation or write.
    app.press("id:open-edit-target").unwrap();
    app.wait_for("Edit Job").unwrap();
    app.wait_for("Runs as").unwrap();
    app.press("id:editor-save").unwrap();
    app.wait_for(
        "status open=false rev=0 last=none notice=com.vendor.helper has no changes to save",
    )
    .unwrap();
    assert!(provider.saves().is_empty());

    // 2) Cancel on confirmation keeps editor open and writes nothing.
    let answer_confirm = Arc::new(Mutex::new(false));
    let seen_dialog: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
    let answer_flag = Arc::clone(&answer_confirm);
    let seen_clone = Arc::clone(&seen_dialog);
    app.on_dialog(move |dialog| {
        *seen_clone.lock().unwrap() = Some((
            dialog.title.clone().unwrap_or_default(),
            dialog.message.clone().unwrap_or_default(),
        ));
        if *answer_flag.lock().unwrap() {
            DialogAnswer::Press("Save Changes".to_string())
        } else {
            DialogAnswer::Press("Cancel".to_string())
        }
    });

    app.press("id:open-edit-target").unwrap();
    app.wait_for("Edit Job").unwrap();
    app.wait_for("Runs as").unwrap();
    app.clear("id:editor-user").unwrap();
    app.type_text("id:editor-user", "_other").unwrap();
    app.press("id:editor-save").unwrap();
    app.wait(Duration::from_millis(200)).unwrap();

    // Editor stayed open and nothing was saved.
    assert!(app.screen().shows("Edit Job"));
    assert!(provider.saves().is_empty());
    let (title, message) = seen_dialog.lock().unwrap().clone().expect("diff dialog");
    assert_eq!(title, "Save changes to com.vendor.helper?");
    assert!(
        message.contains("This changes UserName."),
        "msg = {message}"
    );
    assert!(message.contains("without a macOS password prompt"));

    // 3) Confirm saves the change and closes editor.
    *answer_confirm.lock().unwrap() = true;
    app.press("id:editor-save").unwrap();
    app.wait_for(
        "status open=false rev=1 last=daemon/com.vendor.helper/true notice=com.vendor.helper saved",
    )
    .unwrap();

    let saves = provider.saves();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].spec.user_name, "_other");
    assert_eq!(saves[0].existing.as_deref(), Some(path.as_path()));
    let xml = String::from_utf8(saves[0].marshaled.clone()).unwrap();
    assert!(xml.contains("MachServices"), "unknown key lost:\n{xml}");
}

#[test]
fn saved_but_not_loaded_keeps_editor_open_as_existing_and_reports_error() {
    let provider = FakeEditorProvider::new();
    provider.set_load_error("saved, but it did not load: Input/output error");
    let (_guard, mut app) = start_harness(Arc::clone(&provider));
    app.on_dialog(|_| DialogAnswer::Press("Create Job".to_string()));

    app.press("id:open-new-daemon").unwrap();
    app.wait_for("New Job").unwrap();
    app.type_text("id:editor-label", "com.example.d").unwrap();
    app.type_text("id:editor-args", "/bin/date").unwrap();
    app.press("id:editor-save").unwrap();

    // Error shows, editor transitions to Edit Job on the newly-written path,
    // and revision bumps so callers refresh.
    app.wait_for("saved, but it did not load: Input/output error")
        .unwrap();
    app.wait_for("Edit Job").unwrap();
    assert!(app.screen().shows(
        "/Library/LaunchDaemons/com.example.d.plist · Asks for in-app confirmation before saving"
    ));
    assert!(
        app.screen()
            .shows("status open=true rev=1 last=daemon/com.example.d/false notice=")
    );
}

#[test]
fn save_failure_keeps_form_open_and_does_not_bump_revision() {
    let provider = FakeEditorProvider::new();
    provider.set_save_error("authorization canceled");
    let (_guard, mut app) = start_harness(Arc::clone(&provider));
    app.on_dialog(|_| DialogAnswer::Press("Create Job".to_string()));

    app.press("id:open-new-daemon").unwrap();
    app.wait_for("New Job").unwrap();
    app.type_text("id:editor-label", "com.example.fail")
        .unwrap();
    app.type_text("id:editor-args", "/bin/date").unwrap();
    app.press("id:editor-save").unwrap();

    app.wait_for("authorization canceled").unwrap();
    assert!(app.screen().shows("New Job"));
    assert!(
        app.screen()
            .shows("status open=true rev=0 last=none notice=")
    );
}

#[test]
fn editing_service_with_custom_keep_alive_shows_custom_label_and_preserves_conditions() {
    let provider = FakeEditorProvider::new();
    let path = PathBuf::from("/Users/tester/Library/LaunchAgents/com.example.keep.plist");
    provider.insert_plist(
        &path,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.example.keep</string>
<key>ProgramArguments</key><array><string>/usr/bin/true</string></array>
<key>KeepAlive</key><dict><key>PathState</key><dict><key>/tmp/ready</key><true/></dict></dict>
</dict></plist>"#,
    );

    let (_guard, mut app) = start_harness(Arc::clone(&provider));
    set_edit_target(Source::UserAgent, &path, true);

    app.press("id:open-edit-target").unwrap();
    app.wait_for("By conditions of its own").unwrap();
    assert!(!app.screen().matches("id:editor-restart-policy").unwrap());

    app.press("id:editor-save").unwrap();
    app.wait_for("status open=false rev=1 last=user-agent/com.example.keep/true notice=com.example.keep saved")
        .unwrap();

    let xml = String::from_utf8(provider.file_bytes(&path).unwrap()).unwrap();
    assert!(xml.contains("PathState"), "custom KeepAlive lost:\n{xml}");
    assert!(
        xml.contains("/tmp/ready"),
        "custom KeepAlive path lost:\n{xml}"
    );
}

#[test]
fn program_mode_env_kind_switching_cancel_and_missing_file_load_error() {
    let provider = FakeEditorProvider::new();
    let (_guard, mut app) = start_harness(Arc::clone(&provider));

    // Open new UserAgent, switch kind tab to Daemon, then back to UserAgent.
    app.press("id:open-new-user").unwrap();
    app.wait_for("New Job").unwrap();
    app.press("id:editor-kind-daemon").unwrap();
    app.wait_for("Runs as").unwrap();
    app.press("id:editor-kind-user-agent").unwrap();
    app.wait_for_gone("Runs as", Duration::from_secs(2))
        .unwrap();

    // Switch to Program mode, enter multiline args + env, then cancel -> nothing saved.
    app.press("id:editor-cmd-program").unwrap();
    app.type_text("id:editor-label", "com.example.prog")
        .unwrap();
    app.type_text("id:editor-args", "/opt/homebrew/bin/node\n/tmp/app.js")
        .unwrap();
    app.type_text("id:editor-env", "NODE_ENV=development\nPORT=3000")
        .unwrap();
    app.press("id:editor-cancel").unwrap();
    app.wait_for("status open=false rev=0 last=none notice=")
        .unwrap();
    assert!(provider.saves().is_empty());

    // Open edit on a missing plist -> surfaces load error without panicking.
    set_edit_target(
        Source::UserAgent,
        "/Users/tester/Library/LaunchAgents/missing.plist",
        true,
    );
    app.press("id:open-edit-target").unwrap();
    app.wait_for("plist not found: /Users/tester/Library/LaunchAgents/missing.plist")
        .unwrap();
}
