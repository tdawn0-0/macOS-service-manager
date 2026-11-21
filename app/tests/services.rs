//! The inspector, headless: each launchd source's jobs, the search, the
//! detail groups, the provider's action matrix, the confirmation path and the
//! busy state while an action runs. A fake provider stands in for launchd:
//! no test reads the real LaunchAgents directories or runs launchctl.

use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use devcore::service::{
    Action, Autostart, BoxError, Detail, Item, LogFile, Provider, Row, Source, State,
};
use nsh_core::{Checked, prelude::*};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

use nsh_app_devbase::testing::{
    SystemCacheClear, SystemCacheInfo, SystemCliChange, SystemCliStatus, SystemHelperChange,
    SystemHelperStatus, SystemLoginChange, SystemSettingsProvider, SystemSettingsSnapshot,
    install_provider, install_system_settings_provider, use_fake_providers, use_fake_system,
};

/// The service tests share one fake-provider registry, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Normal,
    /// Starting the job asks first.
    ConfirmStart,
    /// Every action takes this long, so the busy state can be seen.
    Slow,
    /// An action on an admin item fails like the missing helper does.
    HelperMissing,
}

struct FakeProvider {
    source: Source,
    items: Mutex<Vec<Item>>,
    logs: Mutex<Vec<LogFile>>,
    behaviour: Behaviour,
}

impl FakeProvider {
    fn new(source: Source, items: Vec<Item>, behaviour: Behaviour) -> Arc<Self> {
        Arc::new(FakeProvider {
            source,
            items: Mutex::new(items),
            logs: Mutex::new(Vec::new()),
            behaviour,
        })
    }

    fn with_logs(self: Arc<Self>, logs: Vec<LogFile>) -> Arc<Self> {
        *self
            .logs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = logs;
        self
    }
}

impl Provider for FakeProvider {
    fn source(&self) -> Source {
        self.source
    }

