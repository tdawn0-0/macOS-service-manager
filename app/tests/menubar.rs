//! The menu bar panel and the first-run guide, headless. Fakes stand in for
//! the database, proxy and settings providers and for the guide's file and
//! TCC probe: no test runs launchd, reaches the network or reads a real file.
//!
//! The tray itself is the platform's (NSMenu); the rows it shows are the
//! panel model, which the test harness renders as text and whose actions it
//! dispatches exactly as the menu items do.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use devcore::{
    databases::{
        BackupFile, ConnectionInfo, DatabaseWorkInfo, InstanceAction, InstanceCatalog,
        InstanceDeletionInfo, InstanceDeletionPreview, InstanceInitializationInfo, PortInspection,
    },
    model::{ConnectionProfile, Instance, ProxyRule},
    shutdown::{ShutdownPlan, ShutdownResource},
};
use nsh_app_devbase::testing::{
    AuthorityInfo, AuthorityStatus, CancelToken, DatabasesProvider, DatabasesSnapshot, GuideAccess,
    GuidePersistence, ProxyProvider, ProxySnapshot, ProxyWorkers, SystemSettingsProvider,
    SystemSettingsSnapshot, install_databases_provider, install_guide_access,
    install_guide_persistence, install_proxy_provider, install_system_settings_provider,
    use_fake_databases, use_fake_guide, use_fake_proxy, use_fake_system,
};
use nsh_core::prelude::*;
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
        data_identity: String::new(),
        data_directory: String::new(),
        data_owner: String::new(),
        id: id.to_string(),
        kind: kind.to_string(),
        package: format!("{kind}@17.11-arm64"),
        port,
        persistent: true,
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
    assert!(contains(
        &app,
        "Everything that starts on your Mac, in one place."
    ));
    assert!(contains(&app, "Full Disk Access"));
    assert!(contains(&app, "Only to read the list of login items."));
    assert!(contains(&app, "Skip for Now"));
    assert!(contains(&app, "Without it, some login items may not show."));

    app.press("id:guide-allow").unwrap();
    app.wait_for("Add this app with the + button").unwrap();
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

    // Toggle database instance switch on.
    app.press("id:popover-switch-instance-app").unwrap();
    wait_contains(&mut app, "2 running", Duration::from_secs(5)).unwrap();
    assert!(databases.calls().iter().any(|call| call == "start app"));

    // Toggle proxy switch on.
    app.press("id:popover-switch-proxy").unwrap();
    wait_contains(&mut app, "Proxy · Running", Duration::from_secs(5)).unwrap();
    assert!(proxy.calls().iter().any(|call| call == "apply proxy"));

    // Toggle tunnel switch on.
    app.press("id:popover-switch-tunnel").unwrap();
    wait_contains(&mut app, "Tunnel · Running", Duration::from_secs(5)).unwrap();
    assert!(proxy.calls().iter().any(|call| call == "start tunnel"));

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
