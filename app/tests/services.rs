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
    Action, Autostart, BoxError, Detail, Item, LogFile, Provider, Restricted, Restriction, Row,
    Source, State,
};
use nsh_core::{Checked, prelude::*};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

use nsh_app_devbase::testing::{
    EditorProvider, EditorSaveResult, SystemCacheClear, SystemCacheInfo, SystemCliChange,
    SystemCliStatus, SystemHelperChange, SystemHelperStatus, SystemLoginChange,
    SystemSettingsProvider, SystemSettingsSnapshot, install_editor_provider, install_provider,
    install_system_settings_provider, use_fake_databases, use_fake_dev_logs, use_fake_development,
    use_fake_editor, use_fake_guide, use_fake_menubar, use_fake_providers, use_fake_proxy,
    use_fake_software, use_fake_system, use_fake_updater,
};

/// The service tests share one fake-provider registry, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn install_all_fakes() {
    use_fake_providers();
    use_fake_software();
    use_fake_development();
    use_fake_databases();
    use_fake_proxy();
    use_fake_system();
    use_fake_guide();
    use_fake_menubar();
    use_fake_dev_logs();
    use_fake_editor();
    use_fake_updater();
}

fn open_services(app: &mut Headless) {
    app.wait_for("Services").unwrap();
    app.press("role:button[Services]").unwrap();
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
    install_all_fakes();
    install_provider(Source::UserAgent, user);
    install_provider(Source::SystemAgent, system);
    install_provider(Source::Daemon, daemon);
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    open_services(&mut app);
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
    install_all_fakes();
    install_provider(Source::UserAgent, user_agents(Behaviour::Normal));
    install_provider(Source::SystemAgent, system_agents());
    install_provider(Source::Daemon, daemons(Behaviour::Normal));
    install_system_settings_provider(FakeSettings::new(false));

    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    open_services(&mut app);
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

    // Autostart control uses nsh_base::switch with ARIA switch role, meaningful
    // label ("Load at login" / "Load at boot"), and checked state.
    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("id:autostart-toggle").unwrap();
    assert!(
        app.screen()
            .matches("id:inspector >> role:switch[Load at login]")
            .unwrap()
    );
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

    // Daemon autostart switch exposes "Load at boot" accessibility label.
    app.press("id:source-daemon").unwrap();
    app.wait_for("com.example.gamma").unwrap();
    app.press("id:item-com.example.gamma").unwrap();
    app.wait_for("id:autostart-toggle").unwrap();
    assert!(
        app.screen()
            .matches("id:inspector >> role:switch[Load at boot]")
            .unwrap()
    );
}

struct FakeRestrictedLoginItems {
    items: Vec<Item>,
    settings_opened: AtomicU64,
}

impl Provider for FakeRestrictedLoginItems {
    fn source(&self) -> Source {
        Source::LoginItem
    }

    fn list(&self) -> Result<Vec<Item>, BoxError> {
        Ok(self.items.clone())
    }

    fn detail(&self, _item: &Item) -> Result<Detail, BoxError> {
        Ok(Detail::default())
    }

    fn actions(&self, _item: &Item) -> Vec<Action> {
        vec![Action::Remove]
    }

    fn confirm(&self, _item: &Item, _action: Action) -> Option<String> {
        None
    }

    fn do_action(&self, _item: &Item, _action: Action) -> Result<(), BoxError> {
        Ok(())
    }

    fn as_restricted(&self) -> Option<&dyn Restricted> {
        Some(self)
    }
}

impl Restricted for FakeRestrictedLoginItems {
    fn restriction(&self) -> Option<Restriction> {
        Some(Restriction {
            text: "Helpers that apps registered are not listed: Full Disk Access required."
                .to_string(),
            settings: true,
        })
    }

    fn needs_access(&self) -> bool {
        true
    }

