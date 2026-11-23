//! Headless integration tests for the dedicated Ports screen and its
//! integrations with Databases, HTTPS Proxy, and watched-port notifications.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use chrono::Utc;
use devcore::{
    databases::{
        BackupFile, ConnectionInfo, DatabaseWorkInfo, InstanceAction, InstanceCatalog,
        InstanceDeletionInfo, InstanceDeletionPreview, InstanceInitializationInfo, ListeningPort,
        PortCategory, PortInspection, PortListener, PortOwner, SignalOutcome,
    },
    model::{ConnectionProfile, Instance, ProxyRule},
};
use nsh_app_devbase::{
    App,
    testing::{
        AuthorityAction, AuthorityInfo, AuthorityStatus, CancelToken, DatabasesProvider,
        DatabasesSnapshot, ProxyHttpResult, ProxyListeningPort, ProxyProvider,
        ProxySavedCloudflare, ProxySnapshot, SystemPortsProvider, TrustState,
        install_databases_provider, install_proxy_provider, install_system_ports_provider,
        use_fake_databases, use_fake_dev_logs, use_fake_development, use_fake_editor,
        use_fake_guide, use_fake_menubar, use_fake_providers, use_fake_proxy, use_fake_software,
        use_fake_system, use_fake_updater,
    },
};
use nsh_core::prelude::{Size, px};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct FakePorts {
    ports: Mutex<Vec<ListeningPort>>,
    favorites: Mutex<BTreeSet<i64>>,
    watched: Mutex<BTreeSet<i64>>,
    stubborn_pids: Mutex<BTreeSet<i64>>,
    kills: Mutex<Vec<(i64, bool)>>,
}

impl FakePorts {
    fn new(ports: Vec<ListeningPort>) -> Arc<Self> {
        Arc::new(FakePorts {
            ports: Mutex::new(ports),
            favorites: Mutex::new(BTreeSet::new()),
            watched: Mutex::new(BTreeSet::new()),
            stubborn_pids: Mutex::new(BTreeSet::new()),
            kills: Mutex::new(Vec::new()),
        })
    }

    fn set_ports(&self, next: Vec<ListeningPort>) {
        *self.ports.lock().unwrap() = next;
    }

    fn mark_stubborn(&self, pid: i64) {
        self.stubborn_pids.lock().unwrap().insert(pid);
    }

    fn kills(&self) -> Vec<(i64, bool)> {
        self.kills.lock().unwrap().clone()
    }
}

impl SystemPortsProvider for FakePorts {
    fn inspect(&self, port: i64) -> Result<PortInspection, String> {
        let entry = self
            .ports
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.port == port)
            .cloned();
        match entry {
            Some(p) => Ok(PortInspection {
                port,
                status: "In use".to_string(),
                checked_at: "2026-11-23T12:00:00Z".to_string(),
                listeners: vec![PortListener {
                    pid: p.pid.unwrap_or(0),
                    uid: p.uid.unwrap_or(-1),
                    name: p.process,
                    address: format!("{}:{}", p.address, p.port),
                    own: false,
                }],
                truncated: false,
                detail: String::new(),
            }),
            None => Ok(PortInspection {
                port,
                status: "Available".to_string(),
                checked_at: "2026-11-23T12:00:00Z".to_string(),
                listeners: Vec::new(),
                truncated: false,
                detail: String::new(),
            }),
        }
    }

    fn available_local_port(&self) -> Result<i64, String> {
        Ok(41234)
    }

    fn listening(&self) -> Result<Vec<ListeningPort>, String> {
        let favs = self.favorites.lock().unwrap().clone();
        let watched = self.watched.lock().unwrap().clone();
        let mut list = self.ports.lock().unwrap().clone();
        for p in &mut list {
            p.starred = favs.contains(&p.port);
            p.watched = watched.contains(&p.port);
        }
        Ok(list)
    }

    fn favorite_ports(&self) -> Result<Vec<i64>, String> {
        Ok(self.favorites.lock().unwrap().iter().copied().collect())
    }

    fn watched_ports(&self) -> Result<Vec<i64>, String> {
        Ok(self.watched.lock().unwrap().iter().copied().collect())
    }

    fn toggle_starred(&self, port: i64) -> Result<(bool, Vec<i64>), String> {
        let mut favs = self.favorites.lock().unwrap();
        let now = if favs.contains(&port) {
            favs.remove(&port);
            false
        } else {
            favs.insert(port);
            true
        };
        Ok((now, favs.iter().copied().collect()))
    }

    fn toggle_watched(&self, port: i64) -> Result<(bool, Vec<i64>), String> {
        let mut w = self.watched.lock().unwrap();
        let now = if w.contains(&port) {
            w.remove(&port);
            false
        } else {
            w.insert(port);
            true
        };
        Ok((now, w.iter().copied().collect()))
    }

    fn terminate_pid(&self, pid: i64, force: bool) -> Result<SignalOutcome, String> {
        self.kills.lock().unwrap().push((pid, force));
        if !force && self.stubborn_pids.lock().unwrap().contains(&pid) {
            return Ok(SignalOutcome::StillRunning);
        }
        self.ports.lock().unwrap().retain(|p| p.pid != Some(pid));
        Ok(SignalOutcome::Stopped)
    }

    fn current_pid(&self) -> i64 {
        9000
    }

    fn current_uid(&self) -> i64 {
        501
    }
}

