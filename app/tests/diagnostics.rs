//! Diagnostics, ports and tasks, headless: the runtime snapshot keys, the
//! report with its honest Unknown Phase 3 probes, the redacted export, the
//! port card and the journal's retry/dismiss. Fake providers stand in for
//! devcore: no test reads launchd, runs lsof or writes a report.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use devcore::{
    databases::{ListeningPort, PortInspection, PortListener},
    diagnostics::{DiagnosticCheck, DiagnosticReport, DiagnosticTarget},
    operations::{OperationRecord, OperationRequest},
};
use nsh_app_devbase::testing::{
    SystemDiagnosticsProvider, SystemOperationsProvider, SystemPortsProvider,
    install_system_diagnostics_provider, install_system_operations_provider,
    install_system_ports_provider, use_fake_databases, use_fake_development, use_fake_guide,
    use_fake_menubar, use_fake_providers, use_fake_proxy, use_fake_software, use_fake_system,
    use_fake_updater,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

/// The diagnostics tests share the fake-provider registry, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct FakeDiagnostics {
    diagnose_calls: Mutex<usize>,
    export_calls: Mutex<Vec<String>>,
    fail: AtomicBool,
}

impl FakeDiagnostics {
    fn new() -> Arc<Self> {
        Arc::new(FakeDiagnostics {
            diagnose_calls: Mutex::new(0),
            export_calls: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        })
    }
}

impl SystemDiagnosticsProvider for FakeDiagnostics {
    fn snapshot(&self) -> Result<BTreeMap<String, String>, String> {
        let mut states = BTreeMap::new();
        states.insert("instance:blog".to_string(), "Running".to_string());
        states.insert("proxy".to_string(), "Unknown".to_string());
        Ok(states)
    }

    fn diagnose(&self) -> Result<DiagnosticReport, String> {
        *self
            .diagnose_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
        if self.fail.load(Ordering::Relaxed) {
            return Err("diagnose failed: state lock timed out".to_string());
        }
        Ok(DiagnosticReport {
            schema_version: 1,
            generated_at: "2026-11-20T12:00:00Z".to_string(),
            redacted: false,
            proxy_revision: 0,
            checks: vec![
                check("CA", "Local CA", "Unknown"),
                check("Trust", "Local CA", "Unknown"),
            ],
        })
    }

    fn export(&self, _report: &DiagnosticReport) -> Result<PathBuf, String> {
        self.export_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push("export".to_string());
        Ok(PathBuf::from(
            "/tmp/devbase-tests/reports/diagnostics-abc.json",
        ))
    }

    fn log_rotation_disabled(&self) -> Result<bool, String> {
        Ok(false)
    }

    fn set_log_rotation(&self, _enabled: bool) -> Result<(), String> {
        Ok(())
    }
}

fn check(layer: &str, resource: &str, status: &str) -> DiagnosticCheck {
    DiagnosticCheck {
        layer: layer.to_string(),
        resource: resource.to_string(),
        status: status.to_string(),
        summary: format!("{layer} check has not been observed yet."),
        remedy: "Review the related setting in the app.".to_string(),
        target: DiagnosticTarget {
            kind: "ca".to_string(),
            id: String::new(),
        },
    }
}

struct FakeOperations {
    records: Mutex<Vec<OperationRecord>>,
    calls: Mutex<Vec<String>>,
}

