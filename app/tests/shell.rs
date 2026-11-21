//! The shell's first frame, navigation, command palette, global toasts,
//! sidebar live glanceability, and macOS desktop chrome, headless: every store
//! runs on fake providers so tests never touch real launchd, Homebrew, devcore
//! state, or host preferences.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use chrono::Utc;
use devcore::{
    databases::{
        BackupFile, ConnectionInfo, DatabaseWorkInfo, InstanceAction, InstanceCatalog,
        InstanceDeletionInfo, InstanceDeletionPreview, InstanceInitializationInfo, PortInspection,
    },
    model::{ConnectionProfile, Instance, Package, ProxyRule},
    service::{
        Action as ServiceAction, Autostart, Detail as ServiceDetail, Item as ServiceItem,
        Provider as ServiceProvider, Result as ServiceResult, State as ServiceState,
    },
    shutdown::ShutdownPlan,
};
use nsh_app_devbase::testing::{
    AuthorityAction, AuthorityInfo, AuthorityStatus, CancelToken, DatabasesProvider,
    DatabasesSnapshot, DevelopmentJava, DevelopmentNode, DevelopmentProvider, DevelopmentReleases,
    DevelopmentSelection, DevelopmentShell, DevelopmentSnapshot, GuideAccess, GuidePersistence,
    ProxyHttpResult, ProxyListeningPort, ProxyProvider, ProxySavedCloudflare, ProxySnapshot,
    ProxyWorkers, Source, SystemCacheClear, SystemCliChange, SystemCliStatus, SystemHelperChange,
    SystemHelperStatus, SystemLoginChange, SystemSettingsProvider, SystemSettingsSnapshot,
    TrustState, command_palette_is_open, development_load, install_databases_provider,
    install_development_provider, install_guide_access, install_guide_persistence,
    install_provider, install_proxy_provider, install_system_settings_provider, menubar_load,
    refresh_services, shell_trigger_find, toast_dismiss_all, toast_error, toast_info,
    toast_success, use_fake_databases, use_fake_dev_logs, use_fake_development, use_fake_editor,
    use_fake_guide, use_fake_menubar, use_fake_providers, use_fake_proxy, use_fake_software,
    use_fake_system,
};
use nsh_core::{
    desktop::{WindowMode, WindowState},
    platform::Os,
    prelude::*,
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn install_all_fakes() {
    use_fake_menubar();
    use_fake_providers();
    use_fake_software();
    use_fake_development();
    use_fake_databases();
    use_fake_proxy();
    use_fake_system();
    use_fake_guide();
    use_fake_dev_logs();
    use_fake_editor();
}

fn start() -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    install_all_fakes();
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.wait_for("Services").unwrap();
    (turn, app)
}

fn start_macos() -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    install_all_fakes();
    let mut config = EngineConfig::new(Root::of::<nsh_app_devbase::App>());
    config.environment.os = Os::MacOs;
    config.environment.title_bar.top = px(52.0);
    let mut app = Headless::start(config).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    (turn, app)
}

struct FakeShellDatabases {
    snapshot: DatabasesSnapshot,
}