    fn list(&self) -> Result<Vec<Item>, BoxError> {
        Ok(self
            .items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn detail(&self, item: &Item) -> Result<Detail, BoxError> {
        Ok(Detail {
            rows: vec![
                Row {
                    group: "Status".to_string(),
                    key: "Label".to_string(),
                    value: item.name.clone(),
                },
                Row {
                    group: "Status".to_string(),
                    key: "Domain".to_string(),
                    value: item.owner.clone(),
                },
                Row {
                    group: "Program".to_string(),
                    key: "Command".to_string(),
                    value: format!("/usr/bin/{}", item.name),
                },
                Row {
                    group: "Files".to_string(),
                    key: "Property list".to_string(),
                    value: item.path.display().to_string(),
                },
            ],
            logs: self
                .logs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        })
    }

    fn actions(&self, item: &Item) -> Vec<Action> {
        let mut actions = match item.state {
            State::Running if item.keep_alive => vec![Action::Stop, Action::Restart],
            State::Running => vec![Action::Stop, Action::Restart, Action::Unload],
            State::Loaded | State::Scheduled | State::Failed => {
                vec![Action::Start, Action::Unload]
            }
            _ => vec![Action::Load],
        };
        if item.autostart == Autostart::Off {
            actions.push(Action::Enable);
        } else {
            actions.push(Action::Disable);
        }
        actions.push(Action::Remove);
        actions
    }

    fn confirm(&self, item: &Item, action: Action) -> Option<String> {
        match (self.behaviour, action) {
            (Behaviour::ConfirmStart, Action::Start) => Some(format!("Start {} now?", item.name)),
            (_, Action::Remove) => Some(format!("Remove {}?", item.name)),
            _ => None,
        }
    }

    fn do_action(&self, item: &Item, action: Action) -> Result<(), BoxError> {
        if self.behaviour == Behaviour::Slow {
            thread::sleep(Duration::from_millis(500));
        }
        if self.behaviour == Behaviour::HelperMissing && item.admin {
            return Err("the privileged helper is not installed".into());
        }
        let mut items = self
            .items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match action {
            Action::Remove => items.retain(|held| held.id != item.id),
            Action::Start | Action::Load | Action::Restart => {
                if let Some(held) = items.iter_mut().find(|held| held.id == item.id) {
                    held.state = State::Running;
                    held.pid = 900;
                }
            }
            Action::Stop | Action::Unload => {
                if let Some(held) = items.iter_mut().find(|held| held.id == item.id) {
                    held.state = if action == Action::Unload {
                        State::Stopped
                    } else {
                        State::Loaded
                    };
                    held.pid = 0;
                }
            }
            Action::Enable => {
                if let Some(held) = items.iter_mut().find(|held| held.id == item.id) {
                    held.autostart = Autostart::On;
                }
            }
            Action::Disable => {
                if let Some(held) = items.iter_mut().find(|held| held.id == item.id) {
                    held.autostart = Autostart::Off;
                }
            }
        }
        Ok(())
    }
}

fn item(source: Source, name: &str, state: State, autostart: Autostart, admin: bool) -> Item {
    Item {
        id: format!("{}:{}", source.as_str(), name),
        source,
        name: name.to_string(),
        state,
        pid: if state == State::Running { 4242 } else { 0 },
        autostart,
        owner: match source {
            Source::Daemon => "system".to_string(),
            _ => "gui/501".to_string(),
        },
        path: PathBuf::from(format!("/tmp/devbase-tests/{name}.plist")),
        kind: None,
        admin,
        keep_alive: false,
    }
}

fn user_agents(behaviour: Behaviour) -> Arc<FakeProvider> {
    FakeProvider::new(
        Source::UserAgent,
        vec![
            item(
                Source::UserAgent,
                "com.example.alpha",
                State::Running,
                Autostart::On,
                false,
            ),
            item(
                Source::UserAgent,
                "com.example.bravo",
                State::Loaded,
                Autostart::On,
                false,
            ),
            item(
                Source::UserAgent,
                "com.example.charlie",
                State::Stopped,
                Autostart::Off,
                false,
            ),
        ],
        behaviour,
    )
}

fn system_agents() -> Arc<FakeProvider> {
    FakeProvider::new(
        Source::SystemAgent,
        vec![item(
            Source::SystemAgent,
            "com.example.beta",
            State::Loaded,
            Autostart::On,
            false,
        )],
        Behaviour::Normal,
    )
}

fn daemons(behaviour: Behaviour) -> Arc<FakeProvider> {
    FakeProvider::new(
        Source::Daemon,
        vec![item(
            Source::Daemon,
            "com.example.gamma",
            State::Running,
            Autostart::On,
            true,
        )],
        behaviour,
    )
}

/// Installs one fake per source and starts the surface, holding the tests'
/// turn for as long as the returned guard lives.
fn start(
    user: Arc<FakeProvider>,
    system: Arc<FakeProvider>,
    daemon: Arc<FakeProvider>,
) -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    use_fake_providers();
    install_provider(Source::UserAgent, user);
    install_provider(Source::SystemAgent, system);
    install_provider(Source::Daemon, daemon);
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    (turn, app)
}

fn normal_app() -> (MutexGuard<'static, ()>, Headless) {
    start(
        user_agents(Behaviour::Normal),
        system_agents(),
        daemons(Behaviour::Normal),
    )
}

#[test]
fn switching_source_shows_that_sources_items() {
    let (_turn, mut app) = normal_app();
    app.wait_for("com.example.alpha").unwrap();

    app.press("id:source-system-agent").unwrap();
    app.wait_for("com.example.beta").unwrap();
    assert!(!app.screen().shows("com.example.alpha"));

    app.press("id:source-daemon").unwrap();
    app.wait_for("com.example.gamma").unwrap();
    assert!(!app.screen().shows("com.example.beta"));

    app.press("id:source-user-agent").unwrap();
    app.wait_for("com.example.alpha").unwrap();
}

#[test]
fn search_filters_the_list_by_name() {
    let (_turn, mut app) = normal_app();
    app.wait_for("com.example.alpha").unwrap();
    app.wait_for("com.example.bravo").unwrap();

    app.type_text("id:services-search", "alpha").unwrap();
    app.wait_for_gone("com.example.bravo", Duration::from_secs(2))
        .unwrap();
    assert!(app.screen().shows("com.example.alpha"));

    // Clearing the field restores the whole list.
    app.clear("id:services-search").unwrap();
    app.wait_for("com.example.bravo").unwrap();
}

#[test]
fn selecting_an_item_shows_its_detail_groups() {
    let (_turn, mut app) = normal_app();
    app.wait_for("com.example.alpha").unwrap();

    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("Program").unwrap();
    assert!(app.screen().shows("Files"));
    assert!(app.screen().shows("Property list"));
    assert!(
        app.screen()
            .shows("/tmp/devbase-tests/com.example.alpha.plist")
    );
    assert!(app.screen().shows("Running · PID 4242"));
}

#[test]
fn actions_follow_the_state_matrix() {
    let (_turn, mut app) = normal_app();
    app.wait_for("com.example.alpha").unwrap();

    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("/tmp/devbase-tests/com.example.alpha.plist")
        .unwrap();
    assert!(
        app.screen()
            .matches("id:inspector >> role:button[Stop]")
            .unwrap()
    );
    assert!(
        app.screen()
            .matches("id:inspector >> role:button[Restart]")
            .unwrap()
    );

    // Idle: Start is the main action, and Restart does not apply.
    app.press("id:item-com.example.bravo").unwrap();
    app.wait_for("/tmp/devbase-tests/com.example.bravo.plist")
        .unwrap();
    assert!(
        app.screen()
            .matches("id:inspector >> role:button[Start]")
            .unwrap()
    );
    assert!(
        !app.screen()
            .matches("id:inspector >> role:button[Restart]")
            .unwrap()
    );

    // Stopped and not autostarted: Load.
    app.press("id:item-com.example.charlie").unwrap();
    app.wait_for("/tmp/devbase-tests/com.example.charlie.plist")
        .unwrap();
    assert!(
        app.screen()
            .matches("id:inspector >> role:button[Load]")
            .unwrap()
    );
    assert!(
        !app.screen()
            .matches("id:inspector >> role:button[Stop]")
            .unwrap()
    );
}

#[test]
fn a_confirming_action_waits_for_the_dialog() {
    let (turn, mut app) = start(
        user_agents(Behaviour::ConfirmStart),
        system_agents(),
        daemons(Behaviour::Normal),
    );
    app.on_dialog(|_| DialogAnswer::Press("Start".to_string()));
    app.wait_for("com.example.bravo").unwrap();

    app.press("id:item-com.example.bravo").unwrap();
    app.wait_for("Start").unwrap();
    app.press("id:action-com.example.bravo").unwrap();
    app.wait_for("Running · PID 900").unwrap();

    drop(app);
    drop(turn);
}

#[test]
fn cancelling_a_confirming_action_changes_nothing() {
    let (turn, mut app) = start(
        user_agents(Behaviour::ConfirmStart),
        system_agents(),
        daemons(Behaviour::Normal),
    );
    app.wait_for("com.example.bravo").unwrap();
    app.press("id:item-com.example.bravo").unwrap();
    app.wait_for("Start").unwrap();

    app.press("id:action-com.example.bravo").unwrap();
    // The default answer presses Cancel: the action must not run.
    app.wait(Duration::from_millis(800)).unwrap();
    assert!(!app.screen().shows("Running · PID 900"));

    drop(app);
    drop(turn);
}

#[test]
fn an_action_shows_the_busy_state_while_it_runs() {
    let (_turn, mut app) = start(
        user_agents(Behaviour::Slow),
        system_agents(),
        daemons(Behaviour::Normal),
    );
    app.wait_for("com.example.alpha").unwrap();
    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("Stop").unwrap();

    app.press("id:action-com.example.alpha").unwrap();
    app.wait_for("Working").unwrap();
    app.wait_for("Idle").unwrap();
    app.wait_for_gone("Working", Duration::from_secs(2))
        .unwrap();
}

#[test]
fn a_privileged_action_reports_the_missing_helper() {
    let (_turn, mut app) = start(
        user_agents(Behaviour::Normal),
        system_agents(),
        daemons(Behaviour::HelperMissing),
    );
    app.wait_for("com.example.alpha").unwrap();
    app.press("id:source-daemon").unwrap();
    app.wait_for("com.example.gamma").unwrap();
    app.press("id:item-com.example.gamma").unwrap();
    app.wait_for("Stop").unwrap();

    app.press("id:action-com.example.gamma").unwrap();
    app.wait_for("com.example.gamma: the privileged helper is not installed")
        .unwrap();
}

struct FakeSettings {
    snapshot: Mutex<SystemSettingsSnapshot>,
}

impl FakeSettings {
    fn new(helper_installed: bool) -> Arc<Self> {
        Arc::new(FakeSettings {
            snapshot: Mutex::new(SystemSettingsSnapshot {
                helper: SystemHelperStatus {
                    installed: helper_installed,
                    message: if helper_installed {
                        "Installed. Root operations run through it without password prompts."
                            .to_string()
                    } else {
                        "Not installed. Root operations need it; installing asks for your administrator password once."
                            .to_string()
                    },
                },
                cli: SystemCliStatus {
                    installed: false,
                    wrapper_path: "/tmp/devbase-tests/bin/devbase".to_string(),
                    target_executable: "/tmp/devbase-tests/Devbase".to_string(),
                    in_path: false,
                    managed_by_devbase: false,
                },
                cache: SystemCacheInfo {
                    directory: "/tmp/devbase-tests/downloads".to_string(),
                    files: 0,
                    bytes: 0,
                },
                log_rotation_enabled: true,
                login_at_login: false,
                workspace_root: "/tmp/devbase-tests/root".to_string(),
            }),
        })
    }
}

impl SystemSettingsProvider for FakeSettings {
    fn snapshot(&self) -> Result<SystemSettingsSnapshot, String> {
        Ok(self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn install_helper(&self) -> SystemHelperChange {
        let status = SystemHelperStatus {
            installed: true,
            message: "Installed. Root operations run through it without password prompts."
                .to_string(),
        };
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .helper = status.clone();
        SystemHelperChange {
            status,
            error: None,
        }
    }

    fn uninstall_helper(&self) -> SystemHelperChange {
        let status = SystemHelperStatus {
            installed: false,
            message: "Not installed. Root operations need it; installing asks for your administrator password once."
                .to_string(),
        };
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .helper = status.clone();
        SystemHelperChange {
            status,
            error: None,
        }
    }

    fn install_cli(&self) -> SystemCliChange {
        SystemCliChange::default()
    }

    fn uninstall_cli(&self) -> SystemCliChange {
        SystemCliChange::default()
    }

    fn clear_cache(&self) -> Result<SystemCacheClear, String> {
        Ok(SystemCacheClear::default())
    }

    fn set_log_rotation(&self, _enabled: bool) -> Result<(), String> {
        Ok(())
    }

    fn set_login_item(&self, enabled: bool) -> SystemLoginChange {
        SystemLoginChange {
            enabled,
            error: None,
        }
    }

    fn open_full_disk_access(&self) -> Result<(), String> {
        Ok(())
    }

    fn open_login_items(&self) -> Result<(), String> {
        Ok(())
    }

    fn reveal(&self, _path: &str) -> Result<(), String> {
        Ok(())
    }

    fn preview_shutdown(&self) -> Result<devcore::shutdown::ShutdownPlan, String> {
        Ok(devcore::shutdown::ShutdownPlan::default())
    }

    fn shutdown(&self, _plan: &devcore::shutdown::ShutdownPlan) -> Result<(), String> {
        Ok(())
    }
}

struct TempLogFile {
    path: PathBuf,
}

impl TempLogFile {
    fn new(initial: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "devbase-services-log-{}-{n}.log",
            std::process::id()
        ));
        std::fs::write(&path, initial).unwrap();
        TempLogFile { path }
    }

