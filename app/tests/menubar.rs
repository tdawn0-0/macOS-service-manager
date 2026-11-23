//! The menu bar panel and the first-run guide, headless. Fakes stand in for
//! the database, proxy and settings providers and for the guide's file and
//! TCC probe: no test runs launchd, reaches the network or reads a real file.
//!
//! The tray itself is the platform's (NSMenu); the rows it shows are the
//! panel model, which the test harness renders as text and whose actions it
//! dispatches exactly as the menu items do.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::Utc;
use devcore::{
    databases::{
        BackupFile, ConnectionInfo, DatabaseWorkInfo, InstanceAction, InstanceCatalog,
        InstanceDeletionInfo, InstanceDeletionPreview, InstanceInitializationInfo, PortInspection,
    },
    model::{ConnectionProfile, Instance, ProxyRule},
    service::{
        Action as ServiceAction, Autostart, Detail as ServiceDetail, Item as ServiceItem,
        Provider as ServiceProvider, Result as ServiceResult, State as ServiceState,
    },
    shutdown::{ShutdownPlan, ShutdownResource},
};
use nsh_app_devbase::dev_logs;
use nsh_app_devbase::testing::{
    AuthorityInfo, AuthorityStatus, CancelToken, DatabasesProvider, DatabasesSnapshot, GuideAccess,
    GuidePersistence, MENUBAR_GLOBAL_SHORTCUT, MENUBAR_GLOBAL_SHORTCUT_LABEL, MenubarAction,
    MenubarInput, MenubarInstance, MenubarMenuEntry, MenubarPanel, MenubarRowKind, MenubarRule,
    MenubarService, ProxyProvider, ProxySnapshot, ProxyWorkers, Source, SystemSettingsProvider,
    SystemSettingsSnapshot, clear_menubar_url_opener, install_databases_provider,
    install_guide_access, install_guide_persistence, install_menubar_url_opener, install_provider,
    install_proxy_provider, install_system_settings_provider, menubar_anchor_position_in,
    menubar_menu_entries, menubar_should_reopen_after_blur, menubar_title_runs, refresh_services,
    use_fake_databases, use_fake_dev_logs, use_fake_development, use_fake_guide, use_fake_menubar,
    use_fake_providers, use_fake_proxy, use_fake_software, use_fake_system,
};
#[cfg(target_os = "macos")]
use nsh_core::platform::Os;
use nsh_core::{
    desktop::{Display, DisplayId, Shortcut},
    geometry::{Bounds, point, size},
    prelude::*,
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

/// The fake-provider registries and the surface's globals are shared by the
/// tests of this binary, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- The test root -----------------------------------------------------------

/// A root that renders the tray panel's model as text and gives each enabled
/// row a button that runs its action. It calls the same functions the tray's
/// menu items call.
#[component]
fn TrayHarness() -> impl IntoElement {
    nsh_app_devbase::testing::menubar_load();
    view()
        .test_id("menubar-harness")
        .size_full()
        .flex_col()
        .child(dynamic(|| {
            let panel = nsh_app_devbase::testing::menubar_panel();
            let mut rows = view().flex_col().gap(px(1.0));
            rows = rows.child(text(format!("{} running", panel.running)).test_id("row-running"));
            for row in panel.rows {
                let checked = match row.checked {
                    Some(true) => "checked",
                    Some(false) => "unchecked",
                    None => "",
                };
                rows = rows.child(
                    text(format!(
                        "{} | {} | {} | {} | {checked}",
                        row.kind.tag(),
                        row.label,
                        row.detail,
                        row.state
                    ))
                    .test_id(&format!("row-{}", row.key)),
                );
                if row.enabled
                    && let Some(action) = row.action.clone()
                {
                    rows = rows.child(
                        nsh_base::button()
                            .test_id(&format!("act-{}", row.key))
                            .on_press(move || {
                                nsh_app_devbase::testing::menubar_dispatch(action.clone())
                            })
                            .child(text("run")),
                    );
                }
            }
            Some(rows.into_element())
        }))
        .child(when(nsh_app_devbase::testing::quit_open, || {
            text("quit pending").test_id("quit-open")
        }))
}

/// A root that starts the guide and shows its overlay, as the app root does.
#[component]
fn GuideHarness() -> impl IntoElement {
    nsh_app_devbase::testing::guide_start();
    view()
        .relative()
        .size_full()
        .child(text("guide harness ready").test_id("guide-ready"))
        .child(when(
            nsh_app_devbase::testing::guide_visible,
            nsh_app_devbase::testing::guide_overlay,
        ))
}

// --- Fakes -------------------------------------------------------------------

#[derive(Clone)]
struct DatabaseFixture {
    instances: Vec<Instance>,
    statuses: BTreeMap<String, String>,
}

struct FakeDatabases {
    fixture: Mutex<DatabaseFixture>,
    calls: Mutex<Vec<String>>,
}

impl FakeDatabases {
    fn new(fixture: DatabaseFixture) -> Arc<Self> {
        Arc::new(FakeDatabases {
            fixture: Mutex::new(fixture),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn record(&self, call: String) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(call);
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl DatabasesProvider for FakeDatabases {
    fn snapshot(&self) -> Result<DatabasesSnapshot, String> {
        let fixture = self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(DatabasesSnapshot {
            instances: fixture.instances.clone(),
            statuses: fixture.statuses.clone(),
            ..DatabasesSnapshot::default()
        })
    }

    fn next_port(&self) -> Result<i64, String> {
        Ok(55432)
    }

    fn inspect_port(&self, _port: i64) -> Result<PortInspection, String> {
        Err("not used by the menu bar".to_string())
    }

    fn create(&self, _instance: Instance, _cancel: CancelToken) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn action(&self, id: &str, action: InstanceAction, _cancel: CancelToken) -> Result<(), String> {
        self.record(format!("{} {id}", action.as_str()));
        let status = match action {
            InstanceAction::Start | InstanceAction::Restart => "Running",
            InstanceAction::Stop => "Stopped",
            InstanceAction::Remove => "Stopped",
        };
        let mut fixture = self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        fixture.statuses.insert(id.to_string(), status.to_string());
        Ok(())
    }

    fn catalog(&self, _id: &str, _cancel: CancelToken) -> Result<InstanceCatalog, String> {
        Err("not used by the menu bar".to_string())
    }

    fn create_database(&self, _id: &str, _name: &str, _cancel: CancelToken) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn create_user(
        &self,
        _id: &str,
        _user: &str,
        _password: &str,
        _database: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn drop_database(&self, _id: &str, _name: &str, _cancel: CancelToken) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn drop_user(
        &self,
        _id: &str,
        _user: &str,
        _database: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn backups(&self, _id: &str, _cancel: CancelToken) -> Result<Vec<BackupFile>, String> {
        Ok(Vec::new())
    }

    fn backup(&self, _id: &str, _database: &str, _cancel: CancelToken) -> Result<String, String> {
        Err("not used by the menu bar".to_string())
    }

    fn restore(
        &self,
        _id: &str,
        _database: &str,
        _path: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn delete_backup(&self, _id: &str, _name: &str, _cancel: CancelToken) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn save_profile(&self, profile: ConnectionProfile) -> Result<ConnectionProfile, String> {
        Ok(profile)
    }

    fn delete_profile(&self, _instance_id: &str, _profile_id: &str) -> Result<(), String> {
        Ok(())
    }

    fn connection_info(
        &self,
        _instance_id: &str,
        _profile_id: &str,
    ) -> Result<ConnectionInfo, String> {
        Err("not used by the menu bar".to_string())
    }

    fn connection_details(&self, _id: &str) -> Result<String, String> {
        Err("not used by the menu bar".to_string())
    }

    fn password(&self, _id: &str) -> Result<String, String> {
        Err("not used by the menu bar".to_string())
    }

    fn open_client(&self, _uri: &str) -> Result<(), String> {
        Ok(())
    }

    fn open_cli(
        &self,
        _id: &str,
        _profile_id: &str,
        _database: Option<&str>,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Ok(())
    }

    fn upgrade(
        &self,
        _instance: &Instance,
        _target: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn preview_deletion(&self, _id: &str) -> Result<InstanceDeletionPreview, String> {
        Err("not used by the menu bar".to_string())
    }

    fn delete_instance_data(
        &self,
        _id: &str,
        _fingerprint: &str,
        _confirmation: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn pending_deletions(&self) -> Result<Vec<InstanceDeletionInfo>, String> {
        Ok(Vec::new())
    }

    fn restore_deletion(
        &self,
        _id: &str,
        _revision: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn finish_deletion(
        &self,
        _id: &str,
        _revision: &str,
        _confirmation: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }

    fn pending_initializations(&self) -> Result<Vec<InstanceInitializationInfo>, String> {
        Ok(Vec::new())
    }

    fn resolve_initialization(
        &self,
        _id: &str,
        _revision: &str,
        _cancel: CancelToken,
    ) -> Result<String, String> {
        Err("not used by the menu bar".to_string())
    }

    fn pending_work(&self) -> Result<Vec<DatabaseWorkInfo>, String> {
        Ok(Vec::new())
    }

    fn resolve_work(&self, _id: &str, _revision: &str) -> Result<(), String> {
        Err("not used by the menu bar".to_string())
    }
}

struct FakeProxy {
    snapshot: Mutex<ProxySnapshot>,
    calls: Mutex<Vec<String>>,
}

impl FakeProxy {
    fn new(snapshot: ProxySnapshot) -> Arc<Self> {
        Arc::new(FakeProxy {
            snapshot: Mutex::new(snapshot),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn record(&self, call: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(call.to_string());
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_status(&self, key: &str, status: &str) {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .statuses
            .insert(key.to_string(), status.to_string());
    }
}

fn authority() -> AuthorityStatus {
    AuthorityStatus {
        checked_at: Utc::now(),
        active: AuthorityInfo::default(),
        prepared: None,
        previous: None,
        archives: Vec::new(),
        prepared_is_active: false,
        revision: String::new(),
    }
}

impl ProxyProvider for FakeProxy {
    fn snapshot(&self) -> Result<ProxySnapshot, String> {
        Ok(self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn listening_ports(&self) -> Result<Vec<nsh_app_devbase::testing::ProxyListeningPort>, String> {
        Ok(Vec::new())
    }

    fn save_rules(
        &self,
        _expected_revision: u64,
        _listener_port: i64,
        _rules: Vec<ProxyRule>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn apply_proxy(&self) -> Result<(), String> {
        self.record("apply proxy");
        self.set_status("proxy", "Running");
        Ok(())
    }

    fn stop_proxy(&self) -> Result<(), String> {
        self.record("stop proxy");
        self.set_status("proxy", "Stopped");
        Ok(())
    }

    fn restart_proxy(&self) -> Result<(), String> {
        self.record("restart proxy");
        Ok(())
    }

    fn proxy_result(
        &self,
        _domain: &str,
    ) -> Result<nsh_app_devbase::testing::ProxyHttpResult, String> {
        Err("not used by the menu bar".to_string())
    }

    fn tunnel_result(
        &self,
        _domain: &str,
    ) -> Result<nsh_app_devbase::testing::ProxyHttpResult, String> {
        Err("not used by the menu bar".to_string())
    }

    fn authority_status(&self) -> Result<AuthorityStatus, String> {
        Ok(authority())
    }

    fn authority_action(
        &self,
        _action: nsh_app_devbase::testing::AuthorityAction,
    ) -> Result<(), String> {
        Ok(())
    }

    fn save_cloudflare(
        &self,
        _token_path: Option<&Path>,
        account_id: &str,
        client_id: &str,
    ) -> Result<nsh_app_devbase::testing::ProxySavedCloudflare, String> {
        let _ = client_id;
        Ok(nsh_app_devbase::testing::ProxySavedCloudflare {
            token_configured: true,
            account_id: account_id.to_string(),
        })
    }

    fn refresh_cloudflare(&self) -> Result<(), String> {
        Ok(())
    }

    fn deploy_workers(&self) -> Result<(), String> {
        Ok(())
    }

    fn start_tunnel(&self) -> Result<(), String> {
        self.record("start tunnel");
        self.set_status("tunnel", "Running");
        Ok(())
    }

    fn stop_tunnel(&self) -> Result<(), String> {
        self.record("stop tunnel");
        self.set_status("tunnel", "Stopped");
        Ok(())
    }

    fn restart_tunnel(&self) -> Result<(), String> {
        self.record("restart tunnel");
        Ok(())
    }

    fn remove_cloudflare(&self, _delete_workers: bool) -> Result<(), String> {
        Ok(())
    }

    fn ensure_tunnel_token(&self, _domain: &str) -> Result<String, String> {
        Ok("token".to_string())
    }

    fn rotate_tunnel_token(&self, _domain: &str) -> Result<(), String> {
        Ok(())
    }

    fn logs(&self, _job: &str) -> Result<String, String> {
        Ok(String::new())
    }
}

struct FakeSettings {
    snapshot: Mutex<SystemSettingsSnapshot>,
    calls: Mutex<Vec<String>>,
}

impl FakeSettings {
    fn new(snapshot: SystemSettingsSnapshot) -> Arc<Self> {
        Arc::new(FakeSettings {
            snapshot: Mutex::new(snapshot),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn record(&self, call: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(call.to_string());
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
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

    fn install_helper(&self) -> nsh_app_devbase::testing::SystemHelperChange {
        nsh_app_devbase::testing::SystemHelperChange::default()
    }

    fn uninstall_helper(&self) -> nsh_app_devbase::testing::SystemHelperChange {
        nsh_app_devbase::testing::SystemHelperChange::default()
    }

    fn install_cli(&self) -> nsh_app_devbase::testing::SystemCliChange {
        nsh_app_devbase::testing::SystemCliChange::default()
    }

    fn uninstall_cli(&self) -> nsh_app_devbase::testing::SystemCliChange {
        nsh_app_devbase::testing::SystemCliChange::default()
    }

    fn clear_cache(&self) -> Result<nsh_app_devbase::testing::SystemCacheClear, String> {
        Err("not used by the menu bar".to_string())
    }

    fn set_log_rotation(&self, _enabled: bool) -> Result<(), String> {
        Ok(())
    }

    fn set_login_item(&self, enabled: bool) -> nsh_app_devbase::testing::SystemLoginChange {
        nsh_app_devbase::testing::SystemLoginChange {
            enabled,
            error: None,
        }
    }

    fn open_full_disk_access(&self) -> Result<(), String> {
        self.record("open full disk access");
        Ok(())
    }

    fn open_login_items(&self) -> Result<(), String> {
        Ok(())
    }

    fn reveal(&self, _path: &str) -> Result<(), String> {
        Ok(())
    }

    fn preview_shutdown(&self) -> Result<ShutdownPlan, String> {
        self.record("preview shutdown");
        Ok(ShutdownPlan {
            instances: vec![ShutdownResource {
                id: "app".to_string(),
                name: "app".to_string(),
            }],
            fingerprint: "fingerprint".to_string(),
        })
    }

    fn shutdown(&self, _plan: &ShutdownPlan) -> Result<(), String> {
        self.record("shutdown");
        Ok(())
    }
}

// --- Fixtures ----------------------------------------------------------------

fn instance(id: &str, kind: &str, port: i64) -> Instance {
    Instance {
        id: id.to_string(),
        kind: kind.to_string(),
        package: format!("{kind}@17.11-arm64"),
        port,
        persistent: true,
        ..Instance::default()
    }
}

fn rule(domain: &str, tunnel: bool) -> ProxyRule {
    ProxyRule {
        domain: domain.to_string(),
        local_port: 8080,
        tunnel,
        ..ProxyRule::default()
    }
}

fn proxy_snapshot(rules: Vec<ProxyRule>, proxy_status: &str) -> ProxySnapshot {
    let mut statuses = BTreeMap::new();
    statuses.insert("proxy".to_string(), proxy_status.to_string());
    ProxySnapshot {
        rules,
        listener_port: 8443,
        revision: 1,
        publication: "Published".to_string(),
        statuses,
        token_configured: false,
        workers: ProxyWorkers::default(),
        ..ProxySnapshot::default()
    }
}

fn settings_snapshot() -> SystemSettingsSnapshot {
    SystemSettingsSnapshot {
        helper: nsh_app_devbase::testing::SystemHelperStatus {
            installed: true,
            message: "Installed".to_string(),
        },
        workspace_root: "/tmp/devbase-tests/Development".to_string(),
        ..SystemSettingsSnapshot::default()
    }
}

fn database_fixture(statuses: &[(&str, &str)]) -> DatabaseFixture {
    let mut map = BTreeMap::new();
    for (id, status) in statuses {
        map.insert((*id).to_string(), (*status).to_string());
    }
    DatabaseFixture {
        instances: vec![
            instance("app", "postgresql", 5432),
            instance("cache", "redis", 6379),
        ],
        statuses: map,
    }
}

fn start_tray(
    databases: Arc<FakeDatabases>,
    proxy: Arc<FakeProxy>,
    settings: Arc<FakeSettings>,
) -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    use_fake_providers();
    use_fake_databases();
    install_databases_provider(databases as Arc<dyn DatabasesProvider>);
    use_fake_proxy();
    install_proxy_provider(proxy as Arc<dyn ProxyProvider>);
    use_fake_system();
    install_system_settings_provider(settings as Arc<dyn SystemSettingsProvider>);
    let mut app = Headless::start(EngineConfig::new(Root::of::<TrayHarness>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("id:menubar-harness").unwrap();
    (turn, app)
}

/// Whether any text on screen contains `needle`.
fn contains(app: &Headless, needle: &str) -> bool {
    app.screen()
        .texts()
        .iter()
        .any(|text| text.contains(needle))
}

/// Waits for a text node that contains `needle`.
fn wait_contains(app: &mut Headless, needle: &str, timeout: Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if contains(app, needle) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "`{needle}` never appeared; on screen: {:?}",
                app.screen().texts()
            ));
        }
        app.wait(Duration::from_millis(50))?;
    }
}

// --- The panel ---------------------------------------------------------------

#[test]
fn rows_render_instances_switches_status_and_no_tunnel_until_configured() {
    let databases = FakeDatabases::new(database_fixture(&[
        ("app", "Running"),
        ("cache", "Stopped"),
    ]));
    let proxy = FakeProxy::new(proxy_snapshot(
        vec![rule("app.test.devbase.localhost", false)],
        "Stopped",
    ));
    let settings = FakeSettings::new(settings_snapshot());
    let (_turn, mut app) = start_tray(databases, proxy, settings);

    app.wait_for("id:row-instance:app").unwrap();
    app.wait_for("id:row-instance:cache").unwrap();
    app.wait_for("id:row-proxy").unwrap();

    assert!(contains(&app, "PostgreSQL · :5432 · Running"));
    assert!(contains(&app, "instance | app |"));
    assert!(contains(&app, "Redis · :6379 · Stopped"));
    assert!(contains(&app, "Stopped"));
    assert!(contains(&app, "Proxy · Stopped"));
    assert!(contains(&app, "1 rule · :8443"));
    assert!(contains(&app, "/tmp/devbase-tests/Development"));
    assert!(contains(&app, "Helper | Installed"));
    assert_eq!(app.screen().count("id:row-tunnel").unwrap(), 0);
    assert!(contains(&app, "Open Devbase"));
    assert!(contains(&app, "Quit Devbase"));
}

#[test]
fn the_proxy_switch_applies_and_the_instance_switch_starts_and_stops() {
    let databases = FakeDatabases::new(database_fixture(&[("app", "Stopped")]));
    let proxy = FakeProxy::new(proxy_snapshot(
        vec![rule("app.test.devbase.localhost", false)],
        "Stopped",
    ));
    let settings = FakeSettings::new(settings_snapshot());
    let (turn, mut app) = start_tray(databases.clone(), proxy.clone(), settings);

    app.wait_for("id:act-proxy").unwrap();
    app.press("id:act-proxy").unwrap();
    app.wait(Duration::from_millis(300)).unwrap();
    assert!(
        proxy.calls().iter().any(|call| call == "apply proxy"),
        "calls: {:?}",
        proxy.calls()
    );
    wait_contains(&mut app, "Proxy · Running", Duration::from_secs(5)).unwrap();
    assert!(proxy.calls().iter().any(|call| call == "apply proxy"));

    app.wait_for("id:act-instance:app:toggle").unwrap();
    app.press("id:act-instance:app:toggle").unwrap();
    wait_contains(&mut app, "Stop app", Duration::from_secs(5)).unwrap();
    assert!(databases.calls().iter().any(|call| call == "start app"));

    app.press("id:act-instance:app:toggle").unwrap();
    wait_contains(&mut app, "Start app", Duration::from_secs(5)).unwrap();
    assert!(databases.calls().iter().any(|call| call == "stop app"));
    drop(turn);
}

#[test]
fn the_tunnel_switch_appears_once_a_tunnel_is_configured() {
    let databases = FakeDatabases::new(database_fixture(&[("app", "Running")]));
    let mut snapshot = proxy_snapshot(vec![rule("app.test.devbase.localhost", true)], "Running");
    snapshot.token_configured = true;
    snapshot
        .workers
        .insert("app-test", "https://app-test.example.workers.dev");
    snapshot
        .statuses
        .insert("tunnel".to_string(), "Running".to_string());
    snapshot.statuses.insert(
        "tunnel:detail".to_string(),
        "tunnel.example.com".to_string(),
    );
    let proxy = FakeProxy::new(snapshot);
    let settings = FakeSettings::new(settings_snapshot());
    let (_turn, mut app) = start_tray(databases, proxy.clone(), settings);

    app.wait_for("id:row-tunnel").unwrap();
    assert!(contains(&app, "Tunnel · Running"));
    assert!(contains(&app, "tunnel.example.com"));

    app.press("id:act-tunnel").unwrap();
    wait_contains(&mut app, "Tunnel · Stopped", Duration::from_secs(5)).unwrap();
    assert!(proxy.calls().iter().any(|call| call == "stop tunnel"));
}

#[test]
fn the_quit_entry_runs_the_quit_flow() {
    let databases = FakeDatabases::new(database_fixture(&[("app", "Running")]));
    let proxy = FakeProxy::new(proxy_snapshot(
        vec![rule("app.test.devbase.localhost", false)],
        "Stopped",
    ));
    let settings = FakeSettings::new(settings_snapshot());
    let (_turn, mut app) = start_tray(databases, proxy, settings.clone());

    app.wait_for("id:act-quit").unwrap();
    app.press("id:act-quit").unwrap();
    app.wait_for("id:quit-open").unwrap();
    assert!(
        settings
            .calls()
            .iter()
            .any(|call| call == "preview shutdown")
    );
}

// --- The guide ---------------------------------------------------------------

struct FakePersistence {
    completed: AtomicBool,
    completions: AtomicUsize,
}

impl FakePersistence {
    fn new(completed: bool) -> Arc<Self> {
        Arc::new(FakePersistence {
            completed: AtomicBool::new(completed),
            completions: AtomicUsize::new(0),
        })
    }

    fn completions(&self) -> usize {
        self.completions.load(Ordering::Relaxed)
    }
}

impl GuidePersistence for FakePersistence {
    fn completed(&self) -> Result<bool, String> {
        Ok(self.completed.load(Ordering::Relaxed))
    }

    fn complete(&self) -> Result<(), String> {
        self.completed.store(true, Ordering::Relaxed);
        self.completions.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct FakeAccess {
    needs: AtomicBool,
}

impl FakeAccess {
    fn new(needs: bool) -> Arc<Self> {
        Arc::new(FakeAccess {
            needs: AtomicBool::new(needs),
        })
    }
}

impl GuideAccess for FakeAccess {
    fn needs_access(&self) -> bool {
        self.needs.load(Ordering::Relaxed)
    }
}

fn start_guide(
    persistence: Arc<FakePersistence>,
    access: Arc<FakeAccess>,
) -> (MutexGuard<'static, ()>, Headless, Arc<FakeSettings>) {
    let turn = serial();
    let settings = FakeSettings::new(settings_snapshot());
    use_fake_system();
    install_system_settings_provider(settings.clone() as Arc<dyn SystemSettingsProvider>);
    use_fake_guide();
    install_guide_persistence(persistence as Arc<dyn GuidePersistence>);
    install_guide_access(access as Arc<dyn GuideAccess>);
    let mut app = Headless::start(EngineConfig::new(Root::of::<GuideHarness>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("id:guide-ready").unwrap();
    (turn, app, settings)
}

#[test]
fn the_first_run_guide_leads_through_its_pages_and_remembers_completion() {
    let persistence = FakePersistence::new(false);
    let access = FakeAccess::new(true);
    let (_turn, mut app, settings) = start_guide(persistence.clone(), access.clone());

    app.wait_for("id:guide-overlay").unwrap();
    app.wait_for("Welcome to Devbase").unwrap();
    assert!(!contains(
        &app,
        "Everything that starts on your Mac, in one place."
    ));
    assert!(contains(&app, "Full Disk Access"));
    assert!(contains(&app, "Only to read the list of login items."));
    assert!(contains(&app, "Skip"));
    assert!(!contains(
        &app,
        "Without it, some login items may not show."
    ));

    app.press("id:guide-allow").unwrap();
    app.wait_for("Add this app with the + button").unwrap();
    assert!(!contains(&app, "With the + button under the list."));
    app.wait_for("Waiting for access").unwrap();
    assert!(
        settings
            .calls()
            .iter()
            .any(|call| call == "open full disk access")
    );

    // macOS allows it while the guide waits: the done page shows, then closes.
    access.needs.store(false, Ordering::Relaxed);
    app.wait_for("Full Disk Access is on").unwrap();
    app.wait_for_gone("id:guide-overlay", Duration::from_secs(4))
        .unwrap();
    app.wait(Duration::from_millis(200)).unwrap();
    assert_eq!(persistence.completions(), 1);
    assert!(persistence.completed.load(Ordering::Relaxed));
}

#[component]
fn PopoverHarness() -> impl IntoElement {
    nsh_app_devbase::testing::menubar_load();
    view()
        .size_full()
        .flex_col()
        .child(when(nsh_app_devbase::testing::quit_open, || {
            text("quit pending").test_id("quit-open")
        }))
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .child(nsh_app_devbase::testing::menubar_popover()),
        )
}

#[test]
fn the_popover_ui_renders_status_and_toggles_switches() {
    let databases = FakeDatabases::new(database_fixture(&[
        ("app", "Stopped"),
        ("cache", "Running"),
    ]));
    let mut snapshot = proxy_snapshot(vec![rule("app.test.devbase.localhost", true)], "Stopped");
    snapshot.token_configured = true;
    snapshot
        .workers
        .insert("app-test", "https://app-test.example.workers.dev");
    snapshot
        .statuses
        .insert("tunnel".to_string(), "Stopped".to_string());
    let proxy = FakeProxy::new(snapshot);
    let settings = FakeSettings::new(settings_snapshot());

    let turn = serial();
    use_fake_providers();
    use_fake_databases();
    install_databases_provider(databases.clone() as Arc<dyn DatabasesProvider>);
    use_fake_proxy();
    install_proxy_provider(proxy.clone() as Arc<dyn ProxyProvider>);
    use_fake_system();
    install_system_settings_provider(settings.clone() as Arc<dyn SystemSettingsProvider>);

    let mut app = Headless::start(EngineConfig::new(Root::of::<PopoverHarness>())).unwrap();
    app.set_viewport(Size::new(px(340.0), px(520.0))).unwrap();
    app.wait_for("id:menubar-popover").unwrap();
    app.wait_for("id:popover-switch-instance-app").unwrap();
    app.wait_for("id:popover-switch-proxy").unwrap();
    app.wait_for("id:popover-switch-tunnel").unwrap();

    assert!(contains(&app, "1 running"));
    assert!(contains(&app, "PostgreSQL · :5432 · Stopped"));
    assert!(contains(&app, "Redis · :6379 · Running"));
    assert!(!contains(&app, "Proxy port"));
    assert!(!contains(&app, "/tmp/devbase-tests/Development"));

    // Toggle database instance switch on.
    app.press("id:popover-switch-instance-app").unwrap();
    wait_contains(&mut app, "2 running", Duration::from_secs(5)).unwrap();
    assert!(databases.calls().iter().any(|call| call == "start app"));

    // Toggle proxy switch on.
    app.press("id:popover-switch-proxy").unwrap();
    wait_contains(&mut app, "3 running", Duration::from_secs(5)).unwrap();
    assert!(proxy.calls().iter().any(|call| call == "apply proxy"));
    assert!(contains(&app, "Proxy"));
    assert!(!contains(&app, "Proxy · Running"));

    // Toggle tunnel switch on.
    app.press("id:popover-switch-tunnel").unwrap();
    wait_contains(&mut app, "4 running", Duration::from_secs(5)).unwrap();
    assert!(proxy.calls().iter().any(|call| call == "start tunnel"));
    assert!(contains(&app, "Tunnel"));
    assert!(!contains(&app, "Tunnel · Running"));

    // Footer Quit button triggers quit flow.
    app.press("id:popover-quit").unwrap();
    app.wait_for("id:quit-open").unwrap();
    assert!(
        settings
            .calls()
            .iter()
            .any(|call| call == "preview shutdown")
    );
    drop(turn);
}

#[test]
fn a_completed_guide_does_not_open_again() {
    let persistence = FakePersistence::new(true);
    let access = FakeAccess::new(true);
    let (_turn, mut app, _settings) = start_guide(persistence, access);

    app.wait_for("id:guide-ready").unwrap();
    app.wait(Duration::from_millis(500)).unwrap();
    assert_eq!(app.screen().count("id:guide-overlay").unwrap(), 0);
    assert!(!app.screen().shows("Allow Full Disk Access"));
}

#[test]
fn panel_includes_optional_services_and_excludes_internal_devbase_jobs() {
    let panel = MenubarPanel::from(MenubarInput {
        services: vec![
            MenubarService {
                id: "brew:redis".to_string(),
                source: Source::Brew,
                name: "redis".to_string(),
                status: "Running".to_string(),
                running: true,
                busy: false,
                can_toggle: true,
            },
            MenubarService {
                id: "user-agent:/Users/me/Library/LaunchAgents/xyz.jyeu.devbase.dev.proxy.plist"
                    .to_string(),
                source: Source::UserAgent,
                name: "xyz.jyeu.devbase.dev.proxy".to_string(),
                status: "Running".to_string(),
                running: true,
                busy: false,
                can_toggle: true,
            },
        ],
        proxy_status: "Stopped".to_string(),
        proxy_port: 8443,
        ..MenubarInput::default()
    });
    assert!(
        panel
            .rows
            .iter()
            .any(|row| row.kind == MenubarRowKind::Service && row.label == "redis")
    );
    assert!(
        !panel
            .rows
            .iter()
            .any(|row| row.label.contains("xyz.jyeu.devbase.dev."))
    );
}

// --- Native macOS tray & popover polish tests --------------------------------

#[test]
fn title_runs_renders_running_and_colored_failure_indicator() {
    let idle = MenubarPanel::from(MenubarInput {
        proxy_status: "Stopped".to_string(),
        proxy_port: 8443,
        ..MenubarInput::default()
    });
    assert!(menubar_title_runs(&idle).is_empty());

    let running_only = MenubarPanel::from(MenubarInput {
        instances: vec![
            MenubarInstance {
                id: "app".to_string(),
                title: "PostgreSQL".to_string(),
                port: 5432,
                status: "Running".to_string(),
            },
            MenubarInstance {
                id: "cache".to_string(),
                title: "Redis".to_string(),
                port: 6379,
                status: "Running".to_string(),
            },
        ],
        proxy_status: "Stopped".to_string(),
        proxy_port: 8443,
        ..MenubarInput::default()
    });
    let runs = menubar_title_runs(&running_only);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].text, "2");
    assert_eq!(runs[0].color, None);

    let running_and_failed = MenubarPanel::from(MenubarInput {
        instances: vec![
            MenubarInstance {
                id: "app".to_string(),
                title: "PostgreSQL".to_string(),
                port: 5432,
                status: "Running".to_string(),
            },
            MenubarInstance {
                id: "cache".to_string(),
                title: "Redis".to_string(),
                port: 6379,
                status: "Failed".to_string(),
            },
        ],
        proxy_status: "Running".to_string(),
        proxy_port: 8443,
        ..MenubarInput::default()
    });
    let runs = menubar_title_runs(&running_and_failed);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].text, "2");
    assert_eq!(runs[0].color, None);
    assert_eq!(runs[1].text, " · 1!");
    assert!(runs[1].color.is_some());

    let failed_only = MenubarPanel::from(MenubarInput {
        instances: vec![MenubarInstance {
            id: "app".to_string(),
            title: "PostgreSQL".to_string(),
            port: 5432,
            status: "Failed".to_string(),
        }],
        proxy_status: "Failed".to_string(),
        proxy_port: 8443,
        ..MenubarInput::default()
    });
    let runs = menubar_title_runs(&failed_only);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].text, "2!");
    assert!(runs[0].color.is_some());
}

#[test]
fn blur_reopen_debounce_suppresses_bounce_within_200ms() {
    let closed_at = Instant::now();
    assert!(menubar_should_reopen_after_blur(None, closed_at));
    assert!(!menubar_should_reopen_after_blur(
        Some(closed_at),
        closed_at
    ));
    assert!(!menubar_should_reopen_after_blur(
        Some(closed_at),
        closed_at + Duration::from_millis(50)
    ));
    assert!(!menubar_should_reopen_after_blur(
        Some(closed_at),
        closed_at + Duration::from_millis(199)
    ));
    assert!(menubar_should_reopen_after_blur(
        Some(closed_at),
        closed_at + Duration::from_millis(200)
    ));
    assert!(menubar_should_reopen_after_blur(
        Some(closed_at),
        closed_at + Duration::from_millis(500)
    ));
}

#[test]
fn anchor_position_falls_back_to_top_right_of_primary_display_and_shortcut_parses() {
    let shortcut = Shortcut::parse(MENUBAR_GLOBAL_SHORTCUT, cfg!(target_os = "macos")).unwrap();
    assert!(shortcut.ctrl && shortcut.alt && shortcut.meta);
    #[cfg(target_os = "macos")]
    assert_eq!(shortcut.label_for(Os::MacOs), MENUBAR_GLOBAL_SHORTCUT_LABEL);
    #[cfg(not(target_os = "macos"))]
    assert_eq!(MENUBAR_GLOBAL_SHORTCUT_LABEL, "Ctrl+Alt+Super+D");

    let secondary = Display::new(
        DisplayId(2),
        Bounds::new(point(px(-1920.0), px(0.0)), size(px(1920.0), px(1080.0))),
        Bounds::new(point(px(-1920.0), px(25.0)), size(px(1920.0), px(1055.0))),
        false,
    );
    let primary = Display::new(
        DisplayId(1),
        Bounds::new(point(px(0.0), px(0.0)), size(px(1512.0), px(982.0))),
        Bounds::new(point(px(0.0), px(38.0)), size(px(1512.0), px(874.0))),
        true,
    );
    let screens = vec![secondary, primary];

    // No tray click anchor (e.g. opened via global shortcut): top-right under
    // the menu bar, inset by POPOVER_MARGIN (8pt), POPOVER_WIDTH (340pt).
    let fallback = menubar_anchor_position_in(None, &screens).unwrap();
    assert_eq!(fallback, point(px(1512.0 - 340.0 - 8.0), px(38.0 + 8.0)));

    // Tray click near right edge clamps cleanly inside visible_bounds.
    let right_edge_tray = Bounds::new(point(px(1480.0), px(0.0)), size(px(28.0), px(24.0)));
    let clamped = menubar_anchor_position_in(Some(right_edge_tray), &screens).unwrap();
    assert_eq!(clamped, point(px(1512.0 - 340.0 - 8.0), px(30.0)));

    // No displays and no anchor yields None.
    assert_eq!(menubar_anchor_position_in(None, &[]), None);
}

fn assert_every_menu_entry_has_svg_icon(entries: &[MenubarMenuEntry]) {
    for entry in entries {
        match entry {
            MenubarMenuEntry::Separator => {}
            MenubarMenuEntry::Item { label, icon, .. } => {
                assert!(
                    !icon.is_empty() && icon.contains('<'),
                    "menu item `{label}` missing template SVG icon"
                );
            }
            MenubarMenuEntry::Submenu { label, icon, items } => {
                assert!(
                    !icon.is_empty() && icon.contains('<'),
                    "submenu `{label}` missing template SVG icon"
                );
                assert!(!items.is_empty(), "submenu `{label}` should not be empty");
                assert_every_menu_entry_has_svg_icon(items);
            }
        }
    }
}

#[test]
fn right_click_menu_has_explicit_macos_section_headers_and_svg_icons_on_every_item() {
    let panel = MenubarPanel::from(MenubarInput {
        instances: vec![
            MenubarInstance {
                id: "app".to_string(),
                title: "PostgreSQL".to_string(),
                port: 5432,
                status: "Running".to_string(),
            },
            MenubarInstance {
                id: "cache".to_string(),
                title: "Redis".to_string(),
                port: 6379,
                status: "Stopped".to_string(),
            },
        ],
        services: vec![
            MenubarService {
                id: "brew:redis".to_string(),
                source: Source::Brew,
                name: "redis".to_string(),
                status: "Running".to_string(),
                running: true,
                busy: false,
                can_toggle: true,
            },
            MenubarService {
                id: "user-agent:com.example.sync".to_string(),
                source: Source::UserAgent,
                name: "com.example.sync".to_string(),
                status: "Stopped".to_string(),
                running: false,
                busy: false,
                can_toggle: true,
            },
        ],
        proxy_status: "Running".to_string(),
        proxy_rules: 2,
        proxy_port: 8443,
        tunnel_status: "Stopped".to_string(),
        tunnel_configured: true,
        tunnel_rules: 1,
        tunnel_workers: 1,
        ..MenubarInput::default()
    });

    let entries = menubar_menu_entries(&panel);
    assert_every_menu_entry_has_svg_icon(&entries);

    let disabled_headers: Vec<&str> = entries
        .iter()
        .filter_map(|entry| match entry {
            MenubarMenuEntry::Item {
                label,
                enabled: false,
                action: None,
                ..
            } => Some(label.as_str()),
            _ => None,
        })
        .collect();
    assert!(disabled_headers.iter().any(|h| h.starts_with("Devbase · ")));
    assert!(disabled_headers.contains(&"Databases · 1/2"));
    assert!(disabled_headers.contains(&"HTTPS"));
    assert!(disabled_headers.contains(&"Homebrew · 1/1"));
    assert!(disabled_headers.contains(&"Login Agents · 0/1"));

    // Start and Stop toggle items both have distinct template SVG icons.
    let stop_icon = entries.iter().find_map(|entry| match entry {
        MenubarMenuEntry::Item { label, icon, .. } if label.starts_with("Stop app") => Some(*icon),
        _ => None,
    });
    let start_icon = entries.iter().find_map(|entry| match entry {
        MenubarMenuEntry::Item { label, icon, .. } if label.starts_with("Start cache") => {
            Some(*icon)
        }
        _ => None,
    });
    assert!(stop_icon.is_some() && start_icon.is_some());
    assert_ne!(stop_icon, start_icon);

    // Long service lists collapse into a clean non-inline submenu with an SVG icon.
    let many_brew: Vec<MenubarService> = (0..8)
        .map(|idx| MenubarService {
            id: format!("brew:svc-{idx}"),
            source: Source::Brew,
            name: format!("svc-{idx}"),
            status: if idx % 2 == 0 { "Running" } else { "Stopped" }.to_string(),
            running: idx % 2 == 0,
            busy: false,
            can_toggle: true,
        })
        .collect();
    let long_panel = MenubarPanel::from(MenubarInput {
        services: many_brew,
        proxy_status: "Stopped".to_string(),
        proxy_port: 8443,
        ..MenubarInput::default()
    });
    let long_entries = menubar_menu_entries(&long_panel);
    assert_every_menu_entry_has_svg_icon(&long_entries);
    let brew_submenu = long_entries.iter().find_map(|entry| match entry {
        MenubarMenuEntry::Submenu { label, items, .. } if label == "Homebrew · 4/8" => Some(items),
        _ => None,
    });
    assert_eq!(brew_submenu.map(Vec::len), Some(16));
}

struct FakeServiceProvider {
    source: Source,
    items: Mutex<Vec<ServiceItem>>,
    calls: Mutex<Vec<String>>,
}

impl FakeServiceProvider {
    fn new(source: Source, items: Vec<ServiceItem>) -> Arc<Self> {
        Arc::new(Self {
            source,
            items: Mutex::new(items),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ServiceProvider for FakeServiceProvider {
    fn source(&self) -> Source {
        self.source
    }

    fn list(&self) -> ServiceResult<Vec<ServiceItem>> {
        Ok(self
            .items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn detail(&self, _item: &ServiceItem) -> ServiceResult<ServiceDetail> {
        Ok(ServiceDetail::default())
    }

    fn actions(&self, item: &ServiceItem) -> Vec<ServiceAction> {
        if item.up() {
            vec![ServiceAction::Stop]
        } else {
            vec![ServiceAction::Start]
        }
    }

    fn confirm(&self, _item: &ServiceItem, _action: ServiceAction) -> Option<String> {
        None
    }

    fn do_action(&self, item: &ServiceItem, action: ServiceAction) -> ServiceResult<()> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(format!("{action:?} {}", item.name));
        let mut items = self
            .items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(found) = items.iter_mut().find(|existing| existing.id == item.id) {
            found.state = match action {
                ServiceAction::Start | ServiceAction::Load | ServiceAction::Restart => {
                    ServiceState::Running
                }
                ServiceAction::Stop | ServiceAction::Unload => ServiceState::Stopped,
                _ => found.state,
            };
        }
        Ok(())
    }
}

fn service_item(id: &str, source: Source, name: &str, state: ServiceState) -> ServiceItem {
    ServiceItem {
        id: id.to_string(),
        source,
        name: name.to_string(),
        state,
        pid: 0,
        autostart: Autostart::Off,
        owner: String::new(),
        path: PathBuf::new(),
        kind: None,
        admin: false,
        keep_alive: false,
    }
}

#[component]
fn PopoverWithServicesHarness() -> impl IntoElement {
    nsh_app_devbase::testing::menubar_load();
    refresh_services(Source::Brew);
    refresh_services(Source::UserAgent);
    view().size_full().flex_col().child(
        view()
            .flex_1()
            .min_h(px(0.0))
            .child(nsh_app_devbase::testing::menubar_popover()),
    )
}

#[test]
fn collapsible_service_sections_group_brew_and_login_agents_and_toggle_cleanly() {
    let databases = FakeDatabases::new(database_fixture(&[("app", "Running")]));
    let proxy = FakeProxy::new(proxy_snapshot(
        vec![rule("app.test.devbase.localhost", false)],
        "Stopped",
    ));
    let settings = FakeSettings::new(settings_snapshot());
    let brew = FakeServiceProvider::new(
        Source::Brew,
        vec![
            service_item("brew:redis", Source::Brew, "redis", ServiceState::Running),
            service_item("brew:nginx", Source::Brew, "nginx", ServiceState::Stopped),
        ],
    );
    let agents = FakeServiceProvider::new(
        Source::UserAgent,
        vec![
            service_item(
                "user-agent:com.example.sync",
                Source::UserAgent,
                "com.example.sync",
                ServiceState::Running,
            ),
            service_item(
                "user-agent:xyz.jyeu.devbase.dev.proxy",
                Source::UserAgent,
                "xyz.jyeu.devbase.dev.proxy",
                ServiceState::Running,
            ),
        ],
    );

    let turn = serial();
    use_fake_menubar();
    use_fake_providers();
    install_provider(Source::Brew, brew.clone() as Arc<dyn ServiceProvider>);
    install_provider(Source::UserAgent, agents as Arc<dyn ServiceProvider>);
    use_fake_databases();
    install_databases_provider(databases as Arc<dyn DatabasesProvider>);
    use_fake_proxy();
    install_proxy_provider(proxy as Arc<dyn ProxyProvider>);
    use_fake_system();
    install_system_settings_provider(settings as Arc<dyn SystemSettingsProvider>);

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<PopoverWithServicesHarness>())).unwrap();
    app.set_viewport(Size::new(px(340.0), px(560.0))).unwrap();
    app.wait_for("id:popover-section-brew-toggle").unwrap();
    app.wait_for("id:popover-section-user-agent-toggle")
        .unwrap();
    app.wait_for("id:popover-service-redis").unwrap();
    app.wait_for("id:popover-service-nginx").unwrap();
    app.wait_for("id:popover-service-com.example.sync").unwrap();

    assert!(contains(&app, "Homebrew"));
    assert!(contains(&app, "Login Agents"));
    assert!(contains(&app, "1/2"));
    assert!(contains(&app, "1/1"));
    assert!(!contains(&app, "xyz.jyeu.devbase.dev.proxy"));

    // Collapsing Homebrew hides its rows while keeping Login Agents expanded.
    app.press("id:popover-section-brew-toggle").unwrap();
    app.wait_for_gone("id:popover-service-redis", Duration::from_secs(2))
        .unwrap();
    assert_eq!(app.screen().count("id:popover-service-nginx").unwrap(), 0);
    assert_eq!(
        app.screen()
            .count("id:popover-service-com.example.sync")
            .unwrap(),
        1
    );

    // Expanding Homebrew brings its rows back and toggling a switch calls provider.
    app.press("id:popover-section-brew-toggle").unwrap();
    app.wait_for("id:popover-switch-service-nginx").unwrap();
    app.press("id:popover-switch-service-nginx").unwrap();
    wait_contains(&mut app, "2/2", Duration::from_secs(5)).unwrap();
    assert!(brew.calls().iter().any(|call| call == "Start nginx"));
    drop(turn);
}

#[test]
fn popover_scroll_body_and_rows_stay_stable_across_switch_toggles_and_refresh() {
    let databases = FakeDatabases::new(database_fixture(&[
        ("app", "Stopped"),
        ("cache", "Running"),
    ]));
    let proxy = FakeProxy::new(proxy_snapshot(
        vec![rule("app.test.devbase.localhost", false)],
        "Stopped",
    ));
    let settings = FakeSettings::new(settings_snapshot());

    let turn = serial();
    use_fake_menubar();
    use_fake_providers();
    use_fake_databases();
    install_databases_provider(databases.clone() as Arc<dyn DatabasesProvider>);
    use_fake_proxy();
    install_proxy_provider(proxy.clone() as Arc<dyn ProxyProvider>);
    use_fake_system();
    install_system_settings_provider(settings as Arc<dyn SystemSettingsProvider>);

    let mut app = Headless::start(EngineConfig::new(Root::of::<PopoverHarness>())).unwrap();
    app.set_viewport(Size::new(px(340.0), px(520.0))).unwrap();
    app.wait_for("id:menubar-popover-body").unwrap();
    app.wait_for("id:popover-instance-app").unwrap();
    app.wait_for("id:popover-instance-cache").unwrap();
    app.wait_for("id:popover-proxy").unwrap();
    app.wait_for("id:popover-shortcut-hint").unwrap();
    assert!(contains(&app, MENUBAR_GLOBAL_SHORTCUT_LABEL));

    let body_id = app.screen().id("id:menubar-popover-body").unwrap();
    let app_row_id = app.screen().id("id:popover-instance-app").unwrap();
    let cache_row_id = app.screen().id("id:popover-instance-cache").unwrap();
    let proxy_row_id = app.screen().id("id:popover-proxy").unwrap();

    // Toggle an instance switch, toggle proxy, and trigger Refresh.
    app.press("id:popover-switch-instance-app").unwrap();
    wait_contains(&mut app, "2 running", Duration::from_secs(5)).unwrap();

    app.press("id:popover-switch-proxy").unwrap();
    wait_contains(&mut app, "3 running", Duration::from_secs(5)).unwrap();

    app.press("id:popover-refresh").unwrap();
    app.wait(Duration::from_millis(200)).unwrap();

    // Fine-grained reactivity: neither the scroll container nor keyed rows
    // were torn down or recreated.
    assert_eq!(app.screen().id("id:menubar-popover-body").unwrap(), body_id);
    assert_eq!(
        app.screen().id("id:popover-instance-app").unwrap(),
        app_row_id
    );
    assert_eq!(
        app.screen().id("id:popover-instance-cache").unwrap(),
        cache_row_id
    );
    assert_eq!(app.screen().id("id:popover-proxy").unwrap(), proxy_row_id);
    drop(turn);
}

#[component]
fn PopoverNavigationHarness() -> impl IntoElement {
    nsh_app_devbase::testing::menubar_load();
    view()
        .size_full()
        .flex_row()
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_col()
                .child(nsh_app_devbase::app()),
        )
        .child(
            view()
                .w(px(340.0))
                .flex_none()
                .flex_col()
                .child(
                    nsh_base::button()
                        .test_id("open-popover-shortcut")
                        .on_press(nsh_app_devbase::testing::menubar_toggle_popover_shortcut)
                        .child(text("Toggle Popover")),
                )
                .child(
                    text(move || format!("logs-job:{}", dev_logs::active_job_id()))
                        .test_id("active-logs-job"),
                )
                .child(when(
                    nsh_app_devbase::testing::menubar_popover_open,
                    nsh_app_devbase::testing::menubar_popover,
                )),
        )
}

#[test]
fn clicking_proxy_and_tunnel_rows_navigates_to_https_proxy_and_closes_popover() {
    let databases = FakeDatabases::new(database_fixture(&[("app", "Running")]));
    let mut snapshot = proxy_snapshot(vec![rule("app.test.devbase.localhost", true)], "Running");
    snapshot.token_configured = true;
    snapshot
        .workers
        .insert("app-test", "https://app-test.example.workers.dev");
    snapshot
        .statuses
        .insert("tunnel".to_string(), "Stopped".to_string());
    let proxy = FakeProxy::new(snapshot);
    let settings = FakeSettings::new(settings_snapshot());

    let turn = serial();
    use_fake_menubar();
    use_fake_providers();
    use_fake_databases();
    install_databases_provider(databases as Arc<dyn DatabasesProvider>);
    use_fake_proxy();
    install_proxy_provider(proxy as Arc<dyn ProxyProvider>);
    use_fake_system();
    install_system_settings_provider(settings as Arc<dyn SystemSettingsProvider>);
    use_fake_guide();
    use_fake_software();
    use_fake_development();
    use_fake_dev_logs();

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<PopoverNavigationHarness>())).unwrap();
    app.set_viewport(Size::new(px(1440.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();

    // Open popover, click Proxy row -> closes popover & navigates to HTTPS Proxy.
    app.press("id:open-popover-shortcut").unwrap();
    app.wait_for("id:popover-open-proxy").unwrap();
    app.press("id:popover-open-proxy").unwrap();
    app.wait_for_gone("id:menubar-popover", Duration::from_secs(2))
        .unwrap();
    app.wait_for("id:proxy-add-rule").unwrap();

    // Switch back to Services, reopen popover, click Tunnel row -> navigates to HTTPS Proxy.
    app.press("role:button[Services]").unwrap();
    app.wait_for("id:services-search").unwrap();
    app.press("id:open-popover-shortcut").unwrap();
    app.wait_for("id:popover-open-tunnel").unwrap();
    app.press("id:popover-open-tunnel").unwrap();
    app.wait_for_gone("id:menubar-popover", Duration::from_secs(2))
        .unwrap();
    app.wait_for("id:proxy-add-rule").unwrap();
    drop(turn);
}

#[test]
fn stop_all_databases_stops_every_running_instance_in_popover_and_menu() {
    let running_panel = MenubarPanel::from(MenubarInput {
        instances: vec![
            MenubarInstance {
                id: "app".to_string(),
                title: "PostgreSQL".to_string(),
                port: 5432,
                status: "Running".to_string(),
            },
            MenubarInstance {
                id: "cache".to_string(),
                title: "Redis".to_string(),
                port: 6379,
                status: "Running".to_string(),
            },
        ],
        proxy_status: "Stopped".to_string(),
        proxy_port: 8443,
        ..MenubarInput::default()
    });
    let running_entries = menubar_menu_entries(&running_panel);
    assert_every_menu_entry_has_svg_icon(&running_entries);
    assert!(running_entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            label,
            enabled: true,
            action: Some(MenubarAction::StopAllDatabases),
            ..
        } if label == "Stop All Databases"
    )));

    let stopped_panel = MenubarPanel::from(MenubarInput {
        instances: vec![MenubarInstance {
            id: "app".to_string(),
            title: "PostgreSQL".to_string(),
            port: 5432,
            status: "Stopped".to_string(),
        }],
        proxy_status: "Stopped".to_string(),
        proxy_port: 8443,
        ..MenubarInput::default()
    });
    let stopped_entries = menubar_menu_entries(&stopped_panel);
    assert!(!stopped_entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            action: Some(MenubarAction::StopAllDatabases),
            ..
        }
    )));

    let databases = FakeDatabases::new(database_fixture(&[
        ("app", "Running"),
        ("cache", "Running"),
    ]));
    let proxy = FakeProxy::new(proxy_snapshot(Vec::new(), "Stopped"));
    let settings = FakeSettings::new(settings_snapshot());

    let turn = serial();
    use_fake_menubar();
    use_fake_providers();
    use_fake_databases();
    install_databases_provider(databases.clone() as Arc<dyn DatabasesProvider>);
    use_fake_proxy();
    install_proxy_provider(proxy as Arc<dyn ProxyProvider>);
    use_fake_system();
    install_system_settings_provider(settings as Arc<dyn SystemSettingsProvider>);

    let mut app = Headless::start(EngineConfig::new(Root::of::<PopoverHarness>())).unwrap();
    app.set_viewport(Size::new(px(340.0), px(520.0))).unwrap();
    app.wait_for("id:popover-stop-all").unwrap();
    assert!(contains(&app, "2 running"));

    app.press("id:popover-stop-all").unwrap();
    wait_contains(&mut app, "0 running", Duration::from_secs(5)).unwrap();
    app.wait_for_gone("id:popover-stop-all", Duration::from_secs(2))
        .unwrap();
    let calls = databases.calls();
    assert!(
        calls.iter().any(|call| call == "stop app"),
        "calls: {calls:?}"
    );
    assert!(
        calls.iter().any(|call| call == "stop cache"),
        "calls: {calls:?}"
    );
    drop(turn);
}

#[test]
fn https_and_tunnel_urls_open_copy_and_collapse_in_popover_and_menu() {
    let menu_panel = MenubarPanel::from(MenubarInput {
        rules: vec![MenubarRule {
            host: "api.devbase.localhost".to_string(),
            target: "127.0.0.1:8080".to_string(),
            tunnel: true,
            tunnel_url: "https://api.example.workers.dev".to_string(),
        }],
        proxy_status: "Running".to_string(),
        proxy_port: 8443,
        tunnel_status: "Running".to_string(),
        tunnel_configured: true,
        ..MenubarInput::default()
    });
    let entries = menubar_menu_entries(&menu_panel);
    assert_every_menu_entry_has_svg_icon(&entries);
    assert!(entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            action: Some(MenubarAction::OpenUrl(url)),
            ..
        } if url == "https://api.devbase.localhost:8443"
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            action: Some(MenubarAction::CopyUrl(url)),
            ..
        } if url == "https://api.devbase.localhost:8443"
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            action: Some(MenubarAction::OpenUrl(url)),
            ..
        } if url == "https://api.example.workers.dev"
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            action: Some(MenubarAction::CopyUrl(url)),
            ..
        } if url == "https://api.example.workers.dev"
    )));

    // When tunnel is stopped, tunnel URLs are omitted from menu entries.
    let stopped_tunnel_panel = MenubarPanel::from(MenubarInput {
        rules: vec![MenubarRule {
            host: "api.devbase.localhost".to_string(),
            target: "127.0.0.1:8080".to_string(),
            tunnel: true,
            tunnel_url: "https://api.example.workers.dev".to_string(),
        }],
        proxy_status: "Running".to_string(),
        proxy_port: 8443,
        tunnel_status: "Stopped".to_string(),
        tunnel_configured: true,
        ..MenubarInput::default()
    });
    let stopped_entries = menubar_menu_entries(&stopped_tunnel_panel);
    assert!(!stopped_entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            action: Some(MenubarAction::OpenUrl(url)),
            ..
        } if url == "https://api.example.workers.dev"
    )));

    let mut proxy_rule = rule("app.test.devbase.localhost", true);
    proxy_rule.tunnel_url = "https://app-test.example.workers.dev".to_string();
    let mut snapshot = proxy_snapshot(vec![proxy_rule], "Running");
    snapshot.token_configured = true;
    snapshot
        .statuses
        .insert("tunnel".to_string(), "Running".to_string());

    let databases = FakeDatabases::new(database_fixture(&[("app", "Stopped")]));
    let proxy = FakeProxy::new(snapshot);
    let settings = FakeSettings::new(settings_snapshot());
    let opened_urls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let opener_log = opened_urls.clone();

    let turn = serial();
    use_fake_menubar();
    install_menubar_url_opener(Arc::new(move |url| {
        opener_log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(url.to_string());
    }));
    use_fake_providers();
    use_fake_databases();
    install_databases_provider(databases as Arc<dyn DatabasesProvider>);
    use_fake_proxy();
    install_proxy_provider(proxy as Arc<dyn ProxyProvider>);
    use_fake_system();
    install_system_settings_provider(settings as Arc<dyn SystemSettingsProvider>);

    let mut app = Headless::start(EngineConfig::new(Root::of::<PopoverHarness>())).unwrap();
    app.set_viewport(Size::new(px(340.0), px(560.0))).unwrap();
    app.wait_for("id:popover-section-rules-toggle").unwrap();
    app.wait_for("id:popover-open-rule-app.test.devbase.localhost")
        .unwrap();
    app.wait_for("id:popover-open-tunnel-rule-app.test.devbase.localhost")
        .unwrap();

    // Copy local HTTPS URL and verify clipboard.
    app.press("id:popover-copy-rule-app.test.devbase.localhost")
        .unwrap();
    assert_eq!(
        app.screen().clipboard(),
        Some("https://app.test.devbase.localhost:8443")
    );

    // Open local HTTPS URL in browser.
    app.press("id:popover-open-rule-app.test.devbase.localhost")
        .unwrap();

    // Copy & open tunnel URL when tunnel is running.
    app.press("id:popover-copy-tunnel-rule-app.test.devbase.localhost")
        .unwrap();
    assert_eq!(
        app.screen().clipboard(),
        Some("https://app-test.example.workers.dev")
    );
    app.press("id:popover-open-tunnel-rule-app.test.devbase.localhost")
        .unwrap();

    let opened = opened_urls
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(
        opened,
        vec![
            "https://app.test.devbase.localhost:8443".to_string(),
            "https://app-test.example.workers.dev".to_string(),
        ]
    );

    // Collapsing URLs hides rule rows; expanding brings them back.
    app.press("id:popover-section-rules-toggle").unwrap();
    app.wait_for_gone(
        "id:popover-rule-app.test.devbase.localhost",
        Duration::from_secs(2),
    )
    .unwrap();
    app.press("id:popover-section-rules-toggle").unwrap();
    app.wait_for("id:popover-rule-app.test.devbase.localhost")
        .unwrap();

    clear_menubar_url_opener();
    drop(turn);
}

#[test]
fn failed_resources_expose_logs_jump_in_menu_and_popover() {
    let failed_panel = MenubarPanel::from(MenubarInput {
        instances: vec![MenubarInstance {
            id: "app".to_string(),
            title: "PostgreSQL".to_string(),
            port: 5432,
            status: "Failed".to_string(),
        }],
        proxy_status: "Failed".to_string(),
        proxy_rules: 1,
        proxy_port: 8443,
        tunnel_status: "Failed".to_string(),
        tunnel_configured: true,
        tunnel_rules: 1,
        ..MenubarInput::default()
    });
    let entries = menubar_menu_entries(&failed_panel);
    assert_every_menu_entry_has_svg_icon(&entries);
    assert!(entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            action: Some(MenubarAction::OpenLogs(job)),
            ..
        } if job == "db-app"
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            action: Some(MenubarAction::OpenLogs(job)),
            ..
        } if job == "proxy"
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        MenubarMenuEntry::Item {
            action: Some(MenubarAction::OpenLogs(job)),
            ..
        } if job == "tunnel"
    )));

    let databases =
        FakeDatabases::new(database_fixture(&[("app", "Failed"), ("cache", "Running")]));
    let mut snapshot = proxy_snapshot(vec![rule("app.test.devbase.localhost", true)], "Failed");
    snapshot.token_configured = true;
    snapshot
        .statuses
        .insert("tunnel".to_string(), "Failed".to_string());
    let proxy = FakeProxy::new(snapshot);
    let settings = FakeSettings::new(settings_snapshot());

    let turn = serial();
    use_fake_menubar();
    use_fake_providers();
    use_fake_databases();
    install_databases_provider(databases as Arc<dyn DatabasesProvider>);
    use_fake_proxy();
    install_proxy_provider(proxy as Arc<dyn ProxyProvider>);
    use_fake_system();
    install_system_settings_provider(settings as Arc<dyn SystemSettingsProvider>);
    use_fake_guide();
    use_fake_software();
    use_fake_development();
    use_fake_dev_logs();

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<PopoverNavigationHarness>())).unwrap();
    app.set_viewport(Size::new(px(1440.0), px(720.0))).unwrap();
    app.wait_for("Development").unwrap();

    // Open popover, verify Logs buttons on failed db, proxy, and tunnel, and jump to db-app logs.
    app.press("id:open-popover-shortcut").unwrap();
    app.wait_for("id:popover-logs-instance-app").unwrap();
    app.wait_for("id:popover-logs-proxy").unwrap();
    app.wait_for("id:popover-logs-tunnel").unwrap();
    assert_eq!(
        app.screen()
            .count("id:popover-logs-instance-cache")
            .unwrap(),
        0
    );

    app.press("id:popover-logs-instance-app").unwrap();
    app.wait_for_gone("id:menubar-popover", Duration::from_secs(2))
        .unwrap();
    app.wait_for("id:dev-log-drawer").unwrap();
    app.wait_for("text:logs-job:db-app").unwrap();

    // Reopen popover and jump to proxy logs.
    app.press("id:open-popover-shortcut").unwrap();
    app.wait_for("id:popover-logs-proxy").unwrap();
    app.press("id:popover-logs-proxy").unwrap();
    app.wait_for_gone("id:menubar-popover", Duration::from_secs(2))
        .unwrap();
    app.wait_for("text:logs-job:proxy").unwrap();

    // Reopen popover and jump to tunnel logs.
    app.press("id:open-popover-shortcut").unwrap();
    app.wait_for("id:popover-logs-tunnel").unwrap();
    app.press("id:popover-logs-tunnel").unwrap();
    app.wait_for_gone("id:menubar-popover", Duration::from_secs(2))
        .unwrap();
    app.wait_for("text:logs-job:tunnel").unwrap();

    drop(turn);
}

