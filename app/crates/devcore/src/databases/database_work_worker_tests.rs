//! Worker-entry tests for `Databases::run_database_worker`.
//!
//! Everything is a temp workspace with a fake launchd/database runner; no
//! test starts a launchd job or a database client.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use super::{DatabaseWorkRecord, DatabaseWorkRequest, Databases};
use crate::engine::{CommandRunner, Engine};
use crate::launchd::CommandError;
use crate::model::{Instance, Package};
use crate::state::Store;

type RecordedCall = (String, Vec<String>, Option<String>);

#[derive(Default)]
struct WorkerRunner {
    pid: i32,
    calls: Mutex<Vec<RecordedCall>>,
}

impl WorkerRunner {
    fn with_pid(pid: i32) -> Self {
        WorkerRunner {
            pid,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for WorkerRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<String, CommandError> {
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|arg| (*arg).to_string()).collect(),
            None,
        ));
        if program == "launchctl" || program == "/bin/launchctl" {
            return Ok(format!("state = running\npid = {}\nruns = 1\n", self.pid));
        }
        if program == "systemctl" {
            return Ok(format!(
                "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID={}\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n",
                self.pid
            ));
        }
        Ok(String::new())
    }

    fn run_with_input(
        &self,
        program: &str,
        args: &[&str],
        _env: &[(&str, &str)],
        stdin: Option<&[u8]>,
        _timeout: Duration,
    ) -> Result<String, CommandError> {
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|arg| (*arg).to_string()).collect(),
            stdin.map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
        ));
        Ok(String::new())
    }
}

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    runner: Arc<WorkerRunner>,
    databases: Databases,
    instance: Instance,
}

fn fixture(pid: i32) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dev");
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let store = Store::new(&root).with_home(&home);
    store.init().unwrap();

    let package = Package {
        name: "mysql-kit".into(),
        version: "8.4.0".into(),
        arch: crate::platform::HostTarget::current().arch_name().into(),
        url: "https://downloads.example.test/mysql.tar.gz".into(),
        sha256: crate::fs::digest_bytes(b"mysql archive"),
        format: "tar.gz".into(),
        binaries: BTreeMap::from([
            ("mysql".to_string(), "bin/mysql".to_string()),
            ("mysqldump".to_string(), "bin/mysqldump".to_string()),
        ]),
        license: "MIT".into(),
        ..Default::default()
    };
    let package_id = package.id();
    for path in package.binaries.values() {
        let full = root.join("packages").join(&package_id).join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&full, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let instance = Instance {
        id: "shop".into(),
        kind: "mysql".into(),
        package: package_id.clone(),
        port: 3307,
        persistent: true,
        ..Default::default()
    };
    let mut state = store.read().unwrap();
    state.packages.insert(package_id, package);
    state
        .instances
        .insert(instance.id.clone(), instance.clone());
    store.write_state(&state).unwrap();

    let runner = Arc::new(WorkerRunner::with_pid(pid));
    let databases = Databases::with_runner(store.clone(), runner.clone() as Arc<dyn CommandRunner>);
    Fixture {
        _dir: dir,
        root,
        runner,
        databases,
        instance,
    }
}

fn work_dir(root: &Path, id: &str) -> PathBuf {
    root.join("database-work").join(id)
}