impl DatabasesProvider for FakeShellDatabases {
    fn snapshot(&self) -> Result<DatabasesSnapshot, String> {
        Ok(self.snapshot.clone())
    }
    fn next_port(&self) -> Result<i64, String> {
        Ok(55432)
    }
    fn inspect_port(&self, _port: i64) -> Result<PortInspection, String> {
        Err("unused".to_string())
    }
    fn create(&self, _instance: Instance, _cancel: CancelToken) -> Result<(), String> {
        Err("unused".to_string())
    }
    fn action(
        &self,
        _id: &str,
        _action: InstanceAction,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Ok(())
    }
    fn catalog(&self, _id: &str, _cancel: CancelToken) -> Result<InstanceCatalog, String> {
        Err("unused".to_string())
    }
    fn create_database(&self, _id: &str, _name: &str, _cancel: CancelToken) -> Result<(), String> {
        Err("unused".to_string())
    }
    fn create_user(
        &self,
        _id: &str,
        _user: &str,
        _password: &str,
        _database: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("unused".to_string())
    }
    fn drop_database(&self, _id: &str, _name: &str, _cancel: CancelToken) -> Result<(), String> {
        Err("unused".to_string())
    }
    fn drop_user(
        &self,
        _id: &str,
        _user: &str,
        _database: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("unused".to_string())
    }
    fn backups(&self, _id: &str, _cancel: CancelToken) -> Result<Vec<BackupFile>, String> {
        Ok(Vec::new())
    }
    fn backup(&self, _id: &str, _database: &str, _cancel: CancelToken) -> Result<String, String> {
        Err("unused".to_string())
    }
    fn restore(
        &self,
        _id: &str,
        _database: &str,
        _path: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("unused".to_string())
    }
    fn delete_backup(&self, _id: &str, _name: &str, _cancel: CancelToken) -> Result<(), String> {
        Err("unused".to_string())
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
        Err("unused".to_string())
    }
    fn connection_details(&self, _id: &str) -> Result<String, String> {
        Err("unused".to_string())
    }
    fn password(&self, _id: &str) -> Result<String, String> {
        Err("unused".to_string())
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
        Err("unused".to_string())
    }
    fn preview_deletion(&self, _id: &str) -> Result<InstanceDeletionPreview, String> {
        Err("unused".to_string())
    }
    fn delete_instance_data(
        &self,
        _id: &str,
        _fingerprint: &str,
        _confirmation: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("unused".to_string())
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
        Err("unused".to_string())
    }
    fn finish_deletion(
        &self,
        _id: &str,
        _revision: &str,
        _confirmation: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Err("unused".to_string())
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
        Err("unused".to_string())
    }
    fn pending_work(&self) -> Result<Vec<DatabaseWorkInfo>, String> {
        Ok(Vec::new())
    }
    fn resolve_work(&self, _id: &str, _revision: &str) -> Result<(), String> {
        Err("unused".to_string())
    }
}

struct FakeShellProxy {
    snapshot: ProxySnapshot,
    trust: TrustState,
}

impl FakeShellProxy {
    fn new(snapshot: ProxySnapshot) -> Self {
        Self {
            snapshot,
            trust: TrustState::NotChecked,
        }
    }
}