    fn append(&self, line: &str) {
        let mut file = OpenOptions::new().append(true).open(&self.path).unwrap();
        write!(file, "{line}").unwrap();
        file.flush().unwrap();
    }
}

impl Drop for TempLogFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn helper_banner_follows_helper_installation_status() {
    let helper_note = "The privileged helper is not installed. Changing system agents and daemons takes root, so those actions will fail until it is.";
    let turn = serial();
    use_fake_providers();
    install_provider(Source::UserAgent, user_agents(Behaviour::Normal));
    install_provider(Source::SystemAgent, system_agents());
    install_provider(Source::Daemon, daemons(Behaviour::Normal));
    use_fake_system();
    install_system_settings_provider(FakeSettings::new(false));

    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("com.example.alpha").unwrap();

    // Leftover phase footer is gone, and User Agents never show the helper warning.
    assert!(!app.screen().shows("Phase 1 · three launchd sources"));
    assert!(!app.screen().shows(helper_note));

    // When the helper is not installed, System Agents and Daemons show the warning.
    app.press("id:source-system-agent").unwrap();
    app.wait_for("com.example.beta").unwrap();
    assert!(app.screen().shows(helper_note));

    app.press("id:source-daemon").unwrap();
    app.wait_for("com.example.gamma").unwrap();
    assert!(app.screen().shows(helper_note));