#[test]
fn popover_pins_footer_inside_viewport_with_many_databases_and_routes() {
    let mut statuses = BTreeMap::new();
    for (id, status) in [
        ("pg-main", "Running"),
        ("redis-cache", "Running"),
        ("mysql-legacy", "Stopped"),
        ("mongo-events", "Running"),
    ] {
        statuses.insert(id.to_string(), status.to_string());
    }
    let databases = FakeDatabases::new(DatabaseFixture {
        instances: vec![
            instance("pg-main", "postgresql", 5432),
            instance("redis-cache", "redis", 6379),
            instance("mysql-legacy", "mysql", 3306),
            instance("mongo-events", "mongodb", 27017),
        ],
        statuses,
    });

    let mut proxy_rule = rule("app.devbase.localhost", true);
    proxy_rule.tunnel_url = "https://preview.example.dev".to_string();
    let mut snapshot = proxy_snapshot(
        vec![
            proxy_rule,
            rule("api.devbase.localhost", false),
            rule("docs.devbase.localhost", false),
            rule("admin.devbase.localhost", false),
            rule("worker.devbase.localhost", false),
        ],
        "Running",
    );
    snapshot.token_configured = true;
    snapshot
        .statuses
        .insert("tunnel".to_string(), "Running".to_string());
    let proxy = FakeProxy::new(snapshot);
    let settings = FakeSettings::new(settings_snapshot());

    let turn = serial();
    use_fake_menubar();
    use_fake_providers();
    use_fake_databases();
    install_databases_provider(databases as Arc<dyn DatabasesProvider>);
    use_fake_proxy();
    install_proxy_provider(proxy as Arc<dyn ProxyProvider>);
    use_fake_system();
    install_system_settings_provider(settings as Arc<dyn SystemSettingsProvider>);

    let mut app = Headless::start(EngineConfig::new(Root::of::<PopoverHarness>())).unwrap();
    app.set_viewport(Size::new(px(340.0), px(520.0))).unwrap();
    app.wait_for("id:menubar-popover-footer").unwrap();
    app.wait_for("id:popover-open").unwrap();
    app.wait_for("id:popover-quit").unwrap();

    assert!(contains(&app, "Routes"));
    assert!(contains(&app, "Proxy"));
    assert!(!contains(&app, "Proxy · Running"));
    assert!(contains(&app, "Tunnel"));
    assert!(!contains(&app, "Tunnel · Running"));

    let nodes = app.screen().layout_nodes();
    let bounds_of = |id: &str| {
        nodes
            .iter()
            .find(|n| n.test_id.as_deref() == Some(id))
            .map(|n| n.bounds)
            .unwrap_or_else(|| panic!("missing layout node for `{id}`"))
    };

    let header = bounds_of("menubar-popover-header");
    let body = bounds_of("menubar-popover-body");
    let footer = bounds_of("menubar-popover-footer");

    assert!(header.size.height.0 > 0.0, "header collapsed: {header:?}");
    assert!(body.size.height.0 > 100.0, "body collapsed: {body:?}");
    assert!(footer.size.height.0 >= 36.0, "footer collapsed: {footer:?}");
    assert!(
        footer.origin.y.0 + footer.size.height.0 <= 520.5,
        "footer clipped below 520pt viewport: header={header:?} body={body:?} footer={footer:?}"
    );
    assert!(
        body.origin.y.0 + body.size.height.0 <= footer.origin.y.0 + 1.0,
        "body overlapped footer: body={body:?} footer={footer:?}"
    );
    drop(app);

    // Also verify estimated_popover_height_for() sizes the popup cleanly for
    // both rich content (clamped to 560pt) and empty/sparse content (220..=300pt),
    // and that popover_content() keeps header, scrollable body, and footer
    // cleanly separated and unclipped at the computed popup height.
    let rich_panel = MenubarPanel::from(MenubarInput {
        instances: vec![
            MenubarInstance {
                id: "pg-main".into(),
                title: "PostgreSQL".into(),
                port: 5432,
                status: "Running".into(),
            },
            MenubarInstance {
                id: "redis-cache".into(),
                title: "Redis".into(),
                port: 6379,
                status: "Running".into(),
            },
            MenubarInstance {
                id: "mysql-legacy".into(),
                title: "MySQL".into(),
                port: 3306,
                status: "Stopped".into(),
            },
        ],
        proxy_status: "Running".into(),
        proxy_port: 443,
        rules: vec![
            MenubarRule {
                host: "app.devbase.localhost".into(),
                target: "127.0.0.1:3000".into(),
                tunnel: true,
                tunnel_url: "https://preview.example.dev".into(),
            },
            MenubarRule {
                host: "api.devbase.localhost".into(),
                target: "127.0.0.1:8080".into(),
                tunnel: false,
                tunnel_url: String::new(),
            },
            MenubarRule {
                host: "docs.devbase.localhost".into(),
                target: "127.0.0.1:4321".into(),
                tunnel: false,
                tunnel_url: String::new(),
            },
        ],
        tunnel_configured: true,
        tunnel_status: "Running".into(),
        ..MenubarInput::default()
    });
    let estimated = nsh_app_devbase::testing::menubar_estimated_popover_height_for(
        &rich_panel,
        true,
        true,
        true,
        true,
    );
    assert!(
        (520.0..=560.0).contains(&estimated),
        "rich panel estimated height should be 520..=560pt, got {estimated}"
    );
    let empty_estimated = nsh_app_devbase::testing::menubar_estimated_popover_height_for(
        &MenubarPanel::from(MenubarInput::default()),
        true,
        true,
        true,
        true,
    );
    assert!(
        (220.0..=300.0).contains(&empty_estimated),
        "empty panel estimated height should be compact (220..=300pt), got {empty_estimated}"
    );

    let mut fitted = Headless::start(EngineConfig::new(Root::of::<PopoverHarness>())).unwrap();
    fitted
        .set_viewport(Size::new(px(340.0), px(estimated)))
        .unwrap();
    fitted.wait_for("id:menubar-popover-footer").unwrap();
    let fnodes = fitted.screen().layout_nodes();
    let fbounds = |id: &str| {
        fnodes
            .iter()
            .find(|n| n.test_id.as_deref() == Some(id))
            .map(|n| n.bounds)
            .unwrap_or_else(|| panic!("missing fitted layout node for `{id}`"))
    };
    let froot = fbounds("menubar-popover");
    let fbody = fbounds("menubar-popover-body");
    let ffooter = fbounds("menubar-popover-footer");
    assert!(
        froot.size.height.0 >= 460.0 && froot.size.height.0 <= 560.5,
        "popup height wrong: root={froot:?} body={fbody:?} footer={ffooter:?}"
    );
    assert!(
        fbody.size.height.0 > 300.0,
        "popup body collapsed: root={froot:?} body={fbody:?} footer={ffooter:?}"
    );
    assert!(
        ffooter.origin.y.0 + ffooter.size.height.0 <= froot.size.height.0 + 0.5,
        "popup footer clipped: root={froot:?} body={fbody:?} footer={ffooter:?}"
    );

    drop(turn);
}

#[test]
fn right_click_menu_displays_linux_systemd_user_services() {
    let panel = MenubarPanel::from(MenubarInput {
        services: vec![
            MenubarService {
                id: "systemd-user:/home/alice/.config/systemd/user/api.service".to_string(),
                source: Source::SystemdUser,
                name: "api.service".to_string(),
                status: "Running".to_string(),
                running: true,
                busy: false,
                can_toggle: true,
            },
            MenubarService {
                id: "systemd-user:/home/alice/.config/systemd/user/worker.service".to_string(),
                source: Source::SystemdUser,
                name: "worker.service".to_string(),
                status: "Stopped".to_string(),
                running: false,
                busy: false,
                can_toggle: true,
            },
        ],
        proxy_status: "Stopped".to_string(),
        proxy_port: 8443,
        ..MenubarInput::default()
    });
    let entries = menubar_menu_entries(&panel);
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            MenubarMenuEntry::Item { label, enabled: false, .. } if label == "User Units · 1/2"
        )),
        "expected User Units header in Linux tray menu: {entries:#?}"
    );
    assert_every_menu_entry_has_svg_icon(&entries);
}
