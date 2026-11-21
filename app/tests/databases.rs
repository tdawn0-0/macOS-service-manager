//! The databases workspace, headless: the instance list with its engines,
//! states and ports, creation with its port validation and busy state, the
//! lifecycle actions and their errors, the connection panel, backups and
//! restores with their confirmations, and permanent deletion with the typed
//! ID that keeps external data. A fake provider stands in for devcore: no
//! test runs launchd, a database binary or reads a real credential.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, UNIX_EPOCH},
};

use devcore::{
    databases::{
        BackupFile, ConnectionInfo, DatabaseWorkInfo, InstanceAction, InstanceCatalog,
        InstanceDeletionInfo, InstanceDeletionPreview, InstanceInitializationInfo, PortInspection,
    },
    model::{ConnectionProfile, Instance, Package},
};
use nsh_core::{Checked, prelude::*};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

use nsh_app_devbase::testing::{
    DatabasesProvider, DatabasesSnapshot, install_databases_provider, use_fake_databases,
};

/// The database tests share one fake-provider registry, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Normal,
    /// Every creation and action takes this long, so the busy state shows.
    Slow,
    /// The lifecycle action fails the way a refused launchctl call does.
    ActionError,
    /// The deletion review describes internal data that may be erased.
    InternalDeletion,
    /// The deletion review describes external data that is retained.
    ExternalDeletion,
}

#[derive(Clone, Default)]
struct Fixture {
    instances: Vec<Instance>,
    packages: BTreeMap<String, Package>,
    profiles: BTreeMap<String, ConnectionProfile>,
    statuses: BTreeMap<String, String>,
    backups: BTreeMap<String, Vec<BackupFile>>,
    deletions: Vec<InstanceDeletionInfo>,
    initializations: Vec<InstanceInitializationInfo>,
    work: Vec<DatabaseWorkInfo>,
}

type Sink = Mutex<Option<Arc<dyn Fn(String) + Send + Sync>>>;

struct FakeDatabases {
    fixture: Mutex<Fixture>,
    calls: Mutex<Vec<String>>,
    sink: Sink,
    behaviour: Behaviour,
}

impl FakeDatabases {
    fn new(fixture: Fixture, behaviour: Behaviour) -> Arc<Self> {
        Arc::new(FakeDatabases {
            fixture: Mutex::new(fixture),
            calls: Mutex::new(Vec::new()),
            sink: Mutex::new(None),
            behaviour,
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

    fn progress(&self, message: &str) {
        let sink = self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(sink) = sink {
            sink(message.to_string());
        }
    }

    fn slow(&self) {
        if self.behaviour == Behaviour::Slow {
            thread::sleep(Duration::from_millis(600));
        }
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
            packages: fixture.packages.clone(),
            profiles: fixture.profiles.clone(),
            statuses: fixture.statuses.clone(),
            upgrades: BTreeMap::new(),
        })
    }

    fn set_progress_sink(&self, sink: Arc<dyn Fn(String) + Send + Sync>) {
        *self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
    }

    fn next_port(&self) -> Result<i64, String> {
        self.record("next port".to_string());
        Ok(55432)
    }

    fn inspect_port(&self, port: i64) -> Result<PortInspection, String> {
        self.record(format!("inspect port {port}"));
        Ok(PortInspection {
            port,
            status: "Available".to_string(),
            checked_at: "2026-11-20T12:00:00Z".to_string(),
            listeners: Vec::new(),
            truncated: false,
            detail: String::new(),
        })
    }

    fn create(
        &self,
        instance: Instance,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!(
            "create {} {} {} {} {}",
            instance.id, instance.kind, instance.package, instance.port, instance.data_directory
        ));
        self.progress("Preparing the data directory");
        self.slow();
        let mut fixture = self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        fixture
            .statuses
            .insert(instance.id.clone(), "Stopped".to_string());
        fixture.instances.push(instance);
        Ok(())
    }

