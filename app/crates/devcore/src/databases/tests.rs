//! Tests for the database foundation. Every test uses a temp root and an
//! explicit home; the command runner, port binder and readiness probe are
//! fakes, so no test runs `launchctl`, a database binary or binds a real
//! database port.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use super::ports::{
    ListeningPort, free_database_port, free_instance_port, parse_netstat_listeners,
    parse_port_listeners,
};
use super::{
    DatabaseError, Databases, InstanceAction, InstanceCatalog, MongoAdmin, PortBindFailure,
    PortBinder,
};
use crate::engine::CommandRunner;
use crate::launchd::CommandError;
use crate::model::{ConnectionProfile, Instance, Package, ProxyRule};
use crate::state::{ResourceKey, StateError, Store};
use crate::test_support_engine::FakeProbe;

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

type InputCall = (String, Vec<String>, Vec<(String, String)>, Option<String>);

#[derive(Default)]
struct TestRunner {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    jobs: Mutex<BTreeMap<String, String>>,
    job_argv: Mutex<BTreeMap<String, Vec<String>>>,
    lsof: Mutex<String>,
    inputs: Mutex<Vec<InputCall>>,
}

impl TestRunner {
    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }

    fn set_lsof(&self, output: String) {
        *self.lsof.lock().unwrap() = output;
    }

    fn inputs(&self) -> Vec<InputCall> {
        self.inputs.lock().unwrap().clone()
    }

    fn argv_for(&self, suffix: &str) -> Vec<Vec<String>> {
        self.job_argv
            .lock()
            .unwrap()
            .values()
            .filter(|argv| {
                argv.first()
                    .is_some_and(|program| program.ends_with(suffix))
            })
            .cloned()
            .collect()
    }

    fn label(target: &str) -> String {
        target.rsplit('/').next().unwrap_or_default().to_string()
    }
}

impl CommandRunner for TestRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<String, CommandError> {
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|arg| (*arg).to_string()).collect(),
        ));
        let fail = |message: &str| {
            Err(CommandError::new(
                program,
                args.iter().map(|arg| (*arg).to_string()).collect(),
                message,
            ))
        };
        if program == "/usr/sbin/lsof" {
            return Ok(self.lsof.lock().unwrap().clone());
        }
        if program == "/usr/sbin/netstat" {
            return Ok(String::new());
        }
        if program == "launchctl" || program == "/bin/launchctl" {
            let Some(command) = args.first() else {
                return fail("unexpected launchctl invocation");
            };
            match *command {
                "print" => {
                    let label = TestRunner::label(args.get(1).copied().unwrap_or_default());
                    match self.jobs.lock().unwrap().get(&label).map(String::as_str) {
                        Some("running") => {
                            Ok("state = running\npid = 987654\nruns = 1\n".to_string())
                        }
                        Some(_) => {
                            Ok("state = exited\npid = 0\nruns = 1\nlast exit code = 0\n"
                                .to_string())
                        }
                        None => fail("Could not find service"),
                    }
                }
                "bootstrap" => {
                    let path = args.get(2).copied().unwrap_or_default();
                    let label = Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .trim_end_matches(".plist")
                        .to_string();
                    let phase = if label.contains("-tool") {
                        "exited"
                    } else {
                        "running"
                    };
                    if let Ok(plist::Value::Dictionary(dict)) = plist::Value::from_file(path)
                        && let Some(plist::Value::Array(arguments)) = dict.get("ProgramArguments")
                    {
                        let argv: Vec<String> = arguments
                            .iter()
                            .filter_map(|value| value.as_string().map(str::to_string))
                            .collect();
                        simulate_database_side_effect(&argv);
                        self.job_argv.lock().unwrap().insert(label.clone(), argv);
                    }
                    self.jobs.lock().unwrap().insert(label, phase.to_string());
                    Ok(String::new())
                }
                "kickstart" => {
                    let label = TestRunner::label(args.get(1).copied().unwrap_or_default());
                    self.jobs
                        .lock()
                        .unwrap()
                        .insert(label, "running".to_string());
                    Ok(String::new())
                }
                "bootout" => {
                    let label = TestRunner::label(args.get(1).copied().unwrap_or_default());
                    self.jobs.lock().unwrap().remove(&label);
                    Ok(String::new())
                }
                _ => fail("unexpected launchctl command"),
            }
        } else {
            Ok(String::new())
        }
    }

    fn run_with_input(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
        _timeout: Duration,
    ) -> Result<String, CommandError> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let input = stdin.map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        self.inputs
            .lock()
            .unwrap()
            .push((program.to_string(), args.clone(), env, input));
        if program.ends_with("pg_dump") {
            if let Some(index) = args.iter().position(|arg| arg == "-f")
                && let Some(path) = args.get(index + 1)
            {
                fs::write(path, b"pgdump").unwrap();
            }
            return Ok(String::new());
        }
        if program.ends_with("mysqldump") {
            return Ok("CREATE TABLE t (id int);\n".to_string());
        }
        if program.ends_with("psql") {
            let is_query = args.iter().any(|arg| arg == "-Atc");
            if is_query {
                let query = args.last().cloned().unwrap_or_default();
                if query.contains("pg_database") {
                    return Ok("app\npostgres\n".to_string());
                }
                if query.contains("pg_roles") {
                    return Ok("devmanager\n".to_string());
                }
            }
            return Ok(String::new());
        }
        Ok(String::new())
    }
}

#[derive(Default)]
struct FakeMongo {
    calls: Mutex<Vec<(i64, String, String)>>,
}