struct FakeDatabases {
    actions: Mutex<Vec<(String, InstanceAction)>>,
}

impl FakeDatabases {
    fn new() -> Arc<Self> {
        Arc::new(FakeDatabases {
            actions: Mutex::new(Vec::new()),
        })
    }

    fn actions(&self) -> Vec<(String, InstanceAction)> {
        self.actions.lock().unwrap().clone()
    }
}

impl DatabasesProvider for FakeDatabases {
    fn snapshot(&self) -> Result<DatabasesSnapshot, String> {
        Ok(DatabasesSnapshot {
            instances: vec![Instance {
                id: "pg17".to_string(),
                kind: "postgresql".to_string(),
                package: "postgresql@17.4-arm64".to_string(),
                port: 5432,
                persistent: false,
                ..Instance::default()
            }],
            statuses: BTreeMap::from([("instance:pg17".to_string(), "Running".to_string())]),
            ..DatabasesSnapshot::default()
        })
    }

    fn next_port(&self) -> Result<i64, String> {
        Ok(15432)
    }

    fn inspect_port(&self, port: i64) -> Result<PortInspection, String> {
        Ok(PortInspection {
            port,
            status: "Available".to_string(),
            checked_at: "2026-11-23T12:00:00Z".to_string(),
            listeners: Vec::new(),
            truncated: false,
            detail: String::new(),
        })
    }

    fn create(&self, _instance: Instance, _cancel: CancelToken) -> Result<(), String> {
        Ok(())
    }

    fn action(&self, id: &str, action: InstanceAction, _cancel: CancelToken) -> Result<(), String> {
        self.actions.lock().unwrap().push((id.to_string(), action));
        Ok(())
    }

    fn catalog(&self, _id: &str, _cancel: CancelToken) -> Result<InstanceCatalog, String> {
        Ok(InstanceCatalog::default())
    }

    fn create_database(&self, _id: &str, _name: &str, _cancel: CancelToken) -> Result<(), String> {
        Ok(())
    }

    fn create_user(
        &self,
        _id: &str,
        _user: &str,
        _password: &str,
        _database: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Ok(())
    }

    fn drop_database(&self, _id: &str, _name: &str, _cancel: CancelToken) -> Result<(), String> {
        Ok(())
    }

    fn drop_user(
        &self,
        _id: &str,
        _user: &str,
        _database: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Ok(())
    }

    fn backups(&self, _id: &str, _cancel: CancelToken) -> Result<Vec<BackupFile>, String> {
        Ok(Vec::new())
    }

    fn backup(&self, _id: &str, _database: &str, _cancel: CancelToken) -> Result<String, String> {
        Ok(String::new())
    }

    fn restore(
        &self,
        _id: &str,
        _database: &str,
        _path: &str,
        _cancel: CancelToken,
    ) -> Result<(), String> {
        Ok(())
    }