    fn action(
        &self,
        id: &str,
        action: InstanceAction,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("{} {id}", action.as_str()));
        if self.behaviour == Behaviour::ActionError {
            return Err("launchctl failed: permission denied".to_string());
        }
        if self.behaviour == Behaviour::Slow {
            let phase = match action {
                InstanceAction::Start => "Starting",
                InstanceAction::Stop | InstanceAction::Remove => "Stopping",
                InstanceAction::Restart => "Restarting",
            };
            self.progress(&format!("{phase} database {id}"));
            thread::sleep(Duration::from_millis(600));
        }
        let mut fixture = self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match action {
            InstanceAction::Start | InstanceAction::Restart => {
                fixture
                    .statuses
                    .insert(id.to_string(), "Running".to_string());
            }
            InstanceAction::Stop => {
                fixture
                    .statuses
                    .insert(id.to_string(), "Stopped".to_string());
            }
            InstanceAction::Remove => {
                fixture.instances.retain(|instance| instance.id != id);
                fixture.statuses.remove(id);
            }
        }
        Ok(())
    }

    fn catalog(
        &self,
        _id: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<InstanceCatalog, String> {
        Ok(InstanceCatalog {
            databases: vec!["app".to_string(), "postgres".to_string()],
            users: vec!["app_user@postgres".to_string()],
        })
    }

    fn create_database(
        &self,
        id: &str,
        name: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("create database {id} {name}"));
        Ok(())
    }

    fn create_user(
        &self,
        id: &str,
        user: &str,
        password: &str,
        database: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!(
            "create user {id} {user} {} {database}",
            if password.is_empty() { "empty" } else { "set" }
        ));
        Ok(())
    }

    fn drop_database(
        &self,
        id: &str,
        name: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("drop database {id} {name}"));
        Ok(())
    }

    fn drop_user(
        &self,
        id: &str,
        user: &str,
        database: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("drop user {id} {user} {database}"));
        Ok(())
    }

    fn backups(
        &self,
        id: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Vec<BackupFile>, String> {
        Ok(self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .backups
            .get(id)
            .cloned()
            .unwrap_or_default())
    }

    fn backup(
        &self,
        id: &str,
        database: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<String, String> {
        self.record(format!("backup {id} {database}"));
        let path = format!("/tmp/devbase-tests/backups/{id}-{database}-0123456789abcdef.dump");
        let row = BackupFile {
            name: format!("{id}-{database}-0123456789abcdef.dump"),
            path: PathBuf::from("/tmp/devbase-tests/backups")
                .join(format!("{id}-{database}-0123456789abcdef.dump")),
            database: database.to_string(),
            size: 2048,
            modified: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        };
        self.fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .backups
            .entry(id.to_string())
            .or_default()
            .push(row);
        Ok(path)
    }

    fn restore(
        &self,
        id: &str,
        database: &str,
        path: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("restore {id} {database} {path}"));
        Ok(())
    }

    fn delete_backup(
        &self,
        id: &str,
        name: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("delete backup {id} {name}"));
        let mut fixture = self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(rows) = fixture.backups.get_mut(id) {
            rows.retain(|row| row.name != name);
        }
        Ok(())
    }

    fn save_profile(&self, profile: ConnectionProfile) -> Result<ConnectionProfile, String> {
        self.record(format!(
            "save profile {} {} {}",
            profile.instance_id, profile.name, profile.database
        ));
        let mut saved = profile;
        if saved.id.is_empty() {
            saved.id = "profile-1".to_string();
        }
        self.fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .profiles
            .insert(saved.id.clone(), saved.clone());
        Ok(saved)
    }

    fn delete_profile(&self, instance_id: &str, profile_id: &str) -> Result<(), String> {
        self.record(format!("delete profile {instance_id} {profile_id}"));
        self.fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .profiles
            .remove(profile_id);
        Ok(())
    }

    fn connection_info(
        &self,
        instance_id: &str,
        profile_id: &str,
    ) -> Result<ConnectionInfo, String> {
        self.record(format!("connection {instance_id} {profile_id}"));
        let fixture = self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let instance = fixture
            .instances
            .iter()
            .find(|instance| instance.id == instance_id)
            .ok_or_else(|| "database instance not found".to_string())?;
        if profile_id.is_empty() {
            Ok(ConnectionInfo {
                uri: format!(
                    "postgresql://devmanager@127.0.0.1:{}/postgres",
                    instance.port
                ),
                username: "devmanager".to_string(),
                credential_file: format!("/tmp/devbase-tests/instances/{instance_id}/password"),
            })
        } else {
            let profile = fixture
                .profiles
                .get(profile_id)
                .ok_or_else(|| "connection profile not found".to_string())?;
            Ok(ConnectionInfo {
                uri: format!(
                    "postgresql://{}@127.0.0.1:{}/{}",
                    profile.username, instance.port, profile.database
                ),
                username: profile.username.clone(),
                credential_file: String::new(),
            })
        }
    }

    fn connection_details(&self, id: &str) -> Result<String, String> {
        self.record(format!("connection details {id}"));
        Ok(format!(
            "postgresql://devmanager@127.0.0.1:5432/postgres\nUsername: devmanager\nPassword file: /tmp/devbase-tests/instances/{id}/password"
        ))
    }

    fn password(&self, id: &str) -> Result<String, String> {
        self.record(format!("password {id}"));
        Ok("secret-not-in-the-store".to_string())
    }

    fn open_client(&self, uri: &str) -> Result<(), String> {
        self.record(format!("open client {uri}"));
        Ok(())
    }

    fn open_cli(
        &self,
        id: &str,
        profile_id: &str,
        database: Option<&str>,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!(
            "open cli {id} {profile_id} {}",
            database.unwrap_or("")
        ));
        Ok(())
    }

    fn upgrade(
        &self,
        instance: &Instance,
        _target: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("upgrade {}", instance.id));
        Ok(())
    }

    fn preview_deletion(&self, id: &str) -> Result<InstanceDeletionPreview, String> {
        self.record(format!("preview deletion {id}"));
        let external = self.behaviour == Behaviour::ExternalDeletion;
        Ok(InstanceDeletionPreview {
            identity: "dev:ino".to_string(),
            tree_digest: "digest".to_string(),
            instance_id: id.to_string(),
            kind: "postgresql".to_string(),
            package: "postgresql@17.11-arm64".to_string(),
            path: if external {
                PathBuf::from("/Volumes/Work/devbase-data/app")
            } else {
                PathBuf::from(format!("/tmp/devbase-tests/instances/{id}"))
            },
            fingerprint: "fp-1".to_string(),
            detail: if external {
                "External data is retained. Remove the instance registration to keep this data location; permanent erasure is not supported for external data.".to_string()
            } else {
                "Permanently removes this instance record, data, local configuration and credentials. Installed software and backups outside this path are retained.".to_string()
            },
            entries: 12,
            bytes: 1_572_864,
            can_delete: !external,
        })
    }

    fn delete_instance_data(
        &self,
        id: &str,
        fingerprint: &str,
        confirmation: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("delete {id} {fingerprint} {confirmation}"));
        let mut fixture = self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        fixture.instances.retain(|instance| instance.id != id);
        fixture.statuses.remove(id);
        Ok(())
    }

    fn pending_deletions(&self) -> Result<Vec<InstanceDeletionInfo>, String> {
        Ok(self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .deletions
            .clone())
    }

    fn restore_deletion(
        &self,
        id: &str,
        revision: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("restore deletion {id} {revision}"));
        Ok(())
    }

    fn finish_deletion(
        &self,
        id: &str,
        revision: &str,
        confirmation: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        self.record(format!("finish deletion {id} {revision} {confirmation}"));
        Ok(())
    }

    fn pending_initializations(&self) -> Result<Vec<InstanceInitializationInfo>, String> {
        Ok(self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .initializations
            .clone())
    }

    fn resolve_initialization(
        &self,
        id: &str,
        revision: &str,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<String, String> {
        self.record(format!("resolve initialization {id} {revision}"));
        Ok("/tmp/devbase-tests/retained-initializations/init-1".to_string())
    }

    fn pending_work(&self) -> Result<Vec<DatabaseWorkInfo>, String> {
        Ok(self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .work
            .clone())
    }

    fn resolve_work(&self, id: &str, revision: &str) -> Result<(), String> {
        self.record(format!("resolve work {id} {revision}"));
        self.fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .work
            .retain(|row| row.id != id);
        Ok(())
    }
}