fn write_record(f: &Fixture, id: &str, kind: &str, database: &str) -> PathBuf {
    let dir = work_dir(&f.root, id);
    fs::create_dir_all(&dir).unwrap();
    let identity =
        crate::databases::deletion_identity(&fs::symlink_metadata(&dir).unwrap()).unwrap();

    let engine = Engine::new(&f.root, f.runner.clone() as Arc<dyn CommandRunner>);
    let job = format!("dbwork-{id}");
    let job_path = engine.job_path(&job);
    fs::create_dir_all(job_path.parent().unwrap()).unwrap();
    let job_bytes = b"recorded job".to_vec();
    fs::write(&job_path, &job_bytes).unwrap();

    let record = DatabaseWorkRecord {
        schema: 1,
        id: id.to_string(),
        identity,
        job_digest: crate::fs::digest_bytes(&job_bytes),
        parent_operation: String::new(),
        instance: f.instance.clone(),
        request: DatabaseWorkRequest {
            kind: kind.to_string(),
            instance_id: f.instance.id.clone(),
            database: database.to_string(),
            source_path: String::new(),
        },
        created_at: "2026-11-20T00:00:00Z".into(),
    };
    fs::write(
        dir.join("record.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    fs::write(dir.join("control.lock"), b"").unwrap();
    fs::write(dir.join("execution.lock"), b"").unwrap();
    fs::write(dir.join("input.json"), b"{}").unwrap();
    dir
}

#[test]
fn worker_runs_the_recorded_task_and_writes_a_result() {
    let f = fixture(std::process::id() as i32);
    let dir = write_record(&f, "task1", "create-database", "after-review");

    // Without the worker's own-record exemption the guard would refuse.
    let busy = f
        .databases
        .create_database("shop", "other", &|| false)
        .unwrap_err();
    assert!(
        busy.is_busy(),
        "the guard must reserve the instance: {busy}"
    );

    f.databases.run_database_worker("task1", &|| false).unwrap();

    let result = fs::read_to_string(dir.join("result.json")).unwrap();
    assert!(result.contains("\"Status\": \"Succeeded\""), "{result}");
    assert!(
        result.contains("\"FinishedAt\":"),
        "the result must carry a finished time"
    );
    assert!(!dir.join("input.json").exists(), "input.json is spent");

    let calls = f.runner.calls();
    assert!(
        calls.iter().any(|(program, _, stdin)| {
            program.ends_with("/bin/mysql")
                && stdin.as_deref() == Some("CREATE DATABASE `after-review`")
        }),
        "the recorded create-database command must run: {calls:?}"
    );
}

#[test]
fn worker_records_the_published_backup_artifact() {
    let f = fixture(std::process::id() as i32);
    let dir = write_record(&f, "task1", "backup", "shopdb");

    f.databases.run_database_worker("task1", &|| false).unwrap();

    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("result.json")).unwrap()).unwrap();
    let output = result["Output"].as_str().unwrap();
    assert!(output.ends_with(".dump"), "{output}");
    assert!(Path::new(output).exists(), "the backup must be published");
    let artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("artifact.json")).unwrap()).unwrap();
    assert_eq!(artifact["Path"], output);
    assert_eq!(result["Status"], "Succeeded");
}

#[test]
fn worker_refuses_to_replay_an_existing_result() {
    let f = fixture(std::process::id() as i32);
    let dir = write_record(&f, "task1", "create-database", "after-review");
    f.databases.run_database_worker("task1", &|| false).unwrap();
    assert!(dir.join("result.json").exists());

    let error = f
        .databases
        .run_database_worker("task1", &|| false)
        .unwrap_err();
    assert_eq!(error.to_string(), "database task already has a result");
}

#[test]
fn worker_refuses_a_changed_job_plist() {
    let f = fixture(std::process::id() as i32);
    let dir = write_record(&f, "task1", "create-database", "after-review");
    let engine = Engine::new(&f.root, f.runner.clone() as Arc<dyn CommandRunner>);
    fs::write(engine.job_path("dbwork-task1"), b"changed").unwrap();

    let error = f
        .databases
        .run_database_worker("task1", &|| false)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "database task job ownership is not verified"
    );
    assert!(!dir.join("result.json").exists());
}

#[test]
fn worker_refuses_to_run_outside_its_recorded_launchd_job() {
    let f = fixture(1);
    let _dir = write_record(&f, "task1", "create-database", "after-review");

    let error = f
        .databases
        .run_database_worker("task1", &|| false)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "database task must run as its recorded launchd job"
    );
}