    fn delete_backup(&self, _id: &str, _name: &str, _cancel: CancelToken) -> Result<(), String> {
        Ok(())
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
        Ok(())
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

struct FakeProxy;

impl ProxyProvider for FakeProxy {
    fn snapshot(&self) -> Result<ProxySnapshot, String> {
        Ok(ProxySnapshot {
            listener_port: 443,
            revision: 1,
            publication: "Not published".to_string(),
            ..ProxySnapshot::default()
        })
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
        Ok(AuthorityStatus {
            checked_at: Utc::now(),
            active: AuthorityInfo {
                trust: TrustState::NotChecked,
                ..AuthorityInfo::default()
            },
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

fn sample_ports() -> Vec<ListeningPort> {
    vec![
        ListeningPort {
            port: 443,
            process: "devbase".to_string(),
            pid: Some(120),
            uid: Some(0),
            address: "127.0.0.1".to_string(),
            command: "/Applications/Devbase.app/Contents/MacOS/devbase --dev serve".to_string(),
            category: PortCategory::Web,
            owner: PortOwner::DevbaseProxy,
            starred: false,
            watched: false,
        },
        ListeningPort {
            port: 5173,
            process: "vite".to_string(),
            pid: Some(4100),
            uid: Some(501),
            address: "127.0.0.1".to_string(),
            command: "node /workspace/web/node_modules/.bin/vite --port 5173".to_string(),
            category: PortCategory::Dev,
            owner: PortOwner::External,
            starred: false,
            watched: false,
        },
        ListeningPort {
            port: 5432,
            process: "postgres".to_string(),
            pid: Some(4200),
            uid: Some(501),
            address: "127.0.0.1".to_string(),
            command: "/opt/homebrew/opt/postgresql@17/bin/postgres".to_string(),
            category: PortCategory::Database,
            owner: PortOwner::DevbaseDatabase {
                id: "pg17".to_string(),
                engine: "postgresql".to_string(),
            },
            starred: false,
            watched: false,
        },
        ListeningPort {
            port: 22,
            process: "sshd".to_string(),
            pid: Some(250),
            uid: Some(0),
            address: "0.0.0.0".to_string(),
            command: "/usr/sbin/sshd -D".to_string(),
            category: PortCategory::System,
            owner: PortOwner::External,
            starred: false,
            watched: false,
        },
    ]
}

fn start(
    ports: Vec<ListeningPort>,
) -> (
    MutexGuard<'static, ()>,
    Headless,
    Arc<FakePorts>,
    Arc<FakeDatabases>,
) {
    let turn = serial();
    use_fake_menubar();
    use_fake_providers();
    use_fake_software();
    use_fake_development();
    use_fake_databases();
    use_fake_proxy();
    use_fake_guide();
    use_fake_dev_logs();
    use_fake_editor();
    use_fake_updater();
    use_fake_system();

    let fake_ports = FakePorts::new(ports);
    let fake_dbs = FakeDatabases::new();
    install_system_ports_provider(fake_ports.clone() as Arc<dyn SystemPortsProvider>);
    install_databases_provider(fake_dbs.clone() as Arc<dyn DatabasesProvider>);
    install_proxy_provider(Arc::new(FakeProxy) as Arc<dyn ProxyProvider>);

    let mut app = Headless::start(EngineConfig::new(Root::of::<App>())).unwrap();
    app.set_viewport(Size::new(px(1200.0), px(820.0))).unwrap();
    app.wait_for("Ports").unwrap();
    app.press("role:button[Ports]").unwrap();
    app.wait_for("id:ports-page").unwrap();
    (turn, app, fake_ports, fake_dbs)
}

#[test]
fn ports_page_filters_searches_and_pins_starred_ports() {
    let (_turn, mut app, _ports, _dbs) = start(sample_ports());

    app.wait_for("id:port-row-5173").unwrap();
    app.wait_for("id:port-row-5432").unwrap();
    app.wait_for("id:port-row-443").unwrap();
    app.wait_for("id:port-row-22").unwrap();

    // Category filter: Database shows only :5432.
    app.press("id:ports-filter-database").unwrap();
    app.wait_for("id:port-row-5432").unwrap();
    assert!(!app.screen().matches("id:port-row-5173").unwrap());

    // Back to All and search by process/port.
    app.press("id:ports-filter-all").unwrap();
    app.type_text("id:ports-search", "vite").unwrap();
    app.wait_for("id:port-row-5173").unwrap();
    assert!(!app.screen().matches("id:port-row-5432").unwrap());
    app.clear("id:ports-search").unwrap();

    // Star :5173 and verify Starred tab shows it.
    app.press("id:port-star-5173").unwrap();
    app.wait(Duration::from_millis(120)).unwrap();
    app.press("id:ports-filter-starred").unwrap();
    app.wait_for("id:port-row-5173").unwrap();
    assert!(!app.screen().matches("id:port-row-5432").unwrap());
}

#[test]
fn ports_stop_routes_devbase_db_and_promotes_stubborn_external_pid_to_force_kill() {
    let (_turn, mut app, fake_ports, fake_dbs) = start(sample_ports());
    fake_ports.mark_stubborn(4100);

    app.wait_for("id:port-row-5173").unwrap();
    app.wait_for("id:port-row-5432").unwrap();

    // Root/system PID :22 shows Protected and cannot be killed.
    assert!(app.screen().shows("Protected"));

    // Stopping Devbase DB on :5432 routes to DatabasesProvider::action(Stop), never raw kill.
    app.press("id:port-stop-5432").unwrap();
    app.wait(Duration::from_millis(150)).unwrap();
    assert!(
        fake_dbs
            .actions()
            .iter()
            .any(|(id, action)| id == "pg17" && *action == InstanceAction::Stop),
        "expected Devbase DB stop action, got {:?}",
        fake_dbs.actions()
    );
    assert!(fake_ports.kills().is_empty());

    // Stopping stubborn external PID 4100 sends SIGTERM first, then morphs to Force kill (SIGKILL).
    app.press("id:port-stop-5173").unwrap();
    app.wait_for("Force kill").unwrap();
    assert_eq!(fake_ports.kills(), vec![(4100, false)]);

    app.press("id:port-stop-5173").unwrap();
    app.wait_for_gone("id:port-row-5173", Duration::from_secs(2))
        .unwrap();
    assert_eq!(fake_ports.kills(), vec![(4100, false), (4100, true)]);
}

#[test]
fn watched_port_emits_transition_event_when_listener_closes() {
    let (_turn, mut app, fake_ports, _dbs) = start(sample_ports());
    app.wait_for("id:port-row-5173").unwrap();

    // Watch port 5173.
    app.press("id:port-watch-5173").unwrap();
    app.wait(Duration::from_millis(150)).unwrap();

    // Simulate the process on 5173 exiting, then click Refresh in the UI.
    fake_ports.set_ports(
        sample_ports()
            .into_iter()
            .filter(|p| p.port != 5173)
            .collect(),
    );
    app.press("id:ports-refresh").unwrap();
    app.wait_for_gone("id:port-row-5173", Duration::from_secs(2))
        .unwrap();

    // Offline saved ports card and transition toast both report the closed watched port.
    app.wait_for("id:port-offline-5173").unwrap();
    app.wait_for("text:/Port 5173 is no longer listening/")
        .unwrap();
}

#[test]
fn add_https_handoff_prefills_rule_draft_in_https_proxy() {
    let (_turn, mut app, _fake_ports, _dbs) = start(sample_ports());
    app.wait_for("id:port-add-https-5173").unwrap();

    app.press("id:port-add-https-5173").unwrap();
    // Should switch to HTTPS Proxy and prefill a draft rule pointing at 5173.
    app.wait_for("id:proxy-domain-0").unwrap();
    assert_eq!(
        app.screen().input_text("id:proxy-domain-0").unwrap(),
        Some("vite")
    );
    assert_eq!(
        app.screen().input_text("id:proxy-port-0").unwrap(),
        Some("5173")
    );
}