    fn open_settings(&self) -> Result<(), BoxError> {
        self.settings_opened.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn read_once(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

struct FakeEditor {
    saved_items: Arc<Mutex<Vec<Item>>>,
}

impl EditorProvider for FakeEditor {
    fn load(&self, path: &std::path::Path) -> Result<devcore::launchd::Spec, String> {
        Ok(devcore::launchd::Spec {
            label: path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "com.example.loaded".to_string()),
            args: vec!["/usr/bin/true".to_string()],
            ..devcore::launchd::Spec::default()
        })
    }

    fn read_raw(&self, _path: &std::path::Path) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    fn save(
        &self,
        source: Source,
        spec: devcore::launchd::Spec,
        _existing: Option<PathBuf>,
        _load: bool,
    ) -> Result<EditorSaveResult, String> {
        let path = PathBuf::from(format!("/tmp/devbase-tests/{}.plist", spec.label));
        let mut items = self
            .saved_items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !items.iter().any(|existing| existing.name == spec.label) {
            items.push(item(
                source,
                &spec.label,
                State::Loaded,
                Autostart::On,
                false,
            ));
        }
        Ok(EditorSaveResult {
            path,
            load_error: None,
        })
    }

    fn home_dir(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/tmp/devbase-tests"))
    }
}

#[test]
fn brew_login_item_restriction_and_plist_editor_wire_into_services() {
    let turn = serial();
    install_all_fakes();

    let brew = FakeProvider::new(
        Source::Brew,
        vec![item(
            Source::Brew,
            "redis",
            State::Running,
            Autostart::On,
            false,
        )],
        Behaviour::Normal,
    );
    let login = Arc::new(FakeRestrictedLoginItems {
        items: vec![item(
            Source::LoginItem,
            "Raycast",
            State::Running,
            Autostart::On,
            false,
        )],
        settings_opened: AtomicU64::new(0),
    });
    let user = user_agents(Behaviour::Normal);
    let saved_items = Arc::new(Mutex::new(
        user.items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
    ));
    let _ = saved_items;
    install_editor_provider(Arc::new(FakeEditor {
        saved_items: Arc::new(Mutex::new(Vec::new())),
    }));

    install_provider(Source::Brew, brew);
    install_provider(Source::LoginItem, login.clone());
    install_provider(Source::UserAgent, user);
    install_provider(Source::SystemAgent, system_agents());
    install_provider(Source::Daemon, daemons(Behaviour::Normal));

    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    open_services(&mut app);
    app.wait_for("com.example.alpha").unwrap();

    // New Job opens the plist editor overlay.
    app.press("id:new-job").unwrap();
    app.wait_for("id:plist-editor").unwrap();
    app.press("id:editor-cancel").unwrap();
    app.wait_for_gone("id:plist-editor", Duration::from_secs(2))
        .unwrap();

    // Selecting a launchd item exposes Edit, which opens the plist editor.
    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("id:edit-job").unwrap();
    app.press("id:edit-job").unwrap();
    app.wait_for("id:plist-editor").unwrap();
    app.press("id:editor-cancel").unwrap();
    app.wait_for_gone("id:plist-editor", Duration::from_secs(2))
        .unwrap();

    // Switching to Homebrew lists Brew services.
    app.press("id:source-brew").unwrap();
    app.wait_for("redis").unwrap();

    // Switching to Login Items shows the Add Login Item button, restriction note,
    // and Open Full Disk Access… button backed by the fake restricted provider.
    app.press("id:source-login-item").unwrap();
    app.wait_for("Raycast").unwrap();
    app.wait_for("id:add-login-item").unwrap();
    app.wait_for("id:login-item-restriction").unwrap();
    app.wait_for("id:open-full-disk-access").unwrap();
    app.press("id:open-full-disk-access").unwrap();
    app.wait(Duration::from_millis(200)).unwrap();
    assert_eq!(login.settings_opened.load(Ordering::Relaxed), 1);

    drop(app);
    drop(turn);
}

struct RefreshingDetailProvider {
    items: Mutex<Vec<Item>>,
    pid: AtomicU64,
}

impl Provider for RefreshingDetailProvider {
    fn source(&self) -> Source {
        Source::UserAgent
    }

    fn list(&self) -> Result<Vec<Item>, BoxError> {
        Ok(self
            .items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn detail(&self, item: &Item) -> Result<Detail, BoxError> {
        let pid = self.pid.load(Ordering::Relaxed);
        Ok(Detail {
            rows: vec![
                Row {
                    group: "Status".to_string(),
                    key: "Label".to_string(),
                    value: item.name.clone(),
                },
                Row {
                    group: "Status".to_string(),
                    key: "PID".to_string(),
                    value: pid.to_string(),
                },
                Row {
                    group: "Files".to_string(),
                    key: "Property list".to_string(),
                    value: item.path.display().to_string(),
                },
            ],
            logs: Vec::new(),
        })
    }

    fn actions(&self, _item: &Item) -> Vec<Action> {
        vec![Action::Stop, Action::Restart, Action::Disable]
    }

    fn confirm(&self, _item: &Item, _action: Action) -> Option<String> {
        None
    }

    fn do_action(&self, _item: &Item, _action: Action) -> Result<(), BoxError> {
        Ok(())
    }
}

#[test]
fn info_tab_scroll_node_id_stays_stable_across_detail_refreshes() {
    let turn = serial();
    install_all_fakes();

    let provider = Arc::new(RefreshingDetailProvider {
        items: Mutex::new(vec![item(
            Source::UserAgent,
            "com.example.alpha",
            State::Running,
            Autostart::On,
            false,
        )]),
        pid: AtomicU64::new(4242),
    });
    install_provider(Source::UserAgent, provider.clone());
    install_provider(Source::SystemAgent, system_agents());
    install_provider(Source::Daemon, daemons(Behaviour::Normal));

    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    open_services(&mut app);
    app.wait_for("com.example.alpha").unwrap();

    // No inspector is mounted before selection.
    assert!(!app.screen().matches("id:inspector").unwrap());

    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("id:info-scroll").unwrap();
    app.wait_for("4242").unwrap();

    let scroll_id_before = app.screen().id("id:info-scroll").unwrap();
    let inspector_id_before = app.screen().id("id:inspector").unwrap();

    // Simulate a PID change and trigger Refresh; the Info tab scroll container
    // must keep its NodeId so scroll offset and text selection survive.
    provider.pid.store(7788, Ordering::Relaxed);
    app.press("role:button[Refresh]").unwrap();
    app.wait_for("7788").unwrap();

    assert_eq!(
        app.screen().id("id:info-scroll").unwrap(),
        scroll_id_before,
        "info_tab scroll container must stay mounted across detail refreshes"
    );
    assert_eq!(
        app.screen().id("id:inspector").unwrap(),
        inspector_id_before
    );

    drop(app);
    drop(turn);
}

#[test]
fn services_keyboard_shortcuts_focus_search_clear_query_and_close_inspector() {
    let turn = serial();
    install_all_fakes();
    install_provider(Source::UserAgent, user_agents(Behaviour::Normal));
    install_provider(Source::SystemAgent, system_agents());
    install_provider(Source::Daemon, daemons(Behaviour::Normal));

    let mut config = EngineConfig::new(Root::of::<nsh_app_devbase::App>());
    config.environment.os = nsh_core::platform::Os::MacOs;
    let mut app = Headless::start(config).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    open_services(&mut app);
    app.wait_for("com.example.alpha").unwrap();
    app.wait_for("com.example.bravo").unwrap();

    // Cmd+F focuses the search input.
    app.press_key("mod+f").unwrap();
    let search_id = app.screen().id("id:services-search").unwrap();
    assert!(
        app.screen()
            .commands()
            .iter()
            .any(|(id, cmd)| *id == search_id && matches!(cmd, nsh_core::Command::Focus)),
        "mod+f should focus services-search"
    );

    // Filter down and open the inspector.
    app.type_text("id:services-search", "alpha").unwrap();
    app.wait_for_gone("com.example.bravo", Duration::from_secs(2))
        .unwrap();
    app.press("id:item-com.example.alpha").unwrap();
    app.wait_for("id:inspector").unwrap();

    // First Escape clears search while keeping the inspector open.
    app.press_key("escape").unwrap();
    app.wait_for("com.example.bravo").unwrap();
    assert!(app.screen().matches("id:inspector").unwrap());

    // Second Escape closes the inspector.
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:inspector", Duration::from_secs(2))
        .unwrap();

    drop(app);
    drop(turn);
}

#[test]
fn all_items_view_aggregates_sources_badges_counts_search_and_inspector() {
    let turn = serial();
    install_all_fakes();

    let brew = FakeProvider::new(
        Source::Brew,
        vec![item(
            Source::Brew,
            "redis",
            State::Running,
            Autostart::On,
            false,
        )],
        Behaviour::Normal,
    );
    let login = FakeProvider::new(
        Source::LoginItem,
        vec![item(
            Source::LoginItem,
            "Raycast",
            State::Running,
            Autostart::On,
            false,
        )],
        Behaviour::Normal,
    );
    install_editor_provider(Arc::new(FakeEditor {
        saved_items: Arc::new(Mutex::new(Vec::new())),
    }));
    install_provider(Source::Brew, brew);
    install_provider(Source::LoginItem, login);
    install_provider(Source::UserAgent, user_agents(Behaviour::Normal));
    install_provider(Source::SystemAgent, system_agents());
    install_provider(Source::Daemon, daemons(Behaviour::Normal));

    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    open_services(&mut app);
    app.wait_for("com.example.alpha").unwrap();

    // Single-source view does not render per-row source badges.
    assert!(!app.screen().matches("id:source-badge-user-agent").unwrap());

    // Switching to All Items loads and combines all 5 sources.
    app.press("id:source-all").unwrap();
    app.wait_for("redis").unwrap();
    app.wait_for("Raycast").unwrap();
    app.wait_for("com.example.alpha").unwrap();
    app.wait_for("com.example.beta").unwrap();
    app.wait_for("com.example.gamma").unwrap();

    // Total running/total count appears on the All Items rail row (4 running of 7 total).
    app.wait_for("id:source-all >> text:4/7").unwrap();

    // Source pill badges identify each item's origin.
    assert!(app.screen().matches("id:source-badge-brew").unwrap());
    assert!(app.screen().matches("id:source-badge-login-item").unwrap());
    assert!(app.screen().matches("id:source-badge-user-agent").unwrap());
    assert!(
        app.screen()
            .matches("id:source-badge-system-agent")
            .unwrap()
    );
    assert!(app.screen().matches("id:source-badge-daemon").unwrap());
    assert!(app.screen().shows("Homebrew"));
    assert!(app.screen().shows("Login Item"));
    assert!(app.screen().shows("User Agent"));
    assert!(app.screen().shows("System Agent"));
    assert!(app.screen().shows("Daemon"));

    // Cross-source search filters across all combined sources.
    app.type_text("id:services-search", "gamma").unwrap();
    app.wait_for_gone("redis", Duration::from_secs(2)).unwrap();
    assert!(app.screen().shows("com.example.gamma"));
    assert!(!app.screen().shows("com.example.alpha"));

    app.clear("id:services-search").unwrap();
    app.wait_for("redis").unwrap();
    app.wait_for("com.example.beta").unwrap();

    // Running/Stopped filter works across the aggregated list.
    app.press("id:filter-running").unwrap();
    app.wait_for_gone("com.example.beta", Duration::from_secs(2))
        .unwrap();
    assert!(app.screen().shows("redis"));
    assert!(app.screen().shows("Raycast"));
    assert!(app.screen().shows("com.example.alpha"));
    assert!(app.screen().shows("com.example.gamma"));
    app.press("id:filter-all").unwrap();
    app.wait_for("com.example.beta").unwrap();

    // Selecting a Daemon in All Items opens its normal Inspector, Edit Plist, and boot switch.
    app.press("id:item-com.example.gamma").unwrap();
    app.wait_for("id:inspector").unwrap();
    app.wait_for("id:edit-job").unwrap();
    assert!(
        app.screen()
            .matches("id:inspector >> role:switch[Load at boot]")
            .unwrap()
    );
    app.press("id:edit-job").unwrap();
    app.wait_for("id:plist-editor").unwrap();
    app.press("id:editor-cancel").unwrap();
    app.wait_for_gone("id:plist-editor", Duration::from_secs(2))
        .unwrap();

    // Selecting a Homebrew item in All Items opens its Inspector without Edit Plist.
    app.press("id:item-redis").unwrap();
    app.wait_for("/tmp/devbase-tests/redis.plist").unwrap();
    assert!(!app.screen().matches("id:edit-job").unwrap());
    assert!(
        app.screen()
            .matches("id:inspector >> role:button[Stop]")
            .unwrap()
    );

    drop(app);
    drop(turn);
}

#[test]
fn login_items_restriction_opens_full_disk_access_guide_on_demand() {
    let turn = serial();
    install_all_fakes();

    let login = Arc::new(FakeRestrictedLoginItems {
        items: vec![item(
            Source::LoginItem,
            "Raycast",
            State::Running,
            Autostart::On,
            false,
        )],
        settings_opened: AtomicU64::new(0),
    });
    install_provider(Source::LoginItem, login);
    install_provider(Source::UserAgent, user_agents(Behaviour::Normal));
    install_provider(Source::SystemAgent, system_agents());
    install_provider(Source::Daemon, daemons(Behaviour::Normal));

    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    open_services(&mut app);
    app.wait_for("com.example.alpha").unwrap();

    // Guide is closed initially (fake guide defaults to already welcomed).
    assert_eq!(app.screen().count("id:guide-overlay").unwrap(), 0);

    // Switch to Login Items and press "Full Disk Access Guide…".
    app.press("id:source-login-item").unwrap();
    app.wait_for("Raycast").unwrap();
    app.wait_for("id:login-items-fda-guide").unwrap();
    assert!(app.screen().shows("Full Disk Access Guide…"));

    app.press("id:login-items-fda-guide").unwrap();
    app.wait_for("id:guide-overlay").unwrap();
    app.wait_for("Welcome to Devbase").unwrap();

    // Closing the guide dismisses the overlay, and pressing the button reopens it on demand.
    app.press("id:guide-skip").unwrap();
    app.wait_for_gone("id:guide-overlay", Duration::from_secs(2))
        .unwrap();

    app.press("id:login-items-fda-guide").unwrap();
    app.wait_for("id:guide-overlay").unwrap();
    app.wait_for("Welcome to Devbase").unwrap();

    drop(app);
    drop(turn);
}