fn package(name: &str, version: &str) -> Package {
    Package {
        name: name.to_string(),
        version: version.to_string(),
        arch: "arm64".to_string(),
        url: format!("https://example.test/{name}/{version}.tar.gz"),
        sha256: "a".repeat(64),
        format: "tar.gz".to_string(),
        license: "MIT".to_string(),
        min_macos: "26.0".to_string(),
        ..Package::default()
    }
}

fn instance(id: &str, kind: &str, port: i64, data_directory: &str) -> Instance {
    Instance {
        data_identity: String::new(),
        data_directory: data_directory.to_string(),
        data_owner: String::new(),
        id: id.to_string(),
        kind: kind.to_string(),
        package: format!("{kind}@17.11-arm64"),
        port,
        persistent: true,
    }
}

fn fixture_with_packages() -> Fixture {
    let mut fixture = Fixture::default();
    for (name, version) in [
        ("postgresql", "17.11"),
        ("mysql", "8.4.13"),
        ("redis", "7.4.1"),
        ("mongodb", "8.0.4"),
    ] {
        let package = package(name, version);
        fixture.packages.insert(package.id(), package);
    }
    fixture
}

/// Installs the fake provider and starts the surface on the Databases page,
/// holding the tests' turn for as long as the returned guard lives.
fn start(fake: Arc<FakeDatabases>) -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    use_fake_databases();
    install_databases_provider(fake as Arc<dyn DatabasesProvider>);
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    app.press("role:button[Databases]").unwrap();
    app.wait_for("id:db-create-form").unwrap();
    app.wait_for("PostgreSQL · 17.11").unwrap();
    (turn, app)
}