impl FakeMongo {
    fn calls(&self) -> Vec<(i64, String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl MongoAdmin for FakeMongo {
    fn create_root_user(&self, port: i64, user: &str, password: &str) -> Result<(), DatabaseError> {
        self.calls
            .lock()
            .unwrap()
            .push((port, user.to_string(), password.to_string()));
        Ok(())
    }

    fn catalog(&self, _port: i64, _password: &str) -> Result<InstanceCatalog, DatabaseError> {
        Ok(InstanceCatalog {
            databases: vec!["admin".into(), "app".into()],
            users: vec!["devmanager".into()],
        })
    }

    fn create_database(
        &self,
        _port: i64,
        _password: &str,
        _name: &str,
    ) -> Result<(), DatabaseError> {
        Ok(())
    }

    fn drop_database(&self, _port: i64, _password: &str, _name: &str) -> Result<(), DatabaseError> {
        Ok(())
    }

    fn create_user(
        &self,
        _port: i64,
        _password: &str,
        _user: &str,
        _user_password: &str,
        _database: &str,
    ) -> Result<(), DatabaseError> {
        Ok(())
    }

    fn drop_user(
        &self,
        _port: i64,
        _password: &str,
        _user: &str,
        _database: &str,
    ) -> Result<(), DatabaseError> {
        Ok(())
    }

    fn backup(
        &self,
        _port: i64,
        _password: &str,
        _database: &str,
        path: &std::path::Path,
    ) -> Result<(), DatabaseError> {
        std::fs::write(path, b"mongo-backup").map_err(DatabaseError::from)
    }

    fn restore(
        &self,
        _port: i64,
        _password: &str,
        _database: &str,
        path: &std::path::Path,
    ) -> Result<(), DatabaseError> {
        std::fs::read(path).map(|_| ()).map_err(DatabaseError::from)
    }
}

#[derive(Default)]
struct FakeBinder {
    busy: Mutex<BTreeSet<u16>>,
    next: Mutex<VecDeque<u16>>,
    counter: Mutex<u16>,
    binds: Mutex<Vec<u16>>,
}

impl FakeBinder {
    fn busy(&self, port: u16) {
        self.busy.lock().unwrap().insert(port);
    }

    fn queue(&self, port: u16) {
        self.next.lock().unwrap().push_back(port);
    }

    fn binds(&self) -> Vec<u16> {
        self.binds.lock().unwrap().clone()
    }
}

impl PortBinder for FakeBinder {
    fn bind_loopback(&self, port: u16) -> Result<u16, PortBindFailure> {
        self.binds.lock().unwrap().push(port);
        if port == 0 {
            if let Some(queued) = self.next.lock().unwrap().pop_front() {
                if self.busy.lock().unwrap().contains(&queued) {
                    return Err(PortBindFailure::AddrInUse);
                }
                return Ok(queued);
            }
            let mut counter = self.counter.lock().unwrap();
            *counter += 1;
            return Ok(40000 + *counter);
        }
        if self.busy.lock().unwrap().contains(&port) {
            return Err(PortBindFailure::AddrInUse);
        }
        Ok(port)
    }

    fn dial_loopback(&self, _port: u16, _timeout: Duration) -> Result<(), PortBindFailure> {
        Ok(())
    }
}

/// Fake launchd executes nothing, so emulate exactly the directory creation a
/// real `initdb`, `mysqld --initialize-insecure` or `mongod` performs.
fn simulate_database_side_effect(argv: &[String]) {
    let Some(program) = argv.first() else {
        return;
    };
    if program.ends_with("/bin/initdb")
        && let Some(index) = argv.iter().position(|arg| arg == "-D")
        && let Some(target) = argv.get(index + 1)
    {
        let _ = fs::create_dir_all(target);
    }
    if program.ends_with("/bin/mysqld")
        && let Some(target) = argv.iter().find_map(|arg| arg.strip_prefix("--datadir="))
    {
        let _ = fs::create_dir_all(target);
    }
    if program.ends_with("/bin/mongod")
        && let Some(index) = argv.iter().position(|arg| arg == "--dbpath")
        && let Some(target) = argv.get(index + 1)
    {
        let _ = fs::create_dir_all(target);
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    store: Store,
    runner: Arc<TestRunner>,
    binder: Arc<FakeBinder>,
    probe: Arc<FakeProbe>,
    mongo: Arc<FakeMongo>,
    databases: Databases,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dev");
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let store = Store::new(&root).with_home(&home);
    store.init().unwrap();
    let runner = Arc::new(TestRunner::default());
    let binder = Arc::new(FakeBinder::default());
    let probe = Arc::new(FakeProbe::passing());
    let mongo = Arc::new(FakeMongo::default());
    let databases = Databases::with_runner(store.clone(), runner.clone() as Arc<dyn CommandRunner>)
        .with_binder(binder.clone())
        .with_probe(probe.clone())
        .with_mongo_admin(mongo.clone());
    Fixture {
        _dir: dir,
        root,
        store,
        runner,
        binder,
        probe,
        mongo,
        databases,
    }
}

fn seed_package(f: &Fixture, name: &str, version: &str, binaries: &[(&str, &str)]) -> Package {
    let package = Package {
        name: name.to_string(),
        version: version.to_string(),
        arch: "arm64".to_string(),
        url: "https://downloads.example.test/db.tar.gz".to_string(),
        sha256: crate::fs::digest_bytes(b"database archive"),
        format: "tar.gz".to_string(),
        binaries: binaries
            .iter()
            .map(|(tool, path)| (tool.to_string(), path.to_string()))
            .collect(),
        license: "MIT".to_string(),
        ..Default::default()
    };
    let id = package.id();
    for path in package.binaries.values() {
        let full = f.root.join("packages").join(&id).join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&full, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut state = f.store.read().unwrap();
    state.packages.insert(id.clone(), package.clone());
    f.store.write_state(&state).unwrap();
    package
}

fn seed_instance(f: &Fixture, instance: Instance) {
    let mut state = f.store.read().unwrap();
    state.instances.insert(instance.id.clone(), instance);
    f.store.write_state(&state).unwrap();
}

fn pg_instance(id: &str, port: i64, package: &Package) -> Instance {
    Instance {
        id: id.to_string(),
        kind: "postgresql".to_string(),
        package: package.id(),
        port,
        persistent: true,
        ..Default::default()
    }
}

fn install_pg(f: &Fixture) -> Package {
    seed_package(
        f,
        "db-kit",
        "17.0",
        &[
            ("initdb", "bin/initdb"),
            ("postgres", "bin/postgres"),
            ("psql", "bin/psql"),
        ],
    )
}

fn read_password(f: &Fixture, id: &str) -> String {
    let path = f.root.join("instances").join(id).join("password");
    let bytes = fs::read(&path).unwrap();
    String::from_utf8(bytes).unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// ---------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------

#[test]
fn available_local_port_skips_busy_and_reserved_ports() {
    let f = fixture();
    let package = install_pg(&f);
    seed_instance(&f, pg_instance("blog", 5432, &package));
    let mut state = f.store.read().unwrap();
    state.proxy_listener_port = 443;
    state.proxy_rules = vec![ProxyRule {
        domain: "app.devbase.localhost".to_string(),
        local_port: 8080,
        ..Default::default()
    }];
    f.store.write_state(&state).unwrap();
    f.binder.busy(9000);
    // 5432 is reserved by the registered instance; the selector must skip it.
    f.binder.queue(5432);
    f.binder.queue(9001);
    let port = f.databases.available_local_port(&|| false).unwrap();
    assert_eq!(port, 9001);
    assert!(f.binder.binds().len() >= 2);
}

#[test]
fn free_instance_port_prefers_free_preferred_port() {
    let binder = FakeBinder::default();
    let mut reserved = BTreeSet::new();
    reserved.insert(7000);
    assert_eq!(free_instance_port(&binder, 7001, &reserved).unwrap(), 7001);
    assert_eq!(free_instance_port(&binder, 7000, &reserved).unwrap(), 40001);
    assert_eq!(
        free_instance_port(&binder, 80, &reserved)
            .unwrap_err()
            .to_string(),
        "locked database port must be 1024–65535"
    );
}

#[test]
fn busy_database_port_is_a_typed_preflight_error() {
    let binder = FakeBinder::default();
    binder.busy(5432);
    let error = free_database_port(&binder, 5432).unwrap_err();
    assert!(matches!(error, DatabaseError::PortUnavailable(_)));
    assert_eq!(
        error.to_string(),
        "database port 5432 is unavailable; inspect the listener or choose another local port: address already in use"
    );
}

#[test]
fn create_instance_rejects_a_busy_port_before_staging() {
    let f = fixture();
    let package = install_pg(&f);
    f.binder.busy(5433);
    let error = f
        .databases
        .create_instance(pg_instance("blog", 5433, &package), &|| false)
        .unwrap_err();
    assert!(matches!(error, DatabaseError::PortUnavailable(_)));
    assert!(f.store.read().unwrap().instances.is_empty());
    assert!(!f.root.join("instances/blog").exists());
}

#[test]
fn inspect_port_reports_in_use_without_proving_ownership() {
    let f = fixture();
    f.binder.busy(6379);
    f.runner.set_lsof(format!(
        "p4242\0cpostgres\0u{}\0n127.0.0.1:6379\0",
        unsafe { libc::getuid() }
    ));
    let inspection = f.databases.inspect_port(6379, &|| false).unwrap();
    assert_eq!(inspection.status, "In use");
    assert_eq!(inspection.listeners.len(), 1);
    assert_eq!(inspection.listeners[0].pid, 4242);
    assert_eq!(inspection.listeners[0].name, "postgres");
    assert!(!inspection.listeners[0].own);

    let available = f.databases.inspect_port(7432, &|| false).unwrap();
    assert_eq!(available.status, "Available");
}

#[test]
fn parse_lsof_and_netstat_output() {
    let output = format!(
        "p100\0cpostgres\0u{}\0n127.0.0.1:5432\0n127.0.0.1:5433\0p100\0cpostgres\0u{}\0n127.0.0.1:5432\0p200\0cnode\0u{}\0n[::1]:8080\0",
        unsafe { libc::getuid() },
        unsafe { libc::getuid() },
        unsafe { libc::getuid() }
    );
    let listeners = parse_port_listeners(&output);
    assert_eq!(listeners.len(), 3);
    assert_eq!(listeners[0].pid, 100);
    assert_eq!(listeners[0].address, "127.0.0.1:5432");

    let netstat = "tcp4 0 0 127.0.0.1.5432 *.* LISTEN 0 0 131072 131072 launchd:1234 00180 00000006 0000000000000ddc 00000000 00000800 1 0 000000\n";
    let parsed = parse_netstat_listeners(netstat, 5432);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].pid, 1234);
    assert_eq!(parsed[0].name, "launchd");
    assert_eq!(parsed[0].address, "127.0.0.1:5432");
}

#[test]
fn listening_user_ports_excludes_instances_and_proxy() {
    let f = fixture();
    let package = install_pg(&f);
    seed_instance(&f, pg_instance("blog", 5432, &package));
    let mut state = f.store.read().unwrap();
    state.proxy_rules = vec![ProxyRule {
        domain: "app.devbase.localhost".to_string(),
        local_port: 8080,
        ..Default::default()
    }];
    f.store.write_state(&state).unwrap();
    drop(state);
    let uid = unsafe { libc::getuid() };
    f.runner.set_lsof(format!(
        "p1\0cnode\0u{uid}\0n127.0.0.1:8080\0p2\0cpostgres\0u{uid}\0n127.0.0.1:5432\0p3\0credis\0u{uid}\0n127.0.0.1:6379\0p4\0cother\0u99999\0n127.0.0.1:9000\0"
    ));
    let ports: Vec<ListeningPort> = f.databases.listening_user_ports(&|| false).unwrap();
    assert_eq!(
        ports,
        vec![
            ListeningPort {
                port: 6379,
                process: "redis".to_string()
            },
            ListeningPort {
                port: 8080,
                process: "node".to_string()
            }
        ]
    );
}

// ---------------------------------------------------------------------------
// Creation, initialization and publication
// ---------------------------------------------------------------------------

#[test]
fn create_instance_initializes_stages_and_publishes() {
    let f = fixture();
    let package = install_pg(&f);
    f.databases
        .create_instance(pg_instance("blog", 5432, &package), &|| false)
        .unwrap();

    let state = f.store.read().unwrap();
    let instance = state.instances.get("blog").expect("registered instance");
    assert_eq!(instance.kind, "postgresql");
    assert_eq!(instance.port, 5432);

    assert!(f.root.join("instances/blog/data").exists());
    let password = read_password(&f, "blog");
    assert_eq!(password.len(), 32);
    assert!(password.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        mode(&f.root.join("instances/blog/password")),
        0o600,
        "password file must be private"
    );

    let initdb = f.runner.argv_for("/bin/initdb");
    assert_eq!(initdb.len(), 1, "exactly one initdb job");
    let args = &initdb[0];
    assert!(args[0].ends_with("/bin/initdb"));
    assert_eq!(args[1], "-D");
    assert!(args[2].contains("initializations/") && args[2].ends_with("/data/data"));
    assert_eq!(
        &args[3..],
        [
            "-U",
            "devmanager",
            "--locale=C",
            "--encoding=UTF8",
            "--auth-host=scram-sha-256",
            "--auth-local=scram-sha-256",
            &format!(
                "--pwfile={}",
                args.last().unwrap().trim_start_matches("--pwfile=")
            ),
        ]
    );
    assert!(args.last().unwrap().ends_with("/data/password"));
    for (program, call_args) in f.runner.calls() {
        for arg in call_args {
            assert!(
                !arg.contains(&password),
                "password leaked into argv of {program}: {arg}"
            );
        }
    }

    assert!(
        fs::read_dir(f.root.join("jobs")).unwrap().next().is_none(),
        "initialization job plists must be removed before publication"
    );
}

#[test]
fn failed_publication_leaves_no_registered_or_live_instance() {
    let f = fixture();
    let package = install_pg(&f);
    let databases = f.databases.clone().with_publish_fault(Arc::new(|| {
        Err(StateError::invalid("injected publication failure"))
    }));
    let error = databases
        .create_instance(pg_instance("blog", 5432, &package), &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("injected publication failure"));
    assert!(f.store.read().unwrap().instances.is_empty());
    assert!(!f.root.join("instances/blog").exists());
    let staging: Vec<PathBuf> = fs::read_dir(f.root.join("initializations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(staging.len(), 1);
    assert!(staging[0].join("data/password").exists());
    assert!(!staging[0].join("data/data/data").exists());
    let staged_data = staging[0].join("data/data");
    assert!(
        staged_data.exists(),
        "undone publication must restore the staged data directory"
    );
}

#[test]
fn create_instance_holds_the_instance_lease() {
    let f = fixture();
    let package = install_pg(&f);
    let _held = f
        .store
        .resource_leases(&[ResourceKey::new("database instance", "blog")])
        .unwrap();
    let error = f
        .databases
        .create_instance(pg_instance("blog", 5432, &package), &|| false)
        .unwrap_err();
    assert!(matches!(
        error,
        DatabaseError::State(StateError::ResourceBusy(_))
    ));
    assert!(f.store.read().unwrap().instances.is_empty());
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[test]
fn start_restart_stop_and_remove_follow_the_lock_and_argv_rules() {
    let f = fixture();
    let package = install_pg(&f);
    f.databases
        .create_instance(pg_instance("blog", 5432, &package), &|| false)
        .unwrap();
    let password = read_password(&f, "blog");

    f.databases
        .instance_action("blog", InstanceAction::Start, &|| false)
        .unwrap();
    assert!(!f.probe.calls().is_empty());
    assert!(
        f.probe
            .calls()
            .iter()
            .all(|address| address == "tcp://127.0.0.1:5432")
    );
    let plists: Vec<PathBuf> = fs::read_dir(f.root.join("jobs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(plists.len(), 1, "start must write exactly one managed job");
    let job = fs::read_to_string(&plists[0]).unwrap();
    assert!(job.contains("instances/blog/data"));
    assert!(job.contains("postgres"));
    assert!(job.contains("-k"));
    assert!(!job.contains(&password), "password leaked into a job plist");
    let argv = f.runner.argv_for("/bin/postgres");
    assert_eq!(argv.len(), 1);
    let expected: Vec<String> = vec![
        argv[0][0].clone(),
        "-D".to_string(),
        f.root.join("instances/blog/data").display().to_string(),
        "-h".to_string(),
        "127.0.0.1".to_string(),
        "-p".to_string(),
        "5432".to_string(),
        "-k".to_string(),
        String::new(),
    ];
    assert_eq!(argv[0], expected);
    for (program, call_args) in f.runner.calls() {
        for arg in call_args {
            assert!(
                !arg.contains(&password),
                "password leaked into argv of {program}: {arg}"
            );
        }
    }

    let probes = f.probe.calls().len();
    f.databases
        .instance_action("blog", InstanceAction::Restart, &|| false)
        .unwrap();
    assert!(
        f.probe.calls().len() > probes,
        "restart must wait for readiness"
    );
    f.databases
        .instance_action("blog", InstanceAction::Stop, &|| false)
        .unwrap();

    let mut state = f.store.read().unwrap();
    state.connection_profiles.insert(
        "blog-pg".to_string(),
        ConnectionProfile {
            id: "blog-pg".to_string(),
            instance_id: "blog".to_string(),
            name: "Blog".to_string(),
            database: "blog".to_string(),
            username: "devmanager".to_string(),
            ..Default::default()
        },
    );
    f.store.write_state(&state).unwrap();
    drop(state);

    f.databases
        .instance_action("blog", InstanceAction::Remove, &|| false)
        .unwrap();
    let state = f.store.read().unwrap();
    assert!(!state.instances.contains_key("blog"));
    assert!(state.connection_profiles.is_empty());
    // Removal unregisters; it never deletes data.
    assert!(f.root.join("instances/blog/data").exists());
}

#[test]
fn start_rejects_a_busy_port() {
    let f = fixture();
    let package = install_pg(&f);
    f.databases
        .create_instance(pg_instance("blog", 5432, &package), &|| false)
        .unwrap();
    f.binder.busy(5432);
    let error = f
        .databases
        .instance_action("blog", InstanceAction::Start, &|| false)
        .unwrap_err();
    assert!(matches!(error, DatabaseError::PortUnavailable(_)));
}

#[test]
fn instance_action_requires_the_instance_lease() {
    let f = fixture();
    let package = install_pg(&f);
    f.databases
        .create_instance(pg_instance("blog", 5432, &package), &|| false)
        .unwrap();
    let _held = f
        .store
        .resource_leases(&[ResourceKey::new("database instance", "blog")])
        .unwrap();
    let error = f
        .databases
        .instance_action("blog", InstanceAction::Start, &|| false)
        .unwrap_err();
    assert!(matches!(
        error,
        DatabaseError::State(StateError::ResourceBusy(_))
    ));
}

#[test]
fn external_data_directory_is_retained_on_remove() {
    let f = fixture();
    let package = install_pg(&f);
    let external = f._dir.path().canonicalize().unwrap().join("external-pg");
    let mut instance = pg_instance("blog", 5432, &package);
    instance.data_directory = external.display().to_string();
    f.databases
        .create_instance(instance.clone(), &|| false)
        .unwrap();
    let (owner, identity) = {
        let state = f.store.read().unwrap();
        let stored = state.instances.get("blog").unwrap();
        (stored.data_owner.clone(), stored.data_identity.clone())
    };
    assert!(!owner.is_empty());
    assert!(!identity.is_empty());

    let marker = fs::read_to_string(external.join(".service-manager-owner")).unwrap();
    assert_eq!(
        marker,
        format!("{}\n{}\n{}\n", f.root.display(), "blog", owner)
    );
    assert!(external.join("data").exists());

    f.databases
        .instance_action("blog", InstanceAction::Remove, &|| false)
        .unwrap();
    assert!(!f.store.read().unwrap().instances.contains_key("blog"));
    assert!(external.exists(), "external data must survive removal");
    assert!(external.join("data").exists());
    assert!(external.join(".service-manager-owner").exists());
    // Credentials and local configuration stay beside the registration until a
    // separately confirmed permanent erase; removal itself deletes nothing.
    assert!(f.root.join("instances/blog/password").exists());
}

// ---------------------------------------------------------------------------
// Deletion staging
// ---------------------------------------------------------------------------

#[test]
fn delete_instance_data_stages_then_erases_internal_data() {
    let f = fixture();
    let package = install_pg(&f);
    f.databases
        .create_instance(pg_instance("blog", 5432, &package), &|| false)
        .unwrap();
    let preview = f
        .databases
        .preview_instance_deletion("blog", &|| false)
        .unwrap();
    assert!(preview.can_delete);
    assert!(preview.entries > 0);
    let stale = f
        .databases
        .delete_instance_data("blog", "stale-fingerprint", "blog", &|| false)
        .unwrap_err();
    assert!(stale.to_string().contains("refresh the deletion preview"));
    f.databases
        .delete_instance_data("blog", &preview.fingerprint, "blog", &|| false)
        .unwrap();
    assert!(!f.store.read().unwrap().instances.contains_key("blog"));
    assert!(!f.root.join("instances/blog").exists());
    let deletions: Vec<PathBuf> = fs::read_dir(f.root.join("deletions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(!deletions.is_empty());
    for deletion in deletions {
        assert!(!deletion.join("data").exists());
        assert!(!deletion.join("record.json").exists());
    }
}

#[test]
fn delete_instance_data_refuses_external_data_and_a_stale_preview() {
    let f = fixture();
    let package = install_pg(&f);
    let external = f._dir.path().canonicalize().unwrap().join("external-pg");
    let mut instance = pg_instance("blog", 5432, &package);
    instance.data_directory = external.display().to_string();
    f.databases.create_instance(instance, &|| false).unwrap();

    let preview = f
        .databases
        .preview_instance_deletion("blog", &|| false)
        .unwrap();
    assert!(!preview.can_delete);
    assert!(preview.detail.contains("External data is retained"));
    let error = f
        .databases
        .delete_instance_data("blog", &preview.fingerprint, "blog", &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("External data is retained"));
    assert!(external.join("data").exists());
    assert!(f.store.read().unwrap().instances.contains_key("blog"));

    let wrong = f
        .databases
        .delete_instance_data("blog", &preview.fingerprint, "other", &|| false)
        .unwrap_err();
    assert!(wrong.to_string().contains("type the exact instance ID"));
}

// ---------------------------------------------------------------------------
// Profiles and credentials
// ---------------------------------------------------------------------------

#[test]
fn profiles_are_credential_free_and_passwords_stay_in_the_file() {
    let f = fixture();
    let package = install_pg(&f);
    seed_instance(&f, pg_instance("blog", 5432, &package));
    let credentials = f.root.join("instances/blog");
    fs::create_dir_all(&credentials).unwrap();
    fs::write(credentials.join("password"), b"sentinel-secret").unwrap();
    fs::set_permissions(
        credentials.join("password"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    let profile = f
        .databases
        .save_connection_profile(ConnectionProfile {
            id: String::new(),
            instance_id: "blog".to_string(),
            name: "Blog".to_string(),
            database: "blog".to_string(),
            username: "devmanager".to_string(),
            ..Default::default()
        })
        .unwrap();
    assert!(!profile.id.is_empty());
    let info = f
        .databases
        .profile_connection_info("blog", &profile.id)
        .unwrap();
    assert_eq!(info.uri, "postgresql://devmanager@127.0.0.1:5432/blog");
    assert!(!info.uri.contains("sentinel"));
    assert_eq!(info.username, "devmanager");

    let state = f.store.read().unwrap();
    let bytes = serde_json::to_vec(&state).unwrap();
    assert!(
        !String::from_utf8_lossy(&bytes).contains("sentinel-secret"),
        "state must never carry a database password"
    );
    drop(state);

    assert_eq!(
        f.databases.database_password("blog").unwrap(),
        "sentinel-secret"
    );
    let script = f.databases.database_cli_command("blog", "", None).unwrap();
    assert!(script.contains("PGPASSWORD=\"$(cat "));
    assert!(script.contains("instances/blog/password"));
    assert!(!script.contains("sentinel-secret"));
}

#[test]
fn mysql_default_connection_uri_and_redis_profile_uri() {
    let f = fixture();
    let package = seed_package(
        &f,
        "db-kit",
        "17.0",
        &[("mysql", "bin/mysql"), ("redis-cli", "bin/redis-cli")],
    );
    let mut state = f.store.read().unwrap();
    state.instances.insert(
        "mysql".to_string(),
        Instance {
            id: "mysql".to_string(),
            kind: "mysql".to_string(),
            package: package.id(),
            port: 3306,
            ..Default::default()
        },
    );
    state.instances.insert(
        "cache".to_string(),
        Instance {
            id: "cache".to_string(),
            kind: "redis".to_string(),
            package: package.id(),
            port: 6379,
            ..Default::default()
        },
    );
    f.store.write_state(&state).unwrap();
    drop(state);

    let mysql = f.databases.connection_info("mysql").unwrap();
    assert_eq!(mysql.uri, "mysql://root@127.0.0.1:3306");
    let redis_profile = f
        .databases
        .save_connection_profile(ConnectionProfile {
            id: String::new(),
            instance_id: "cache".to_string(),
            name: "Cache".to_string(),
            ..Default::default()
        })
        .unwrap();
    let redis = f
        .databases
        .profile_connection_info("cache", &redis_profile.id)
        .unwrap();
    assert_eq!(redis.uri, "redis://default@127.0.0.1:6379/0");
}

#[test]
fn connection_info_with_database_and_jdbc_url_are_password_free() {
    let info = super::ConnectionInfo {
        uri: "postgresql://devmanager@127.0.0.1:5432/blog".to_string(),
        username: "devmanager".to_string(),
        credential_file: String::new(),
    };
    assert_eq!(
        info.with_database("postgresql", "other").uri,
        "postgresql://devmanager@127.0.0.1:5432/other"
    );
    assert_eq!(info.with_database("postgresql", "../etc").uri, info.uri);
    assert_eq!(
        info.jdbc_url("postgresql").unwrap(),
        "jdbc:postgresql://127.0.0.1:5432/blog?user=devmanager"
    );
}

#[test]
fn redis_instance_writes_a_private_config_and_starts_from_it() {
    let f = fixture();
    let package = seed_package(
        &f,
        "cache-kit",
        "8.2.0",
        &[("redis-server", "bin/redis-server")],
    );
    let instance = Instance {
        id: "cache".to_string(),
        kind: "redis".to_string(),
        package: package.id(),
        port: 6380,
        persistent: true,
        ..Default::default()
    };
    f.databases.create_instance(instance, &|| false).unwrap();
    let password = read_password(&f, "cache");
    let config_path = f.root.join("instances/cache/valkey.conf");
    assert_eq!(mode(&config_path), 0o600);
    let config = fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("bind 127.0.0.1"));
    assert!(config.contains("port 6380"));
    assert!(config.contains(&format!("requirepass {password}")));
    assert!(config.contains("appendonly yes"));
    assert!(!config.contains("password "));

    f.databases
        .instance_action("cache", InstanceAction::Start, &|| false)
        .unwrap();
    let argv = f.runner.argv_for("/bin/redis-server");
    assert_eq!(argv.len(), 1);
    assert_eq!(argv[0][1], config_path.display().to_string());
    assert!(!argv[0].iter().any(|arg| arg.contains(&password)));
}

#[test]
fn mysql_instance_initializes_without_the_password_in_argv() {
    let f = fixture();
    let package = seed_package(
        &f,
        "mysql-kit",
        "8.4.0",
        &[("mysqld", "bin/mysqld"), ("mysql", "bin/mysql")],
    );
    let instance = Instance {
        id: "shop".to_string(),
        kind: "mysql".to_string(),
        package: package.id(),
        port: 3307,
        persistent: true,
        ..Default::default()
    };
    f.databases.create_instance(instance, &|| false).unwrap();
    let password = read_password(&f, "shop");
    let client = f.root.join("instances/shop/client.cnf");
    assert_eq!(mode(&client), 0o600);
    let contents = fs::read_to_string(&client).unwrap();
    assert!(contents.contains("user=root"));
    assert!(contents.contains(&format!("password=\"{password}\"")));
    assert!(contents.contains("port=3307"));

    let initialize = f.runner.argv_for("/bin/mysqld");
    assert!(
        initialize
            .iter()
            .any(|argv| argv.iter().any(|arg| arg == "--initialize-insecure")),
        "mysqld initialization must be an owned launchd job"
    );
    for argv in &initialize {
        assert!(!argv.iter().any(|arg| arg.contains(&password)));
    }
    for (program, args) in f.runner.calls() {
        assert!(
            !args.iter().any(|arg| arg.contains(&password)),
            "password leaked into argv of {program}"
        );
    }
    assert!(
        !f.root.join("instances/shop/init-user.sql").exists(),
        "the generated SQL is removed after use"
    );

    f.databases
        .instance_action("shop", InstanceAction::Start, &|| false)
        .unwrap();
    assert!(!f.probe.calls().is_empty());
}

#[test]
fn mongodb_instance_creates_the_root_user_without_argv_or_a_shell_binary() {
    let f = fixture();
    let package = seed_package(&f, "mongo-kit", "8.0.0", &[("mongod", "bin/mongod")]);
    let instance = Instance {
        id: "docs".to_string(),
        kind: "mongodb".to_string(),
        package: package.id(),
        port: 27017,
        persistent: true,
        ..Default::default()
    };
    f.databases.create_instance(instance, &|| false).unwrap();
    let password = read_password(&f, "docs");
    let calls = f.mongo.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, 27017);
    assert_eq!(calls[0].1, "devmanager");
    assert_eq!(calls[0].2, password);
    for (program, args) in f.runner.calls() {
        assert!(
            !program.contains("mongosh"),
            "MongoDB initialization must not depend on an unmanaged shell"
        );
        assert!(!args.iter().any(|arg| arg.contains(&password)));
    }
    for argv in f.runner.argv_for("/bin/mongod") {
        assert!(!argv.iter().any(|arg| arg.contains(&password)));
    }
    assert!(
        !f.root.join("instances/docs/init-user.js").exists(),
        "no generated credential script stays on disk"
    );
}

// ---------------------------------------------------------------------------
// Connections, inventory, backing up and restoring
// ---------------------------------------------------------------------------

fn install_pg_clients(f: &Fixture) -> Package {
    seed_package(
        f,
        "pg-kit",
        "17.0",
        &[
            ("initdb", "bin/initdb"),
            ("postgres", "bin/postgres"),
            ("psql", "bin/psql"),
            ("createdb", "bin/createdb"),
            ("pg_dump", "bin/pg_dump"),
            ("pg_restore", "bin/pg_restore"),
        ],
    )
}

fn complete_pg(f: &Fixture, id: &str, port: i64) -> Package {
    let package = install_pg_clients(f);
    seed_instance(f, pg_instance(id, port, &package));
    let dir = f.root.join("instances").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("password"), b"admin-secret").unwrap();
    fs::set_permissions(dir.join("password"), fs::Permissions::from_mode(0o600)).unwrap();
    package
}

#[test]
fn inventory_reads_databases_and_users_and_refuses_protected_drops() {
    let f = fixture();
    complete_pg(&f, "app", 15432);
    let catalog = f
        .databases
        .inspect_instance_catalog("app", &|| false)
        .unwrap();
    assert_eq!(catalog.databases, vec!["app", "postgres"]);
    assert_eq!(catalog.users, vec!["devmanager"]);
    let calls = f.runner.inputs();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].1.iter().any(|arg| arg.contains("pg_database")));
    assert!(calls[1].1.iter().any(|arg| arg.contains("pg_roles")));
    // The admin secret is never in argv.
    for call in &calls {
        assert!(!call.1.iter().any(|arg| arg.contains("admin-secret")));
    }

    let error = f
        .databases
        .drop_database("app", "postgres", &|| false)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot drop protected system database")
    );
    let error = f
        .databases
        .drop_user("app", "devmanager", "", &|| false)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot drop protected built-in user")
    );
    f.databases.drop_database("app", "blog", &|| false).unwrap();
    let call = f.runner.inputs().pop().unwrap();
    assert!(call.0.ends_with("psql"));
    assert_eq!(
        call.1,
        vec![
            "-w",
            "-h",
            "127.0.0.1",
            "-p",
            "15432",
            "-U",
            "devmanager",
            "-X",
            "-d",
            "postgres",
            "--set",
            "ON_ERROR_STOP=1",
            "-c",
            "DROP DATABASE \"blog\"",
        ]
    );
}

#[test]
fn create_database_uses_a_private_pgpass_file_and_cleans_it_up() {
    let f = fixture();
    complete_pg(&f, "app", 15432);
    f.databases
        .create_database("app", "blog", &|| false)
        .unwrap();
    let calls = f.runner.inputs();
    let call = calls.last().unwrap();
    assert!(call.0.ends_with("createdb"));
    assert_eq!(
        call.1,
        vec![
            "-w",
            "-h",
            "127.0.0.1",
            "-p",
            "15432",
            "-U",
            "devmanager",
            "--",
            "blog"
        ]
    );
    assert!(!call.1.iter().any(|arg| arg.contains("admin-secret")));
    let pgpass = call
        .2
        .iter()
        .find(|(key, _)| key == "PGPASSFILE")
        .map(|(_, value)| value.clone())
        .expect("PGPASSFILE");
    assert!(pgpass.contains(".pgpass-"));
    assert!(
        !f.root
            .join("instances/app")
            .join(Path::new(&pgpass).file_name().unwrap())
            .exists()
    );
}

#[test]
fn backup_is_staged_published_and_restored_through_the_journal() {
    let f = fixture();
    complete_pg(&f, "app", 15432);
    let actions = Arc::new(super::actions::DatabaseActions::new(f.databases.clone()));
    let journal =
        crate::operations::OperationJournal::new(f.databases.store().clone()).with_actions(actions);
    let path = journal.backup("app", "blog", &|| false).unwrap();
    let backup = PathBuf::from(&path);
    assert!(backup.is_file());
    assert_eq!(mode(&backup), 0o600);
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("app-blog-")
    );
    assert_eq!(fs::read(&backup).unwrap(), b"pgdump");
    assert!(!PathBuf::from(format!("{path}.partial")).exists());
    let record = journal
        .operations(&|| false)
        .unwrap()
        .into_iter()
        .find(|record| record.request.kind == "backup")
        .unwrap();
    assert_eq!(record.artifact, path);
    assert_eq!(record.status, "Succeeded");

    journal.restore("app", "blog", &path, &|| false).unwrap();
    let calls = f.runner.inputs();
    let restore = calls
        .iter()
        .find(|call| call.0.ends_with("pg_restore"))
        .unwrap();
    assert_eq!(
        restore.1,
        vec![
            "-w",
            "-h",
            "127.0.0.1",
            "-p",
            "15432",
            "-U",
            "devmanager",
            "--exit-on-error",
            "--single-transaction",
            "-d",
            "blog",
            path.as_str(),
        ]
    );
    assert!(
        f.runner
            .inputs()
            .iter()
            .any(|call| call.0.ends_with("createdb") && call.1.last() == Some(&"blog".to_string()))
    );
}

#[test]
fn create_user_passes_the_password_on_stdin_and_never_into_the_record() {
    let f = fixture();
    complete_pg(&f, "app", 15432);
    let actions = Arc::new(super::actions::DatabaseActions::new(f.databases.clone()));
    let journal =
        crate::operations::OperationJournal::new(f.databases.store().clone()).with_actions(actions);
    journal
        .create_user("app", "writer", "user-secret", "blog", &|| false)
        .unwrap();
    let call = f.runner.inputs().pop().unwrap();
    assert!(call.0.ends_with("psql"));
    assert!(!call.1.iter().any(|arg| arg.contains("user-secret")));
    assert!(call.3.unwrap().contains("user-secret"));
    let records = journal.operations(&|| false).unwrap();
    let record = records
        .iter()
        .find(|record| record.request.kind == "create-user")
        .unwrap();
    let raw = fs::read_to_string(
        f.databases
            .store()
            .root()
            .join("operations")
            .join(&record.id)
            .join("record.json"),
    )
    .unwrap();
    assert!(!raw.contains("user-secret"));
    assert!(!raw.contains("admin-secret"));
}

// ---------------------------------------------------------------------------
// Database-work reservation
// ---------------------------------------------------------------------------

fn seed_database_work(f: &Fixture, work_id: &str, instance_id: &str) {
    let dir = f.root.join("database-work").join(work_id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("record.json"), b"placeholder").unwrap();
    let metadata = fs::symlink_metadata(&dir).unwrap();
    let identity = super::deletion_identity(&metadata).unwrap();
    let record = serde_json::json!({
        "Schema": 1,
        "ID": work_id,
        "Identity": identity,
        "JobDigest": "",
        "ParentOperation": "",
        "Instance": {
            "id": instance_id,
            "kind": "postgresql",
            "package": "pg-kit@17.0-arm64",
            "port": 15432,
            "persistent": true
        },
        "Request": {
            "Kind": "backup",
            "InstanceID": instance_id,
            "Database": "blog",
            "SourcePath": ""
        },
        "CreatedAt": "2026-01-01T00:00:00Z"
    });
    fs::write(
        dir.join("record.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
}

#[test]
fn an_unfinished_database_task_refuses_the_instance_lease() {
    let f = fixture();
    complete_pg(&f, "app", 15432);
    seed_database_work(&f, "work1", "app");
    let error = f
        .databases
        .store()
        .resource_leases(&[ResourceKey::new("database instance", "app")])
        .unwrap_err();
    assert!(error.is_busy(), "expected a busy refusal, got {error}");
    let pending = f.databases.pending_database_work(&|| false).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].instance_id, "app");
    // Another instance's lease is unaffected.
    f.databases
        .store()
        .resource_leases(&[ResourceKey::new("database instance", "other")])
        .unwrap();
}

// ---------------------------------------------------------------------------
// Recovery and upgrade
// ---------------------------------------------------------------------------

#[test]
fn initialization_recovery_archives_the_prepared_data_without_deleting_it() {
    let f = fixture();
    let initialization_id = "init1";
    let dir = f.root.join("initializations").join(initialization_id);
    let data = dir.join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("PG_VERSION"), b"17").unwrap();
    fs::write(dir.join("owner.lock"), b"").unwrap();
    let metadata = fs::symlink_metadata(&data).unwrap();
    let identity = super::deletion_identity(&metadata).unwrap();
    let record = serde_json::json!({
        "Schema": 1,
        "ID": initialization_id,
        "Instance": {
            "id": "app",
            "kind": "postgresql",
            "package": "pg-kit@17.0-arm64",
            "port": 15432,
            "persistent": true
        },
        "Identity": identity,
        "Jobs": {},
        "Ready": false,
        "CreatedAt": "2026-01-01T00:00:00Z"
    });
    fs::write(
        dir.join("record.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let pending = f
        .databases
        .pending_instance_initializations(&|| false)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].instance_id, "app");
    assert!(pending[0].can_resolve);
    let archive = f
        .databases
        .resolve_instance_initialization(initialization_id, &pending[0].revision, &|| false)
        .unwrap();
    let archive = PathBuf::from(archive);
    assert!(archive.join("PG_VERSION").is_file());
    assert!(!dir.join("data").exists());
    assert!(!dir.join("record.json").exists());
}

#[test]
fn pending_deletion_is_finished_only_for_the_reviewed_revision() {
    let f = fixture();
    let deletion_id = "del1";
    let dir = f.root.join("deletions").join(deletion_id);
    let staged = dir.join("data");
    fs::create_dir_all(&staged).unwrap();
    fs::write(staged.join("table.dat"), b"rows").unwrap();
    fs::write(dir.join("owner.lock"), b"").unwrap();
    let metadata = fs::symlink_metadata(&staged).unwrap();
    let identity = super::deletion_identity(&metadata).unwrap();
    let record = serde_json::json!({
        "Committed": true,
        "Schema": 1,
        "ID": deletion_id,
        "Instance": {
            "id": "app",
            "kind": "postgresql",
            "package": "pg-kit@17.0-arm64",
            "port": 15432,
            "persistent": true
        },
        "Identity": identity,
        "CreatedAt": "2026-01-01T00:00:00Z"
    });
    fs::write(
        dir.join("record.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let pending = f.databases.pending_instance_deletions(&|| false).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, "Ready to erase");
    assert!(pending[0].can_erase);
    let error = f
        .databases
        .finish_instance_deletion(deletion_id, "stale", "app", &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("deletion changed"));
    f.databases
        .finish_instance_deletion(deletion_id, &pending[0].revision, "app", &|| false)
        .unwrap();
    assert!(!staged.exists());
    assert!(!dir.join("record.json").exists());
}

#[test]
fn upgrade_targets_are_same_series_and_the_state_move_keeps_data() {
    let f = fixture();
    let current = seed_package(&f, "mysql", "8.0.1", &[("mysqld", "bin/mysqld")]);
    let target = seed_package(&f, "mysql", "8.0.3", &[("mysqld", "bin/mysqld")]);
    seed_package(&f, "mysql", "8.4.0", &[("mysqld", "bin/mysqld")]);
    seed_instance(
        &f,
        Instance {
            id: "app".into(),
            kind: "mysql".into(),
            package: current.id(),
            port: 13306,
            persistent: true,
            ..Default::default()
        },
    );
    let data = f.root.join("instances/app/data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("ibdata1"), b"data").unwrap();
    fs::write(f.root.join("instances/app/password"), b"secret").unwrap();
    fs::create_dir_all(f.root.join("logs")).unwrap();
    fs::write(
        f.root.join("logs/db-app.err.log"),
        b"[Note] ready for connections\n[Note] Shutdown complete\n",
    )
    .unwrap();
    let state = f.store.read().unwrap();
    let instance = state.instances.get("app").unwrap().clone();
    assert_eq!(
        super::upgrade::upgrade_targets(&state, &instance),
        vec![target.id()]
    );
    drop(state);
    f.databases
        .upgrade_instance_package(&instance, &target.id(), &|| false)
        .unwrap();
    let state = f.store.read().unwrap();
    assert_eq!(state.instances["app"].package, target.id());
    assert!(data.join("ibdata1").is_file());
    assert!(f.root.join("instances/app/password").is_file());
}

#[derive(Default)]
struct FakeRedis {
    calls: Mutex<Vec<String>>,
}

impl super::RedisAdmin for FakeRedis {
    fn keyspace_databases(
        &self,
        _port: i64,
        _password: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        self.calls.lock().unwrap().push("keyspace".into());
        Ok(vec!["0".into(), "3".into()])
    }

    fn backup(
        &self,
        _port: i64,
        _password: &str,
        db: i64,
        path: &Path,
    ) -> Result<(), DatabaseError> {
        self.calls.lock().unwrap().push(format!("backup {db}"));
        fs::write(path, b"redis-dump").map_err(DatabaseError::from)
    }

    fn restore(
        &self,
        _port: i64,
        _password: &str,
        db: i64,
        path: &Path,
    ) -> Result<(), DatabaseError> {
        self.calls.lock().unwrap().push(format!("restore {db}"));
        fs::read(path).map(|_| ()).map_err(DatabaseError::from)
    }
}

#[test]
fn mongodb_backup_and_restore_use_the_admin_seam() {
    let f = fixture();
    let package = "mongo-kit@7.0-arm64";
    seed_instance(
        &f,
        Instance {
            id: "mongo".into(),
            kind: "mongodb".into(),
            package: package.into(),
            port: 17017,
            persistent: true,
            ..Default::default()
        },
    );
    let dir = f.root.join("instances/mongo");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("password"), b"mongo-secret").unwrap();
    let path = f.databases.backup("mongo", "app", &|| false).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"mongo-backup");
    assert_eq!(mode(Path::new(&path)), 0o600);
    f.databases
        .restore("mongo", "app", &path, &|| false)
        .unwrap();
    let catalog = f
        .databases
        .inspect_instance_catalog("mongo", &|| false)
        .unwrap();
    assert!(catalog.databases.contains(&"app".to_string()));
}

#[test]
fn redis_inventory_and_backup_use_the_resp_seam() {
    let f = fixture();
    let redis = Arc::new(FakeRedis::default());
    let databases = Databases::with_runner(f.store.clone(), f.runner.clone())
        .with_binder(f.binder.clone())
        .with_probe(f.probe.clone())
        .with_mongo_admin(f.mongo.clone())
        .with_redis_admin(redis.clone());
    seed_instance(
        &f,
        Instance {
            id: "cache".into(),
            kind: "redis".into(),
            package: "redis-kit@7.2-arm64".into(),
            port: 16379,
            persistent: true,
            ..Default::default()
        },
    );
    let dir = f.root.join("instances/cache");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("password"), b"redis-secret").unwrap();
    let catalog = databases
        .inspect_instance_catalog("cache", &|| false)
        .unwrap();
    assert_eq!(catalog.databases, vec!["0", "3"]);
    assert_eq!(catalog.users, vec!["default"]);
    let path = databases.backup("cache", "3", &|| false).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"redis-dump");
    databases.restore("cache", "3", &path, &|| false).unwrap();
    assert_eq!(
        redis.calls.lock().unwrap().clone(),
        vec!["keyspace", "backup 3", "restore 3"]
    );
}