    // Installing the helper in Settings immediately hides the warning on Services.
    app.press("role:button[Settings]").unwrap();
    app.wait_for("id:settings-install-helper").unwrap();
    app.press("id:settings-install-helper").unwrap();
    app.wait_for("Privileged helper installed").unwrap();

    app.press("role:button[Services]").unwrap();
    app.wait_for("com.example.gamma").unwrap();
    assert!(!app.screen().shows(helper_note));

    app.press("id:source-system-agent").unwrap();
    app.wait_for("com.example.beta").unwrap();
    assert!(!app.screen().shows(helper_note));

    // Removing the helper in Settings brings the warning back.
    app.press("role:button[Settings]").unwrap();
    app.wait_for("id:settings-remove-helper").unwrap();
    app.press("id:settings-remove-helper").unwrap();
    app.wait_for("Privileged helper removed").unwrap();

    app.press("role:button[Services]").unwrap();
    app.wait_for("com.example.beta").unwrap();
    assert!(app.screen().shows(helper_note));

    drop(app);
    drop(turn);
}

#[test]
fn log_tab_updates_without_tearing_down_inspector_or_scroll() {
    let log_file = TempLogFile::new("alpha line 1\nalpha line 2\n");
    let user = user_agents(Behaviour::Normal).with_logs(vec![LogFile {
        label: "Standard output".to_string(),
        path: log_file.path.clone(),
    }]);
    let (_turn, mut app) = start(user, system_agents(), daemons(Behaviour::Normal));
    app.wait_for("com.example.alpha").unwrap();

    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("id:tab-logs").unwrap();
    assert!(app.screen().matches("role:tablist").unwrap());
    assert!(app.screen().matches("role:tab[Info]").unwrap());
    assert!(app.screen().matches("role:tab[Logs]").unwrap());

    app.press("id:tab-logs").unwrap();
    app.wait_for("alpha line 1").unwrap();
    app.wait_for("alpha line 2").unwrap();
    app.wait_for("id:log-scroll").unwrap();

    let scroll_id_before = app.screen().id("id:log-scroll").unwrap();
    let switch_id_before = app.screen().id("id:autostart-toggle").unwrap();
    let tab_id_before = app.screen().id("id:tab-logs").unwrap();
    let inspector_id_before = app.screen().id("id:inspector").unwrap();

    // Appending a line should be picked up by the 1-second log poll without
    // recreating the inspector pane, tabs, switch, or log scroll container.
    log_file.append("alpha line 3\n");
    app.wait_for("alpha line 3").unwrap();

    assert!(app.screen().shows("alpha line 1"));
    assert!(app.screen().shows("alpha line 2"));
    assert!(app.screen().shows("alpha line 3"));
    assert_eq!(app.screen().id("id:log-scroll").unwrap(), scroll_id_before);
    assert_eq!(
        app.screen().id("id:autostart-toggle").unwrap(),
        switch_id_before
    );
    assert_eq!(app.screen().id("id:tab-logs").unwrap(), tab_id_before);
    assert_eq!(
        app.screen().id("id:inspector").unwrap(),
        inspector_id_before
    );
}

#[test]
fn autostart_switch_and_filter_controls_use_native_roles() {
    let (_turn, mut app) = normal_app();
    app.wait_for("com.example.alpha").unwrap();
    app.wait_for("com.example.bravo").unwrap();
    app.wait_for("com.example.charlie").unwrap();

    // Filter toggles filter the list and expose pressed accessibility state.
    assert_eq!(
        app.screen()
            .accessibility_state("id:filter-all")
            .unwrap()
            .pressed,
        Some(Checked::True)
    );
    app.press("id:filter-running").unwrap();
    app.wait_for_gone("com.example.bravo", Duration::from_secs(2))
        .unwrap();
    assert!(app.screen().shows("com.example.alpha"));
    assert_eq!(
        app.screen()
            .accessibility_state("id:filter-running")
            .unwrap()
            .pressed,
        Some(Checked::True)
    );
    app.press("id:filter-stopped").unwrap();
    app.wait_for("com.example.charlie").unwrap();
    assert!(!app.screen().shows("com.example.alpha"));
    app.press("id:filter-all").unwrap();
    app.wait_for("com.example.alpha").unwrap();

    // Autostart control uses nsh_base::switch with ARIA switch role and checked state.
    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("id:autostart-toggle").unwrap();
    assert!(app.screen().matches("id:inspector >> role:switch").unwrap());
    assert_eq!(
        app.screen()
            .accessibility_state("id:autostart-toggle")
            .unwrap()
            .checked,
        Some(Checked::True)
    );

    app.press("id:autostart-toggle").unwrap();
    app.wait_for("com.example.alpha disabled").unwrap();
    assert_eq!(
        app.screen()
            .accessibility_state("id:autostart-toggle")
            .unwrap()
            .checked,
        Some(Checked::False)
    );
}