fn with_instance(behaviour: Behaviour) -> Arc<FakeDatabases> {
    let mut fixture = fixture_with_packages();
    fixture
        .instances
        .push(instance("app", "postgresql", 5432, ""));
    fixture
        .statuses
        .insert("app".to_string(), "Stopped".to_string());
    FakeDatabases::new(fixture, behaviour)
}

#[test]
fn instances_list_their_engine_state_and_port() {
    let mut fixture = fixture_with_packages();
    fixture
        .instances
        .push(instance("app", "postgresql", 5432, ""));
    fixture.instances.push(instance("cache", "redis", 6379, ""));
    fixture
        .statuses
        .insert("app".to_string(), "Running".to_string());
    fixture
        .statuses
        .insert("cache".to_string(), "Stopped".to_string());
    let fake = FakeDatabases::new(fixture, Behaviour::Normal);
    let (_turn, mut app) = start(fake);

    app.wait_for("id:db-instance-app").unwrap();
    app.wait_for("PostgreSQL:5432").unwrap();
    app.wait_for("Redis:6379").unwrap();

    app.press("id:db-instance-app").unwrap();
    app.wait_for("127.0.0.1:5432").unwrap();
    assert!(app.screen().shows("Running"));
    assert!(app.screen().shows("PostgreSQL 17.11 · 127.0.0.1:5432"));

    app.press("id:db-instance-cache").unwrap();
    app.wait_for("127.0.0.1:6379").unwrap();
    assert!(app.screen().shows("Stopped"));
    assert!(!app.screen().shows("127.0.0.1:5432"));
}

#[test]
fn creating_an_instance_previews_the_port_and_shows_the_busy_state() {
    let fake = FakeDatabases::new(fixture_with_packages(), Behaviour::Slow);
    let (_turn, mut app) = start(fake.clone());

    app.type_text("id:db-name", "blog").unwrap();
    app.press("id:db-create").unwrap();
    app.wait_for("Preparing blog").unwrap();
    app.wait_for_within("Created blog", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:db-instance-blog").unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call.starts_with("create blog postgresql postgresql@17.11-arm64 55432"))
    );
}

#[test]
fn a_non_numeric_port_is_refused_with_the_go_message() {
    let fake = FakeDatabases::new(fixture_with_packages(), Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());

    app.type_text("id:db-name", "bad").unwrap();
    app.clear("id:db-port").unwrap();
    app.type_text("id:db-port", "not-a-port").unwrap();
    app.press("id:db-create").unwrap();
    app.wait_for("Error: Enter a valid port").unwrap();
    assert!(
        !fake.calls().iter().any(|call| call.starts_with("create ")),
        "an invalid port must not create an instance"
    );
}

#[test]
fn starting_and_stopping_show_the_phase_and_the_result() {
    let fake = with_instance(Behaviour::Slow);
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("id:db-start").unwrap();

    app.press("id:db-start").unwrap();
    app.wait_for("Starting").unwrap();
    app.wait_for_within("Started app", Duration::from_secs(5))
        .unwrap();
    app.wait(Duration::from_millis(400)).unwrap();

    app.press("id:db-stop").unwrap();
    app.wait_for("Stopping").unwrap();
    app.wait_for_within("Stopped app", Duration::from_secs(5))
        .unwrap();
    assert!(fake.calls().iter().any(|call| call == "start app"));
    assert!(fake.calls().iter().any(|call| call == "stop app"));
}

#[test]
fn a_failed_action_is_surfaced_verbatim() {
    let fake = with_instance(Behaviour::ActionError);
    let (_turn, mut app) = start(fake);
    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("id:db-start").unwrap();

    app.press("id:db-start").unwrap();
    app.wait_for(
        "Database operation did not complete.\nError: launchctl failed: permission denied",
    )
    .unwrap();
}

#[test]
fn permanent_deletion_refuses_external_data() {
    let fake = with_instance(Behaviour::ExternalDeletion);
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("id:db-review-deletion").unwrap();

    app.press("id:db-review-deletion").unwrap();
    app.wait_for("id:db-deletion-preview").unwrap();
    app.wait_for("/Volumes/Work/devbase-data/app").unwrap();
    app.wait_for("External data is retained. Remove the instance registration to keep this data location; permanent erasure is not supported for external data.")
        .unwrap();
    app.wait_for("Permanently delete instance data").unwrap();

    // The typed ID cannot make an external path deletable.
    app.type_text("id:deletion-confirm", "app").unwrap();
    let _ = app.press("id:deletion-delete");
    app.wait(Duration::from_millis(800)).unwrap();
    assert!(
        !fake.calls().iter().any(|call| call.starts_with("delete ")),
        "external data must never be erased"
    );
}

