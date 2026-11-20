//! Diagnostics, logs and rotation tests. Temp roots only; the command runner
//! and dial probe are fakes, so nothing runs `launchctl` or opens a real
//! database port.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::engine::{CommandRunner, Engine};
use crate::launchd::CommandError;
use crate::model::{Instance, ProxyRule, State};
use crate::software::Software;
use crate::state::Store;

type Handler = dyn Fn(&str, &[&str]) -> Result<String, CommandError> + Send + Sync;

struct FakeRunner {
    handler: Box<Handler>,
    calls: Mutex<Vec<(String, Vec<String>)>>,
}

impl FakeRunner {
    fn new(
        handler: impl Fn(&str, &[&str]) -> Result<String, CommandError> + Send + Sync + 'static,
    ) -> Self {
        FakeRunner {
            handler: Box::new(handler),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn missing_services() -> Self {
        FakeRunner::new(|program, args| {
            Err(CommandError::new(
                program,
                args.iter().map(|arg| (*arg).to_string()).collect(),
                "Could not find service",
            ))
        })
    }

    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for FakeRunner {
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
        (self.handler)(program, args)
    }
}

#[derive(Default)]
struct FakeDial {
    fail: Mutex<bool>,
    calls: Mutex<Vec<String>>,
}

impl DialProbe for FakeDial {
    fn dial(&self, address: &str, _timeout: Duration) -> Result<(), String> {
        self.calls.lock().unwrap().push(address.to_string());
        if *self.fail.lock().unwrap() {
            Err("connection refused".into())
        } else {
            Ok(())
        }
    }
}

fn temp_root() -> (TempDir, Store) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("dev");
    std::fs::create_dir_all(&root).unwrap();
    (temp, Store::new(root))
}

fn diagnostics(store: &Store, runner: Arc<FakeRunner>, dial: Arc<FakeDial>) -> Diagnostics {
    let engine = Engine::new(store.root(), runner);
    let software = Software::new(store.clone());
    Diagnostics::new(store.clone(), engine, software).with_dial_probe(dial)
}

fn check(layer: &str, resource: &str, status: &str) -> DiagnosticCheck {
    DiagnosticCheck {
        layer: layer.into(),
        resource: resource.into(),
        status: status.into(),
        summary: "raw summary with /Users/alice/Private and token".into(),
        remedy: "raw remedy".into(),
        target: DiagnosticTarget::default(),
    }
}

#[test]
fn redacted_export_removes_resources_paths_and_fixed_text() {
    let report = DiagnosticReport {
        schema_version: 1,
        generated_at: "2026-01-01T00:00:00Z".into(),
        redacted: false,
        checks: vec![
            check("Package", "/Users/alice/Private Project", "OK"),
            check("Database", "Database internal-db", "Failed"),
            check("Unknown Layer", "internal.example.com", "Bogus"),
        ],
        proxy_revision: 7,
    };
    let bytes = report.redacted_json().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(!text.contains("/Users/alice"));
    assert!(!text.contains("internal.example.com"));
    assert!(!text.contains("token"));
    assert!(!text.contains("raw remedy"));
    assert!(text.contains("\"resource\": \"resource-1\""));
    assert!(text.contains("\"resource\": \"resource-2\""));
    assert!(text.contains("\"resource\": \"resource-3\""));
    assert!(text.contains("\"layer\": \"Other\""));
    assert!(text.contains("\"status\": \"Unknown\""));
    assert!(text.contains("\"summary\": \"Package check: OK\""));
    assert!(text.contains("\"redacted\": true"));
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["schemaVersion"], 1);
    assert!(parsed.get("proxyRevision").is_none());
}

#[test]
fn export_writes_a_new_private_redacted_report() {
    let (temp, store) = temp_root();
    let diag = diagnostics(
        &store,
        Arc::new(FakeRunner::missing_services()),
        Arc::new(FakeDial::default()),
    );
    let report = DiagnosticReport {
        schema_version: 1,
        generated_at: "2026-01-01T00:00:00Z".into(),
        redacted: false,
        checks: vec![check("Package", "secret-package@1-arm64", "OK")],
        proxy_revision: 0,
    };
    let path = diag.export_diagnostics(&report).unwrap();
    assert!(path.starts_with(temp.path().join("dev").join("reports")));
    let metadata = std::fs::metadata(&path).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("secret-package"));
    assert!(text.contains("resource-1"));
}