impl FakeOperations {
    fn new(records: Vec<OperationRecord>) -> Arc<Self> {
        Arc::new(FakeOperations {
            records: Mutex::new(records),
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

impl SystemOperationsProvider for FakeOperations {
    fn operations(&self) -> Result<Vec<OperationRecord>, String> {
        Ok(self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn retry(&self, id: &str, revision: &str) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(format!("retry {id} {revision}"));
        Ok(())
    }

    fn dismiss(&self, id: &str, revision: &str) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(format!("dismiss {id} {revision}"));
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|record| record.id != id);
        Ok(())
    }
}

struct FakePorts {
    inspections: Mutex<Vec<i64>>,
}

impl FakePorts {
    fn new() -> Arc<Self> {
        Arc::new(FakePorts {
            inspections: Mutex::new(Vec::new()),
        })
    }
}

impl SystemPortsProvider for FakePorts {
    fn inspect(&self, port: i64) -> Result<PortInspection, String> {
        self.inspections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(port);
        Ok(PortInspection {
            port,
            status: "Available".to_string(),
            checked_at: "2026-11-20T12:00:00Z".to_string(),
            listeners: vec![PortListener {
                pid: 42,
                uid: 501,
                name: "node".to_string(),
                address: "127.0.0.1:3000".to_string(),
                own: false,
            }],
            truncated: false,
            detail: String::new(),
        })
    }

    fn available_local_port(&self) -> Result<i64, String> {
        Ok(4321)
    }

    fn listening(&self) -> Result<Vec<ListeningPort>, String> {
        Ok(vec![ListeningPort {
            port: 3000,
            process: "node".to_string(),
        }])
    }
}

fn record(id: &str, kind: &str, resource: &str, status: &str, revision: &str) -> OperationRecord {
    OperationRecord {
        schema: 1,
        id: id.to_string(),
        request: OperationRequest {
            kind: kind.to_string(),
            resource: resource.to_string(),
            ..OperationRequest::default()
        },
        phase: "Committed".to_string(),
        status: status.to_string(),
        result: "Completed.".to_string(),
        started_at: "2026-11-20T11:59:00Z".to_string(),
        updated_at: "2026-11-20T12:00:00Z".to_string(),
        revision: revision.to_string(),
        ..OperationRecord::default()
    }
}

/// Starts the surface on Diagnostics with the three fakes installed.
fn start() -> (
    MutexGuard<'static, ()>,
    Headless,
    Arc<FakeDiagnostics>,
    Arc<FakeOperations>,
    Arc<FakePorts>,
) {
    let turn = serial();
    use_fake_menubar();
    use_fake_providers();
    use_fake_software();
    use_fake_development();
    use_fake_databases();
    use_fake_proxy();
    use_fake_guide();
    use_fake_updater();
    use_fake_system();
    let diagnostics = FakeDiagnostics::new();
    let operations = FakeOperations::new(vec![
        record("task-1", "backup", "blog", "Succeeded", "rev-1"),
        record("task-2", "install", "node", "Failed", "rev-2"),
    ]);
    let ports = FakePorts::new();
    install_system_diagnostics_provider(diagnostics.clone());
    install_system_operations_provider(operations.clone());
    install_system_ports_provider(ports.clone());
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    app.press("role:button[Diagnostics]").unwrap();
    app.wait_for("id:diagnostics-page").unwrap();
    (turn, app, diagnostics, operations, ports)
}

#[test]
fn the_runtime_snapshot_and_the_unknown_phase3_checks_render() {
    let (_turn, mut app, diagnostics, _operations, _ports) = start();

    app.wait_for("id:runtime-instance:blog").unwrap();
    app.wait_for("id:runtime-proxy").unwrap();
    app.wait_for("blog").unwrap();
    app.wait_for("Database").unwrap();
    app.wait_for("HTTPS Proxy").unwrap();
    assert!(!app.screen().shows("instance:blog"));
    assert!(!app.screen().shows("Reading runtime status…"));
    assert!(app.screen().matches("id:diagnostics-refresh").unwrap());
    assert!(
        !app.screen()
            .matches("id:diagnostics-refresh-ports")
            .unwrap()
    );
    assert!(!app.screen().matches("id:operations-refresh").unwrap());

    // The diagnostics sheet is horizontally centered inside the Diagnostics page.
    let nodes = app.screen().layout_nodes();
    let page = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("diagnostics-page"))
        .expect("diagnostics-page should be laid out");
    let row = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("runtime-proxy"))
        .expect("runtime-proxy should be laid out");
    let left_gutter = row.bounds.origin.x.0 - page.bounds.origin.x.0;
    let right_gutter = (page.bounds.origin.x.0 + page.bounds.size.width.0)
        - (row.bounds.origin.x.0 + row.bounds.size.width.0);
    assert!(
        (left_gutter - right_gutter).abs() <= 2.0,
        "diagnostics sheet should be horizontally centered (left={left_gutter}, right={right_gutter})"
    );

    app.press("id:diagnostics-run").unwrap();
    app.wait_for("Unknown · CA").unwrap();
    app.wait_for("Unknown · Trust").unwrap();
    app.wait_for("Local CA").unwrap();
    // The summary names the unobserved checks instead of claiming health.
    app.wait_for("0 failed · 0 warnings · 2 checks · 2 unknown")
        .unwrap();
    assert!(app.screen().shows("CA check has not been observed yet."));
    assert_eq!(
        *diagnostics
            .diagnose_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
        1
    );
}

#[test]
fn exporting_writes_the_redacted_report_and_says_where() {
    let (_turn, mut app, diagnostics, _operations, _ports) = start();

    app.press("id:diagnostics-run").unwrap();
    app.wait_for("Unknown · CA").unwrap();
    app.press("id:diagnostics-export").unwrap();
    app.wait_for("Redacted report saved: /tmp/devbase-tests/reports/diagnostics-abc.json")
        .unwrap();
    assert_eq!(
        diagnostics
            .export_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        ["export"]
    );
}

#[test]
fn a_port_is_checked_and_a_free_port_is_suggested() {
    let (_turn, mut app, _diagnostics, _operations, ports) = start();

    app.press("id:diagnostics-check-port").unwrap();
    app.wait_for("Port 3000").unwrap();
    app.wait_for("PID 42 · UID 501 · node · 127.0.0.1:3000")
        .unwrap();
    assert_eq!(
        ports
            .inspections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        [3000]
    );

    app.press("id:diagnostics-free-port").unwrap();
    app.wait_for("text:/Suggested port: 4321/").unwrap();
    assert_eq!(
        app.screen().input_text("id:diagnostics-port").unwrap(),
        Some("4321")
    );
}

#[test]
fn the_journal_lists_tasks_and_retries_and_dismisses_them() {
    let (_turn, mut app, _diagnostics, operations, _ports) = start();

    app.wait_for("Task task-1").unwrap();
    app.wait_for("Task task-2").unwrap();
    assert!(
        !app.screen().matches("id:operation-retry-task-1").unwrap(),
        "succeeded tasks should not show a Retry button"
    );
    app.press("id:operation-retry-task-2").unwrap();
    app.wait(Duration::from_millis(500)).unwrap();
    assert!(
        operations
            .calls()
            .iter()
            .any(|call| call == "retry task-2 rev-2")
    );

    app.press("id:operation-dismiss-task-1").unwrap();
    app.wait_for_gone("id:operation-task-1", Duration::from_secs(3))
        .unwrap();
    assert!(
        operations
            .calls()
            .iter()
            .any(|call| call == "dismiss task-1 rev-1")
    );
}

#[test]
fn typing_port_input_keeps_input_and_listening_ports_mounted() {
    let (_turn, mut app, _diagnostics, _operations, _ports) = start();

    app.wait_for("id:diagnostics-port").unwrap();
    app.wait_for("id:listening-3000").unwrap();

    let port_input_id = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("diagnostics-port"))
        .expect("diagnostics-port should be mounted")
        .id;

    app.clear("id:diagnostics-port").unwrap();
    app.type_text("id:diagnostics-port", "5432").unwrap();

    let port_input_after = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("diagnostics-port"))
        .expect("diagnostics-port should remain mounted")
        .id;
    assert_eq!(port_input_after, port_input_id);
    assert_eq!(
        app.screen().input_text("id:diagnostics-port").unwrap(),
        Some("5432")
    );
}