impl ProxyProvider for FakeShellProxy {
    fn snapshot(&self) -> Result<ProxySnapshot, String> {
        Ok(self.snapshot.clone())
    }
    fn listening_ports(&self) -> Result<Vec<ProxyListeningPort>, String> {
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
        Ok(())
    }
    fn stop_proxy(&self) -> Result<(), String> {
        Ok(())
    }
    fn restart_proxy(&self) -> Result<(), String> {
        Ok(())
    }
    fn proxy_result(&self, _domain: &str) -> Result<ProxyHttpResult, String> {
        Err("unused".to_string())
    }
    fn tunnel_result(&self, _domain: &str) -> Result<ProxyHttpResult, String> {
        Err("unused".to_string())
    }
    fn authority_status(&self) -> Result<AuthorityStatus, String> {
        let active = AuthorityInfo {
            trust: self.trust,
            ..AuthorityInfo::default()
        };
        Ok(AuthorityStatus {
            checked_at: Utc::now(),
            active,
            prepared: None,
            previous: None,
            archives: Vec::new(),
            prepared_is_active: false,
            revision: String::new(),
        })
    }
    fn authority_action(&self, _action: AuthorityAction) -> Result<(), String> {
        Ok(())
    }
    fn save_cloudflare(
        &self,
        _token_path: Option<&Path>,
        account_id: &str,
        _client_id: &str,
    ) -> Result<ProxySavedCloudflare, String> {
        Ok(ProxySavedCloudflare {
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
        Ok(())
    }
    fn stop_tunnel(&self) -> Result<(), String> {
        Ok(())
    }
    fn restart_tunnel(&self) -> Result<(), String> {
        Ok(())
    }
    fn remove_cloudflare(&self, _delete_workers: bool) -> Result<(), String> {
        Ok(())
    }
    fn ensure_tunnel_token(&self, _domain: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn rotate_tunnel_token(&self, _domain: &str) -> Result<(), String> {
        Ok(())
    }
    fn logs(&self, _job: &str) -> Result<String, String> {
        Ok(String::new())
    }
}

struct FakeShellServices {
    source: Source,
    items: Vec<ServiceItem>,
}

impl ServiceProvider for FakeShellServices {
    fn source(&self) -> Source {
        self.source
    }
    fn list(&self) -> ServiceResult<Vec<ServiceItem>> {
        Ok(self.items.clone())
    }
    fn detail(&self, _item: &ServiceItem) -> ServiceResult<ServiceDetail> {
        Ok(ServiceDetail::default())
    }
    fn actions(&self, _item: &ServiceItem) -> Vec<ServiceAction> {
        Vec::new()
    }
    fn confirm(&self, _item: &ServiceItem, _action: ServiceAction) -> Option<String> {
        None
    }
    fn do_action(&self, _item: &ServiceItem, _action: ServiceAction) -> ServiceResult<()> {
        Ok(())
    }
}

fn sample_instance(id: &str, kind: &str, port: i64) -> Instance {
    Instance {
        id: id.to_string(),
        kind: kind.to_string(),
        package: format!("{kind}@17.1-arm64"),
        port,
        persistent: true,
        ..Instance::default()
    }
}

fn sample_service(id: &str, source: Source, name: &str, state: ServiceState) -> ServiceItem {
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

#[test]
fn sidebar_lists_the_six_sections() {
    let (_turn, app) = start();
    assert_eq!(app.screen().count("id:sidebar-group-workspace").unwrap(), 1);
    assert_eq!(app.screen().count("id:sidebar-group-system").unwrap(), 1);
    assert!(app.screen().shows("WORKSPACE"));
    assert!(app.screen().shows("SYSTEM"));
    assert_eq!(app.screen().count("id:readiness-bar").unwrap(), 0);
    for section in [
        "Development",
        "Databases",
        "HTTPS Proxy",
        "Services",
        "Diagnostics",
        "Settings",
    ] {
        assert!(
            app.screen().shows(section),
            "the sidebar lacks the {section} row"
        );
    }
}

#[test]
fn choosing_a_section_switches_the_workspace() {
    let (_turn, mut app) = start();
    app.wait_for("id:development").unwrap();
    app.wait_for("Runtimes and software the development workspace manages.")
        .unwrap();

    app.press("role:button[Databases]").unwrap();
    app.wait_for("Database instances, ports, connection profiles and backups.")
        .unwrap();

    app.press("role:button[Settings]").unwrap();
    app.wait_for("Helper installation, CLI, login items and workspace preferences.")
        .unwrap();

    app.press("role:button[Services]").unwrap();
    app.wait_for("launchd jobs, their status and lifecycle controls.")
        .unwrap();

    app.press("role:button[Development]").unwrap();
    app.wait_for("id:development").unwrap();
}

#[test]
fn macos_sidebar_uses_liquid_glass_and_falls_back_in_full_screen() {
    let (_turn, mut app) = start_macos();

    // Outside full screen on macOS, the Liquid Glass backdrop layer and
    // hidden title-bar traffic-light reservation strip are both mounted.
    app.wait_for("id:sidebar-glass").unwrap();
    app.wait_for("id:sidebar-title-bar").unwrap();
    assert_eq!(app.screen().count("id:sidebar-glass").unwrap(), 1);
    assert_eq!(app.screen().count("id:sidebar-title-bar").unwrap(), 1);

    // Entering full screen removes the glass backdrop and falls back to solid sidebar fill.
    let mut full = WindowState::default();
    full.mode = WindowMode::FullScreen;
    app.set_environment("enter full screen", |env| {
        env.window_state = Some(full);
        env.title_bar.top = px(0.0);
    })
    .unwrap();
    app.wait_for_gone("id:sidebar-glass", Duration::from_secs(2))
        .unwrap();

    // Leaving full screen restores Liquid Glass and the title-bar strip.
    let mut normal = WindowState::default();
    normal.mode = WindowMode::Normal;
    app.set_environment("leave full screen", |env| {
        env.window_state = Some(normal);
        env.title_bar.top = px(52.0);
    })
    .unwrap();
    app.wait_for("id:sidebar-glass").unwrap();
    app.wait_for("id:sidebar-title-bar").unwrap();
}

#[test]
fn services_lists_all_five_sources_and_sidebar_toggles_log_drawer() {
    let (_turn, mut app) = start();
    app.press("role:button[Services]").unwrap();
    app.wait_for("Homebrew").unwrap();
    for source in [
        "Homebrew",
        "Login Items",
        "User Agents",
        "System Agents",
        "Daemons",
    ] {
        assert!(
            app.screen().shows(source),
            "Services rail lacks the {source} source"
        );
    }

    assert_eq!(app.screen().count("id:dev-log-drawer").unwrap(), 0);
    assert_eq!(app.screen().count("id:sidebar-logs-active-dot").unwrap(), 0);
    app.press("id:sidebar-logs-toggle").unwrap();
    app.wait_for("id:dev-log-drawer").unwrap();
    app.wait_for("id:sidebar-logs-active-dot").unwrap();
    assert_eq!(app.screen().count("id:dev-log-drawer").unwrap(), 1);

    app.press("id:sidebar-logs-toggle").unwrap();
    app.wait_for_gone("id:dev-log-drawer", Duration::from_secs(2))
        .unwrap();
    assert_eq!(app.screen().count("id:sidebar-logs-active-dot").unwrap(), 0);
}

#[test]
fn command_palette_opens_filters_and_executes_navigation_actions_and_db_jump() {
    let _turn = serial();
    install_all_fakes();

    let mut db_statuses = BTreeMap::new();
    db_statuses.insert("analytics".to_string(), "Running".to_string());
    install_databases_provider(Arc::new(FakeShellDatabases {
        snapshot: DatabasesSnapshot {
            instances: vec![sample_instance("analytics", "postgresql", 5432)],
            statuses: db_statuses,
            ..DatabasesSnapshot::default()
        },
    }));

    let mut proxy_statuses = BTreeMap::new();
    proxy_statuses.insert("proxy".to_string(), "Running".to_string());
    proxy_statuses.insert("tunnel".to_string(), "Stopped".to_string());
    install_proxy_provider(Arc::new(FakeShellProxy::new(ProxySnapshot {
        rules: vec![ProxyRule {
            domain: "api.devbase.localhost".to_string(),
            local_port: 8080,
            tunnel: true,
            ..ProxyRule::default()
        }],
        listener_port: 8443,
        statuses: proxy_statuses,
        token_configured: true,
        workers: ProxyWorkers::default(),
        ..ProxySnapshot::default()
    })));

    let mut config = EngineConfig::new(Root::of::<nsh_app_devbase::App>());
    config.environment.os = Os::MacOs;
    config.environment.title_bar.top = px(52.0);
    let mut app = Headless::start(config).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();

    // Open Command Palette via Cmd+K.
    assert_eq!(app.screen().count("id:command-palette").unwrap(), 0);
    app.press_key("mod+k").unwrap();
    app.wait_for("id:command-palette").unwrap();
    assert!(app.screen().shows("Go to Services"));
    assert!(app.screen().shows("New LaunchAgent…"));
    assert!(app.screen().shows("Start Cloudflare Tunnel"));

    // Filter to Settings and execute by pressing the matching item.
    app.type_text("label:Search commands", "settings").unwrap();
    app.wait_for("id:command-item-nav:settings").unwrap();
    assert_eq!(
        app.screen().count("id:command-item-nav:databases").unwrap(),
        0
    );
    app.press("id:command-item-nav:settings").unwrap();
    app.wait_for_gone("id:command-palette", Duration::from_secs(2))
        .unwrap();
    app.wait_for("id:settings-page").unwrap();

    // Re-open Command Palette, filter for the loaded DB instance, and jump to it.
    app.press_key("mod+k").unwrap();
    app.wait_for("id:command-palette").unwrap();
    app.type_text("label:Search commands", "analytics").unwrap();
    app.wait_for("id:command-item-db:analytics").unwrap();
    assert!(app.screen().shows("analytics (PostgreSQL · :5432)"));
    app.press("id:command-item-db:analytics").unwrap();
    app.wait_for_gone("id:command-palette", Duration::from_secs(2))
        .unwrap();
    app.wait_for("Database instances, ports, connection profiles and backups.")
        .unwrap();

    // Re-open Command Palette and toggle the Live Log Drawer.
    app.press_key("mod+k").unwrap();
    app.wait_for("id:command-palette").unwrap();
    app.type_text("label:Search commands", "log drawer")
        .unwrap();
    app.wait_for("id:command-item-action:toggle-logs").unwrap();
    app.press("id:command-item-action:toggle-logs").unwrap();
    app.wait_for_gone("id:command-palette", Duration::from_secs(2))
        .unwrap();
    app.wait_for("id:dev-log-drawer").unwrap();
}

#[component]
fn ToastHarness() -> impl IntoElement {
    view()
        .size_full()
        .flex_col()
        .child(
            view()
                .flex_row()
                .gap(px(4.0))
                .child(
                    nsh_base::button()
                        .test_id("fire-toast-info")
                        .on_press(|| toast_info("Copied connection string"))
                        .child(text("Info")),
                )
                .child(
                    nsh_base::button()
                        .test_id("fire-toast-success")
                        .on_press(|| toast_success("Proxy started"))
                        .child(text("Success")),
                )
                .child(
                    nsh_base::button()
                        .test_id("fire-toast-error")
                        .on_press(|| toast_error("Port 5432 already in use"))
                        .child(text("Error")),
                )
                .child(
                    nsh_base::button()
                        .test_id("clear-toasts")
                        .on_press(toast_dismiss_all)
                        .child(text("Clear")),
                ),
        )
        .child(nsh_app_devbase::app())
}

#[test]
fn global_toast_stack_renders_and_dismisses_toasts() {
    let _turn = serial();
    install_all_fakes();
    let mut app = Headless::start(EngineConfig::new(Root::of::<ToastHarness>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("id:fire-toast-info").unwrap();
    assert_eq!(app.screen().count("id:toast-item").unwrap(), 0);

    app.press("id:fire-toast-info").unwrap();
    app.wait_for("id:toast-stack").unwrap();
    app.wait_for("Copied connection string").unwrap();
    app.press("id:fire-toast-success").unwrap();
    app.wait_for("Proxy started").unwrap();
    app.press("id:fire-toast-error").unwrap();
    app.wait_for("Port 5432 already in use").unwrap();
    assert_eq!(app.screen().count("id:toast-item").unwrap(), 3);

    app.press("id:clear-toasts").unwrap();
    app.wait_for_gone("id:toast-item", Duration::from_secs(2))
        .unwrap();
}

#[component]
fn LiveSidebarHarness() -> impl IntoElement {
    menubar_load();
    refresh_services(Source::Brew);
    view()
        .size_full()
        .flex_col()
        .child(when(command_palette_is_open, || view().size_0()))
        .child(nsh_app_devbase::app())
}

#[test]
fn sidebar_displays_live_running_counts_for_services_databases_and_proxy() {
    let _turn = serial();
    install_all_fakes();

    install_provider(
        Source::Brew,
        Arc::new(FakeShellServices {
            source: Source::Brew,
            items: vec![
                sample_service("brew:redis", Source::Brew, "redis", ServiceState::Running),
                sample_service("brew:nginx", Source::Brew, "nginx", ServiceState::Running),
                sample_service("brew:mysql", Source::Brew, "mysql", ServiceState::Stopped),
            ],
        }),
    );

    let mut db_statuses = BTreeMap::new();
    db_statuses.insert("app".to_string(), "Running".to_string());
    db_statuses.insert("cache".to_string(), "Stopped".to_string());
    install_databases_provider(Arc::new(FakeShellDatabases {
        snapshot: DatabasesSnapshot {
            instances: vec![
                sample_instance("app", "postgresql", 5432),
                sample_instance("cache", "redis", 6379),
            ],
            statuses: db_statuses,
            ..DatabasesSnapshot::default()
        },
    }));

    let mut proxy_statuses = BTreeMap::new();
    proxy_statuses.insert("proxy".to_string(), "Running".to_string());
    install_proxy_provider(Arc::new(FakeShellProxy::new(ProxySnapshot {
        rules: vec![ProxyRule {
            domain: "web.devbase.localhost".to_string(),
            local_port: 3000,
            ..ProxyRule::default()
        }],
        listener_port: 8443,
        statuses: proxy_statuses,
        ..ProxySnapshot::default()
    })));

    let mut app = Headless::start(EngineConfig::new(Root::of::<LiveSidebarHarness>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();

    app.wait_for("id:sidebar-badge-services").unwrap();
    app.wait_for("id:sidebar-badge-databases").unwrap();
    app.wait_for("id:sidebar-badge-https-proxy").unwrap();
    assert!(app.screen().shows("1/2"));
}

#[component]
fn FindHarness() -> impl IntoElement {
    view()
        .size_full()
        .flex_col()
        .child(
            nsh_base::button()
                .test_id("trigger-edit-find")
                .on_press(shell_trigger_find)
                .child(text("Find")),
        )
        .child(nsh_app_devbase::app())
}

#[test]
fn edit_find_routes_focus_to_services_logs_or_command_palette() {
    let _turn = serial();
    install_all_fakes();
    let mut app = Headless::start(EngineConfig::new(Root::of::<FindHarness>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    app.press("role:button[Services]").unwrap();
    app.wait_for("id:services-search").unwrap();

    // On Services with the log drawer closed, Edit -> Find… focuses services-search.
    let search_id = app.screen().id("id:services-search").unwrap();
    app.press("id:trigger-edit-find").unwrap();
    assert!(
        app.screen()
            .commands()
            .iter()
            .any(|(id, cmd)| *id == search_id && matches!(cmd, nsh_core::Command::Focus)),
        "Edit -> Find… on Services should focus services-search"
    );
    assert!(!app.screen().matches("id:command-palette").unwrap());

    // When the live log drawer is open, Edit -> Find… focuses dev-log-filter instead.
    app.press("id:sidebar-logs-toggle").unwrap();
    app.wait_for("id:dev-log-filter").unwrap();
    let filter_id = app.screen().id("id:dev-log-filter").unwrap();
    app.press("id:trigger-edit-find").unwrap();
    assert!(
        app.screen()
            .commands()
            .iter()
            .any(|(id, cmd)| *id == filter_id && matches!(cmd, nsh_core::Command::Focus)),
        "Edit -> Find… with live log drawer open should focus dev-log-filter"
    );
    assert!(!app.screen().matches("id:command-palette").unwrap());

    // On a section without a local search bar (Settings) and logs closed,
    // Edit -> Find… opens the Command Palette.
    app.press("id:sidebar-logs-toggle").unwrap();
    app.wait_for_gone("id:dev-log-filter", Duration::from_secs(2))
        .unwrap();
    app.press("text:Settings").unwrap();
    app.wait_for("id:settings-page").unwrap();
    app.press("id:trigger-edit-find").unwrap();
    app.wait_for("id:command-palette").unwrap();
}

struct FakeShellGuidePersistence;

impl GuidePersistence for FakeShellGuidePersistence {
    fn completed(&self) -> Result<bool, String> {
        Ok(false)
    }
    fn complete(&self) -> Result<(), String> {
        Ok(())
    }
}

struct FakeShellGuideAccess;

impl GuideAccess for FakeShellGuideAccess {
    fn needs_access(&self) -> bool {
        true
    }
}

#[test]
fn app_launches_on_development_workspace_without_auto_guide() {
    let _turn = serial();
    install_all_fakes();
    install_guide_persistence(Arc::new(FakeShellGuidePersistence));
    install_guide_access(Arc::new(FakeShellGuideAccess));

    let mut config = EngineConfig::new(Root::of::<nsh_app_devbase::App>());
    config.environment.os = Os::MacOs;
    config.environment.title_bar.top = px(52.0);
    let mut app = Headless::start(config).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("id:development").unwrap();
    app.wait(Duration::from_millis(250)).unwrap();

    // Per docs/product-scope.md: no automatic welcome/Full Disk Access modal on launch.
    assert_eq!(app.screen().count("id:guide-overlay").unwrap(), 0);
}

struct FakeShellSettings {
    snapshot: SystemSettingsSnapshot,
}

impl SystemSettingsProvider for FakeShellSettings {
    fn snapshot(&self) -> Result<SystemSettingsSnapshot, String> {
        Ok(self.snapshot.clone())
    }
    fn install_helper(&self) -> SystemHelperChange {
        SystemHelperChange::default()
    }
    fn uninstall_helper(&self) -> SystemHelperChange {
        SystemHelperChange::default()
    }
    fn install_cli(&self) -> SystemCliChange {
        SystemCliChange::default()
    }
    fn uninstall_cli(&self) -> SystemCliChange {
        SystemCliChange::default()
    }
    fn clear_cache(&self) -> Result<SystemCacheClear, String> {
        Err("unused".to_string())
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
    fn preview_shutdown(&self) -> Result<ShutdownPlan, String> {
        Ok(ShutdownPlan::default())
    }
    fn shutdown(&self, _plan: &ShutdownPlan) -> Result<(), String> {
        Ok(())
    }
}

struct FakeShellDevelopment {
    snapshot: DevelopmentSnapshot,
}

impl DevelopmentProvider for FakeShellDevelopment {
    fn snapshot(&self) -> Result<DevelopmentSnapshot, String> {
        Ok(self.snapshot.clone())
    }
    fn node_releases(&self, _refresh: bool) -> Result<DevelopmentReleases, String> {
        Ok(DevelopmentReleases::default())
    }
    fn set_node_default(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    fn set_java_default(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    fn node_binary(&self, _id: &str) -> Result<String, String> {
        Err("unused".to_string())
    }
    fn install_python(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }
    fn python_action(&self, _key: &str, _action: &str) -> Result<(), String> {
        Ok(())
    }
    fn set_uv_path(&self, _path: &str) -> Result<(), String> {
        Ok(())
    }
    fn java_catalog(&self) -> Result<Vec<Package>, String> {
        Ok(Vec::new())
    }
    fn install_java(&self, _package: Package) -> Result<(), String> {
        Ok(())
    }
    fn node_shell(&self, _enable: bool) -> Result<(), String> {
        Ok(())
    }
    fn java_shell(&self, _enable: bool) -> Result<(), String> {
        Ok(())
    }
    fn select_node(
        &self,
        _dir: &str,
        _override_spec: Option<&str>,
    ) -> Result<DevelopmentSelection, String> {
        Ok(DevelopmentSelection::default())
    }
    fn launch_terminal(&self, _selection: &DevelopmentSelection) -> Result<(), String> {
        Ok(())
    }
}

#[component]
fn ReadinessHarness() -> impl IntoElement {
    menubar_load();
    development_load();
    view().size_full().flex_col().child(nsh_app_devbase::app())
}

#[test]
fn readiness_checklist_surfaces_actionable_pills_navigates_and_dismisses() {
    let _turn = serial();
    install_all_fakes();

    install_system_settings_provider(Arc::new(FakeShellSettings {
        snapshot: SystemSettingsSnapshot {
            helper: SystemHelperStatus {
                installed: false,
                message: "Not installed".to_string(),
            },
            cli: SystemCliStatus {
                installed: false,
                wrapper_path: "/usr/local/bin/devbase".to_string(),
                ..SystemCliStatus::default()
            },
            workspace_root: "/tmp/devbase-tests/Development".to_string(),
            ..SystemSettingsSnapshot::default()
        },
    }));
    install_proxy_provider(Arc::new(FakeShellProxy {
        snapshot: ProxySnapshot {
            rules: vec![ProxyRule {
                domain: "api.devbase.localhost".to_string(),
                local_port: 8080,
                ..ProxyRule::default()
            }],
            listener_port: 8443,
            ..ProxySnapshot::default()
        },
        trust: TrustState::NotTrusted,
    }));
    install_development_provider(Arc::new(FakeShellDevelopment {
        snapshot: DevelopmentSnapshot {
            node: vec![DevelopmentNode {
                id: "node@22.14.0-arm64".to_string(),
                version: "22.14.0".to_string(),
                lts: "Jod".to_string(),
                default: true,
            }],
            java: vec![DevelopmentJava {
                id: "java@21".to_string(),
                version: "21".to_string(),
                product: "JBR".to_string(),
                default: false,
            }],
            shells: vec![DevelopmentShell {
                shell: "zsh".to_string(),
                label: "Zsh".to_string(),
                available: true,
                files: vec!["~/.zshrc".to_string()],
                node_applied: false,
                java_applied: false,
            }],
            node_shell_enabled: false,
            java_shell_enabled: false,
            ..DevelopmentSnapshot::default()
        },
    }));

    let mut app = Headless::start(EngineConfig::new(Root::of::<ReadinessHarness>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();

    app.wait_for("id:readiness-bar").unwrap();
    app.wait_for("id:readiness-pill-helper").unwrap();
    app.wait_for("id:readiness-pill-ca-trust").unwrap();
    app.wait_for("id:readiness-pill-cli").unwrap();
    app.wait_for("id:readiness-pill-shell").unwrap();

    // Clicking Local CA Trust navigates to HTTPS Proxy and opens Authority.
    app.press("id:readiness-pill-ca-trust").unwrap();
    app.wait_for("id:ca-page").unwrap();

    // Clicking Privileged Helper navigates to Settings.
    app.press("id:readiness-pill-helper").unwrap();
    app.wait_for("id:settings-page").unwrap();

    // Clicking Shell Integration navigates to Development -> Shell integration tab.
    app.press("id:readiness-pill-shell").unwrap();
    app.wait_for("id:shell-row-zsh").unwrap();

    // Dismissing hides the strip for the session.
    app.press("id:readiness-dismiss").unwrap();
    app.wait_for_gone("id:readiness-bar", Duration::from_secs(2))
        .unwrap();
}

#[test]
fn readiness_checklist_auto_hides_when_all_items_are_ready() {
    let _turn = serial();
    install_all_fakes();

    install_system_settings_provider(Arc::new(FakeShellSettings {
        snapshot: SystemSettingsSnapshot {
            helper: SystemHelperStatus {
                installed: true,
                message: "Installed".to_string(),
            },
            cli: SystemCliStatus {
                installed: true,
                wrapper_path: "/usr/local/bin/devbase".to_string(),
                in_path: true,
                managed_by_devbase: true,
                ..SystemCliStatus::default()
            },
            workspace_root: "/tmp/devbase-tests/Development".to_string(),
            ..SystemSettingsSnapshot::default()
        },
    }));
    install_proxy_provider(Arc::new(FakeShellProxy {
        snapshot: ProxySnapshot {
            rules: vec![ProxyRule {
                domain: "api.devbase.localhost".to_string(),
                local_port: 8080,
                ..ProxyRule::default()
            }],
            listener_port: 8443,
            ..ProxySnapshot::default()
        },
        trust: TrustState::Trusted,
    }));
    install_development_provider(Arc::new(FakeShellDevelopment {
        snapshot: DevelopmentSnapshot {
            node: vec![DevelopmentNode {
                id: "node@22.14.0-arm64".to_string(),
                version: "22.14.0".to_string(),
                lts: "Jod".to_string(),
                default: true,
            }],
            shells: vec![DevelopmentShell {
                shell: "zsh".to_string(),
                label: "Zsh".to_string(),
                available: true,
                files: vec!["~/.zshrc".to_string()],
                node_applied: true,
                java_applied: false,
            }],
            node_shell_enabled: true,
            java_shell_enabled: false,
            ..DevelopmentSnapshot::default()
        },
    }));

    let mut app = Headless::start(EngineConfig::new(Root::of::<ReadinessHarness>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("id:development").unwrap();
    app.wait(Duration::from_millis(250)).unwrap();

    assert_eq!(app.screen().count("id:readiness-bar").unwrap(), 0);
}