#[test]
fn snapshot_reports_instance_proxy_and_tunnel_keys() {
    let (_temp, store) = temp_root();
    let mut state = State::default();
    state.instances.insert(
        "pg-main".into(),
        Instance {
            id: "pg-main".into(),
            kind: "postgresql".into(),
            package: "postgresql@17-arm64".into(),
            port: 55432,
            persistent: true,
            ..Instance::default()
        },
    );
    state.proxy_rules.push(ProxyRule {
        domain: "app.localhost".into(),
        local_port: 3000,
        ..ProxyRule::default()
    });
    state
        .tunnel_workers
        .insert("app".into(), crate::model::TunnelWorkerState::default());
    let diag = diagnostics(
        &store,
        Arc::new(FakeRunner::missing_services()),
        Arc::new(FakeDial::default()),
    );
    let snapshot = diag.snapshot(&state);
    assert_eq!(
        snapshot.states.get("instance:pg-main"),
        Some(&"Stopped".into())
    );
    assert_eq!(snapshot.states.get("proxy"), Some(&"Stopped".into()));
    assert_eq!(snapshot.states.get("tunnel"), Some(&"Stopped".into()));
    assert_eq!(snapshot.states.len(), 3);
}

#[test]
fn snapshot_reports_a_configured_tunnel_token_without_reading_it() {
    let (_temp, store) = temp_root();
    store.init().unwrap();
    let proxy = store.root().join("proxy");
    std::fs::create_dir_all(&proxy).unwrap();
    std::fs::write(proxy.join("cloudflare.key"), b"0123456789").unwrap();
    let diag = diagnostics(
        &store,
        Arc::new(FakeRunner::missing_services()),
        Arc::new(FakeDial::default()),
    );
    let snapshot = diag.snapshot(&State::default());
    assert_eq!(
        snapshot.states.get("tunnel:token"),
        Some(&"Configured".into())
    );
    assert_eq!(snapshot.states.get("tunnel"), Some(&"Stopped".into()));
}

#[test]
fn snapshot_marks_a_registered_but_exited_service_failed() {
    let (_temp, store) = temp_root();
    let runner = Arc::new(FakeRunner::new(|program, args| {
        if program == "launchctl" && args.first() == Some(&"print") {
            return Ok("state = exited\npid = 0\nruns = 1\nlast exit code = 0\n".into());
        }
        Err(CommandError::new(
            program,
            args.iter().map(|arg| (*arg).to_string()).collect(),
            "unexpected",
        ))
    }));
    let diag = diagnostics(&store, runner, Arc::new(FakeDial::default()));
    let mut state = State::default();
    state.instances.insert("redis".into(), Instance::default());
    let snapshot = diag.snapshot(&state);
    assert_eq!(
        snapshot.states.get("instance:redis"),
        Some(&"Failed".into())
    );
}

#[test]
fn diagnose_checks_packages_databases_and_marks_phase3_unknown() {
    let (_temp, store) = temp_root();
    let mut state = State::default();
    state.instances.insert(
        "pg".into(),
        Instance {
            id: "pg".into(),
            kind: "postgresql".into(),
            port: 55433,
            ..Instance::default()
        },
    );
    state.proxy_rules.push(ProxyRule {
        domain: "app.devbase.localhost".into(),
        local_port: 3000,
        ..ProxyRule::default()
    });
    store.write_state(&state).unwrap();
    let runner = Arc::new(FakeRunner::new(|program, args| {
        if program == "launchctl" && args.first() == Some(&"print") {
            return Ok("state = running\npid = 42\n".into());
        }
        Err(CommandError::new(
            program,
            args.iter().map(|arg| (*arg).to_string()).collect(),
            "unexpected",
        ))
    }));
    let dial = Arc::new(FakeDial::default());
    let diag = diagnostics(&store, runner, dial.clone());
    let report = diag.diagnose().unwrap();
    let database = report
        .checks
        .iter()
        .find(|check| check.layer == "Database")
        .unwrap();
    assert_eq!(database.status, "OK");
    assert_eq!(dial.calls.lock().unwrap().as_slice(), ["127.0.0.1:55433"]);
    for layer in ["CA", "Trust", "DNS", "Upstream", "Proxy TLS", "HTTP"] {
        let check = report
            .checks
            .iter()
            .find(|check| check.layer == layer)
            .unwrap();
        assert_eq!(
            check.status,
            Phase3Probe::ALL
                .iter()
                .find(|probe| probe.layer() == layer)
                .unwrap()
                .status()
        );
        assert_eq!(check.status, "Unknown");
    }
    let redacted = String::from_utf8(report.redacted_json().unwrap()).unwrap();
    assert!(!redacted.contains("app.devbase.localhost"));
}