#[test]
fn permanent_deletion_needs_the_typed_id_and_its_fingerprint() {
    let fake = with_instance(Behaviour::InternalDeletion);
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("id:db-review-deletion").unwrap();
    app.press("id:db-review-deletion").unwrap();
    app.wait_for("/tmp/devbase-tests/instances/app").unwrap();
    app.wait_for("12 entries · 1.50 MiB logical file size")
        .unwrap();

    let _ = app.press("id:deletion-delete");
    app.wait(Duration::from_millis(500)).unwrap();
    assert!(!fake.calls().iter().any(|call| call.starts_with("delete ")));

    app.type_text("id:deletion-confirm", "wrong").unwrap();
    let _ = app.press("id:deletion-delete");
    app.wait(Duration::from_millis(500)).unwrap();
    assert!(!fake.calls().iter().any(|call| call.starts_with("delete ")));

    app.clear("id:deletion-confirm").unwrap();
    app.type_text("id:deletion-confirm", "app").unwrap();
    app.press("id:deletion-delete").unwrap();
    app.wait_for_within(
        "Database deletion status refreshed.",
        Duration::from_secs(4),
    )
    .unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "delete app fp-1 app")
    );
}

#[test]
fn a_backup_is_deleted_only_after_the_confirmation() {
    let mut fixture = fixture_with_packages();
    fixture
        .instances
        .push(instance("app", "postgresql", 5432, ""));
    fixture
        .statuses
        .insert("app".to_string(), "Running".to_string());
    fixture.backups.insert(
        "app".to_string(),
        vec![BackupFile {
            name: "app-app-0123456789abcdef.dump".to_string(),
            path: PathBuf::from("/tmp/devbase-tests/backups/app-app-0123456789abcdef.dump"),
            database: "app".to_string(),
            size: 2048,
            modified: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        }],
    );
    let fake = FakeDatabases::new(fixture, Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("id:db-delete-backup-app-app-0123456789abcdef.dump")
        .unwrap();

    // The default answer is Cancel.
    app.press("id:db-delete-backup-app-app-0123456789abcdef.dump")
        .unwrap();
    app.wait(Duration::from_millis(800)).unwrap();
    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| call.starts_with("delete backup"))
    );

    app.on_dialog(|_| DialogAnswer::Press("Delete Backup".to_string()));
    app.press("id:db-delete-backup-app-app-0123456789abcdef.dump")
        .unwrap();
    app.wait_for("Deleted backup").unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "delete backup app app-app-0123456789abcdef.dump")
    );
}

#[test]
fn restoring_asks_first_and_creates_the_new_database() {
    let mut fixture = fixture_with_packages();
    fixture
        .instances
        .push(instance("app", "postgresql", 5432, ""));
    fixture
        .statuses
        .insert("app".to_string(), "Running".to_string());
    fixture.backups.insert(
        "app".to_string(),
        vec![BackupFile {
            name: "app-app-0123456789abcdef.dump".to_string(),
            path: PathBuf::from("/tmp/devbase-tests/backups/app-app-0123456789abcdef.dump"),
            database: "app".to_string(),
            size: 2048,
            modified: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        }],
    );
    let fake = FakeDatabases::new(fixture, Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("id:db-use-backup-app-app-0123456789abcdef.dump")
        .unwrap();

    app.press("id:db-use-backup-app-app-0123456789abcdef.dump")
        .unwrap();

    app.press("id:db-restore").unwrap();
    app.wait(Duration::from_millis(800)).unwrap();
    assert!(!fake.calls().iter().any(|call| call.starts_with("restore ")));

    app.on_dialog(|_| DialogAnswer::Press("Restore Backup".to_string()));
    app.press("id:db-restore").unwrap();
    app.wait_for("Restored into app_restored").unwrap();
    assert!(fake.calls().iter().any(|call| call
        == "restore app app_restored /tmp/devbase-tests/backups/app-app-0123456789abcdef.dump"));
}

#[test]
fn the_connection_panel_copies_and_launches_without_storing_credentials() {
    let fake = with_instance(Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("id:connection-uri-copy").unwrap();

    app.press("id:connection-uri-copy").unwrap();
    app.wait_for("Connection URI copied").unwrap();
    assert!(
        fake.calls().iter().any(|call| call == "connection app ")
            || fake.calls().iter().any(|call| call == "connection app")
    );

    app.press("id:connection-password-copy").unwrap();
    app.wait_for("Database password copied").unwrap();
    assert!(fake.calls().iter().any(|call| call == "password app"));

    app.press("id:connection-cli").unwrap();
    app.wait_for("Opening CLI in Terminal").unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call.starts_with("open cli app "))
    );

    app.press("id:connection-details").unwrap();
    app.wait_for("text:/Password file: .*password/").unwrap();
}