#[test]
fn log_rotation_setting_round_trips_and_writes_the_maintenance_job() {
    let (_temp, store) = temp_root();
    let runner = Arc::new(FakeRunner::new(|program, args| {
        if program == "launchctl" && args.first() == Some(&"bootstrap") {
            return Ok(String::new());
        }
        Err(CommandError::new(
            program,
            args.iter().map(|arg| (*arg).to_string()).collect(),
            "Could not find service",
        ))
    }));
    let diag = diagnostics(&store, runner.clone(), Arc::new(FakeDial::default()))
        .with_program("/Applications/Devbase.app/Contents/MacOS/Devbase");
    assert!(!diag.log_rotation_disabled().unwrap());
    diag.set_log_rotation(false).unwrap();
    assert!(diag.log_rotation_disabled().unwrap());
    assert!(diag.set_log_rotation(true).is_ok());
    assert!(!diag.log_rotation_disabled().unwrap());
    assert!(diag.engine().job_path("maintenance").exists());
    let calls = runner.calls();
    assert!(calls.iter().any(|(program, args)| {
        program == "launchctl" && args.first().is_some_and(|arg| arg == "bootstrap")
    }));
}

#[test]
fn rotation_moves_the_last_five_mebibytes_and_truncates() {
    let (_temp, store) = temp_root();
    store.init().unwrap();
    let diag = diagnostics(
        &store,
        Arc::new(FakeRunner::missing_services()),
        Arc::new(FakeDial::default()),
    );
    let logs = store.root().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let big = logs.join("service.log");
    let content: Vec<u8> = (0..6 * 1024 * 1024)
        .map(|index| b'a' + (index % 26) as u8)
        .collect();
    std::fs::write(&big, &content).unwrap();
    std::fs::write(big.with_extension("log.1"), b"older").unwrap();
    let small = logs.join("small.log");
    std::fs::write(&small, b"tiny").unwrap();

    diag.rotate_logs().unwrap();
    assert_eq!(std::fs::metadata(&big).unwrap().len(), 0);
    let archive = Path::new(&format!("{}.1", big.display())).to_path_buf();
    let archived = std::fs::read(&archive).unwrap();
    assert_eq!(archived.len() as u64, LOG_LIMIT);
    assert_eq!(
        archived,
        content[content.len() - LOG_LIMIT as usize..].to_vec()
    );
    let previous = Path::new(&format!("{}.2", big.display())).to_path_buf();
    assert_eq!(std::fs::read(previous).unwrap(), b"older");
    assert_eq!(std::fs::read(&small).unwrap(), b"tiny");
}

#[test]
fn logs_tail_bounds_lines_and_reports_empty_files() {
    let (_temp, store) = temp_root();
    store.init().unwrap();
    assert_eq!(
        logs(&store, "api").unwrap(),
        "No logs yet. Start the service to capture its output."
    );
    assert!(logs(&store, "../etc/passwd").is_err());
    let logs_dir = store.root().join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::write(logs_dir.join("api.log"), b"one\ntwo\nthree\n").unwrap();
    let text = logs(&store, "api").unwrap();
    assert!(text.contains("api.log:"));
    assert!(text.contains("one\ntwo\nthree"));
}

#[test]
fn tail_marks_truncation_for_large_files() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("log");
    let body = format!("first\n{}\nlast\n", "x".repeat(100));
    std::fs::write(&path, body).unwrap();
    let snapshot = tail(&path, 64, 2).unwrap();
    assert!(snapshot.truncated);
    assert_eq!(snapshot.lines, vec!["last".to_string()]);
}