#[test]
fn unfinished_lists_render_and_resolve() {
    let mut fixture = fixture_with_packages();
    fixture.work.push(DatabaseWorkInfo {
        id: "work-1".to_string(),
        instance_id: "app".to_string(),
        kind: "backup".to_string(),
        database: "app".to_string(),
        status: "Interrupted".to_string(),
        detail:
            "Review this task before changing the instance. Database and output files are retained."
                .to_string(),
        output: "/tmp/devbase-tests/backups/app-app-0123456789abcdef.dump".to_string(),
        revision: "rev-1".to_string(),
        caller_active: false,
        running: false,
        can_resolve: true,
    });
    fixture.deletions.push(InstanceDeletionInfo {
        tree_digest: String::new(),
        id: "deletion-1".to_string(),
        instance_id: "app".to_string(),
        kind: "postgresql".to_string(),
        original_path: "/tmp/devbase-tests/instances/app".to_string(),
        staged_path: "".to_string(),
        status: "Not moved".to_string(),
        detail: "Removal did not commit; the data is still in its original path.".to_string(),
        revision: "rev-2".to_string(),
        entries: 0,
        bytes: 0,
        can_erase: false,
        can_restore: true,
        busy: false,
    });
    let fake = FakeDatabases::new(fixture, Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());

    app.press("id:db-work").unwrap();
    app.wait_for("id:db-work-page").unwrap();
    app.wait_for("Finish database task work-1").unwrap();
    app.press("id:work-resolve-work-1").unwrap();
    app.wait_for("Database task status refreshed.").unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "resolve work work-1 rev-1")
    );

    app.press("id:db-deletions").unwrap();
    app.wait_for("Pending data deletions").unwrap();
    app.wait_for("Original data path deletion").unwrap();
    app.wait_for("Restore original data deletion").unwrap();
}

#[test]
fn detail_inputs_keep_their_node_ids_across_keystrokes_and_notices() {
    let mut fixture = fixture_with_packages();
    fixture
        .instances
        .push(instance("app", "postgresql", 5432, ""));
    fixture
        .statuses
        .insert("app".to_string(), "Running".to_string());
    let fake = FakeDatabases::new(fixture, Behaviour::Normal);
    let (_turn, mut app) = start(fake);

    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("id:profile-new").unwrap();
    app.press("id:profile-new").unwrap();
    app.wait_for("id:profile-name").unwrap();
    app.wait_for("id:db-database-name").unwrap();
    app.wait_for("id:db-database-user").unwrap();
    app.wait_for("id:db-database-password").unwrap();
    app.wait_for("id:db-backup-path").unwrap();
    app.wait_for("id:db-restore-target").unwrap();

    let profile_name_id = app.screen().id("id:profile-name").unwrap();
    let profile_db_id = app.screen().id("id:profile-database").unwrap();
    let db_name_id = app.screen().id("id:db-database-name").unwrap();
    let db_user_id = app.screen().id("id:db-database-user").unwrap();
    let db_password_id = app.screen().id("id:db-database-password").unwrap();
    let backup_path_id = app.screen().id("id:db-backup-path").unwrap();
    let restore_target_id = app.screen().id("id:db-restore-target").unwrap();

    // Keystrokes across every detail-pane input must never tear down or recreate the input node.
    app.type_text("id:profile-name", "Web").unwrap();
    assert_eq!(app.screen().id("id:profile-name").unwrap(), profile_name_id);
    app.type_text("id:profile-name", " App").unwrap();
    assert_eq!(app.screen().id("id:profile-name").unwrap(), profile_name_id);

    app.type_text("id:profile-database", "analytics").unwrap();
    assert_eq!(
        app.screen().id("id:profile-database").unwrap(),
        profile_db_id
    );

    app.type_text("id:db-database-name", "billing").unwrap();
    assert_eq!(app.screen().id("id:db-database-name").unwrap(), db_name_id);

    app.type_text("id:db-database-user", "billing_user")
        .unwrap();
    assert_eq!(app.screen().id("id:db-database-user").unwrap(), db_user_id);

    app.type_text("id:db-database-password", "pw123").unwrap();
    assert_eq!(
        app.screen().id("id:db-database-password").unwrap(),
        db_password_id
    );

    app.type_text("id:db-backup-path", "/tmp/snap.dump")
        .unwrap();
    assert_eq!(
        app.screen().id("id:db-backup-path").unwrap(),
        backup_path_id
    );

    app.type_text("id:db-restore-target", "billing_copy")
        .unwrap();
    assert_eq!(
        app.screen().id("id:db-restore-target").unwrap(),
        restore_target_id
    );

    // Saving a profile emits a notice and triggers refresh(); every detail input stays mounted.
    app.press("id:profile-save").unwrap();
    app.wait_for("Connection profile saved").unwrap();
    app.press("id:db-refresh").unwrap();
    app.wait(Duration::from_millis(200)).unwrap();

    assert_eq!(app.screen().id("id:profile-name").unwrap(), profile_name_id);
    assert_eq!(
        app.screen().id("id:profile-database").unwrap(),
        profile_db_id
    );
    assert_eq!(app.screen().id("id:db-database-name").unwrap(), db_name_id);
    assert_eq!(app.screen().id("id:db-database-user").unwrap(), db_user_id);
    assert_eq!(
        app.screen().id("id:db-database-password").unwrap(),
        db_password_id
    );
    assert_eq!(
        app.screen().id("id:db-backup-path").unwrap(),
        backup_path_id
    );
    assert_eq!(
        app.screen().id("id:db-restore-target").unwrap(),
        restore_target_id
    );
}

#[test]
fn rail_nav_rows_reactively_update_selection_and_unfinished_counts() {
    let mut fixture = fixture_with_packages();
    fixture
        .instances
        .push(instance("app", "postgresql", 5432, ""));
    fixture
        .statuses
        .insert("app".to_string(), "Stopped".to_string());
    fixture.work.push(DatabaseWorkInfo {
        id: "work-1".to_string(),
        instance_id: "app".to_string(),
        kind: "backup".to_string(),
        database: "app".to_string(),
        status: "Interrupted".to_string(),
        detail: "Interrupted backup".to_string(),
        output: String::new(),
        revision: "rev-1".to_string(),
        caller_active: false,
        running: false,
        can_resolve: true,
    });
    for i in 1..=2 {
        fixture.initializations.push(InstanceInitializationInfo {
            id: format!("init-{i}"),
            instance_id: format!("prep-{i}"),
            kind: "postgresql".to_string(),
            package: "postgresql@17.11-arm64".to_string(),
            port: 55430 + i,
            path: PathBuf::from(format!("/tmp/devbase-tests/init-{i}")),
            status: "Interrupted".to_string(),
            detail: "Interrupted initialization".to_string(),
            revision: format!("rev-init-{i}"),
            busy: false,
            can_resolve: true,
        });
    }
    fixture.deletions.push(InstanceDeletionInfo {
        tree_digest: String::new(),
        id: "deletion-1".to_string(),
        instance_id: "app".to_string(),
        kind: "postgresql".to_string(),
        original_path: "/tmp/devbase-tests/instances/app".to_string(),
        staged_path: String::new(),
        status: "Not moved".to_string(),
        detail: "Removal did not commit".to_string(),
        revision: "rev-del-1".to_string(),
        entries: 0,
        bytes: 0,
        can_erase: false,
        can_restore: true,
        busy: false,
    });

    let fake = FakeDatabases::new(fixture, Behaviour::Normal);
    let (_turn, mut app) = start(fake);

    // Initial selection is "New instance", and unfinished counts populate live after db::load().
    assert!(app.screen().selected("id:db-new").unwrap());
    assert!(!app.screen().selected("id:db-work").unwrap());
    assert!(!app.screen().selected("id:db-initializations").unwrap());
    assert!(!app.screen().selected("id:db-deletions").unwrap());

    app.wait_for("id:db-work >> text:1").unwrap();
    app.wait_for("id:db-initializations >> text:2").unwrap();
    app.wait_for("id:db-deletions >> text:1").unwrap();

    // Switching to Database tasks highlights it immediately; resolving updates the count to 0.
    app.press("id:db-work").unwrap();
    app.wait_for("id:db-work-page").unwrap();
    assert!(app.screen().selected("id:db-work").unwrap());
    assert!(!app.screen().selected("id:db-new").unwrap());

    app.press("id:work-resolve-work-1").unwrap();
    app.wait_for("id:db-work >> text:0").unwrap();

    // Switching across Initializations, Pending deletions, and back to New instance tracks selection.
    app.press("id:db-initializations").unwrap();
    app.wait_for("id:db-initializations-page").unwrap();
    assert!(app.screen().selected("id:db-initializations").unwrap());
    assert!(!app.screen().selected("id:db-work").unwrap());

    app.press("id:db-deletions").unwrap();
    app.wait_for("id:db-pending-deletions").unwrap();
    assert!(app.screen().selected("id:db-deletions").unwrap());
    assert!(!app.screen().selected("id:db-initializations").unwrap());

    app.press("id:db-new").unwrap();
    app.wait_for("id:db-create-form").unwrap();
    assert!(app.screen().selected("id:db-new").unwrap());
    assert!(!app.screen().selected("id:db-deletions").unwrap());
}

#[test]
fn empty_detail_pane_shows_new_instance_cta_when_no_instance_is_selected() {
    let fake = with_instance(Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());

    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("127.0.0.1:5432").unwrap();

    // Simulate the selected instance being removed outside the UI, then refresh.
    {
        let mut fixture = fake
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        fixture.instances.clear();
        fixture.statuses.clear();
    }
    app.press("id:db-refresh").unwrap();

    app.wait_for("id:db-empty-state").unwrap();
    app.wait_for("No instance selected").unwrap();
    app.wait_for("id:db-empty-new-instance").unwrap();

    app.press("id:db-empty-new-instance").unwrap();
    app.wait_for("id:db-create-form").unwrap();
    assert!(app.screen().selected("id:db-new").unwrap());
}

#[test]
fn redis_persistence_uses_native_checkbox_and_toggles_cleanly() {
    let fake = FakeDatabases::new(fixture_with_packages(), Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());

    app.press("id:db-engine-redis").unwrap();
    app.wait_for("id:db-persistent").unwrap();
    app.wait_for("Persist database contents").unwrap();

    // Rendered as an accessible checkbox, checked by default.
    assert!(app.screen().matches("role:checkbox").unwrap());
    assert_eq!(
        app.screen()
            .accessibility_state("id:db-persistent")
            .unwrap()
            .checked,
        Some(Checked::True)
    );

    // Clicking toggles off and back on cleanly.
    app.press("id:db-persistent").unwrap();
    assert_eq!(
        app.screen()
            .accessibility_state("id:db-persistent")
            .unwrap()
            .checked,
        Some(Checked::False)
    );

    app.press("id:db-persistent").unwrap();
    assert_eq!(
        app.screen()
            .accessibility_state("id:db-persistent")
            .unwrap()
            .checked,
        Some(Checked::True)
    );

    // Toggle off and create to verify the form submits persistent = false.
    app.press("id:db-persistent").unwrap();
    app.type_text("id:db-name", "ephemeral-cache").unwrap();
    app.press("id:db-create").unwrap();
    app.wait_for("Created ephemeral-cache").unwrap();
    let persisted = fake
        .fixture
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .instances
        .iter()
        .find(|i| i.id == "ephemeral-cache")
        .map(|i| i.persistent);
    assert_eq!(persisted, Some(false));
}

#[test]
fn restore_dropzone_sets_backup_path_and_rail_context_menu_lists_actions() {
    let fake = with_instance(Behaviour::Normal);
    let (_turn, mut app) = start(fake);
    app.set_environment("macos", |env| {
        env.os = nsh_core::platform::Os::MacOs;
    })
    .unwrap();

    app.wait_for("id:db-instance-app").unwrap();
    app.press("id:db-instance-app").unwrap();
    app.wait_for("id:db-restore-dropzone").unwrap();

    // Drag-and-drop a percent-encoded file:// URL onto the Restore card.
    app.drop_files(
        "id:db-restore-dropzone",
        vec!["file:///tmp/devbase-tests/my%20backup.dump".to_string()],
    )
    .unwrap();
    assert_eq!(
        app.screen().input_text("id:db-backup-path").unwrap(),
        Some("/tmp/devbase-tests/my backup.dump")
    );

    // Right-clicking a rail instance row opens the native context menu with lifecycle/utility actions.
    app.right_click("id:db-instance-app").unwrap();
    let labels: Vec<String> = app
        .menu()
        .expect("context menu should open on right click")
        .into_iter()
        .map(|entry| entry.label)
        .collect();
    for expected in [
        "Start",
        "Stop",
        "Restart",
        "Copy URI",
        "Open CLI in Terminal",
        "View Logs",
        "Remove…",
    ] {
        assert!(
            labels.iter().any(|label| label == expected),
            "expected context menu item `{expected}`, got {labels:?}"
        );
    }
}
