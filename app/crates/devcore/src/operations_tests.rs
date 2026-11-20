use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, mpsc};

use super::*;
use crate::model::Package;

fn temp_root() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp dir")
}

fn journal(root: &std::path::Path) -> OperationJournal {
    OperationJournal::new(Store::new(root.join("workspace")))
}

#[derive(Default)]
struct FakeActions {
    calls: Mutex<Vec<String>>,
    fail_first: bool,
    resolve: Option<Package>,
}

impl FakeActions {
    fn new() -> Self {
        FakeActions::default()
    }

    fn failing_first() -> Self {
        FakeActions {
            fail_first: true,
            ..FakeActions::default()
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl OperationActions for FakeActions {
    fn instance_action(
        &self,
        _scope: &OperationScope<'_>,
        id: &str,
        action: &str,
    ) -> Result<(), StateError> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(format!("{action} {id}"));
        if self.fail_first && calls.len() == 1 {
            return Err(StateError::invalid("the action failed"));
        }
        Ok(())
    }

    fn instance_action_direct(&self, id: &str, action: &str) -> Result<(), StateError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("direct {action} {id}"));
        Ok(())
    }

    fn create_user(
        &self,
        _scope: &OperationScope<'_>,
        id: &str,
        name: &str,
        password: &str,
        database: &str,
    ) -> Result<(), StateError> {
        assert_eq!(password, "must-not-persist");
        self.calls
            .lock()
            .unwrap()
            .push(format!("user {name} {id} {database}"));
        Ok(())
    }

    fn resolve_package(&self, _name: &str, _version: &str) -> Result<Package, StateError> {
        self.resolve
            .clone()
            .ok_or_else(|| StateError::invalid("no package"))
    }

    fn install(&self, _scope: &OperationScope<'_>, package: &Package) -> Result<(), StateError> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(format!("install {}", package.id()));
        if self.fail_first && calls.len() == 1 {
            return Err(StateError::invalid("the install failed"));
        }
        Ok(())
    }

    fn repair_package(&self, _scope: &OperationScope<'_>, id: &str) -> Result<(), StateError> {
        self.calls.lock().unwrap().push(format!("repair {id}"));
        Ok(())
    }
    fn create_instance(
        &self,
        _scope: &OperationScope<'_>,
        instance: &crate::model::Instance,
    ) -> Result<(), StateError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("create {}", instance.id));
        Ok(())
    }

    fn restore(
        &self,
        _scope: &OperationScope<'_>,
        id: &str,
        name: &str,
        path: &str,
    ) -> Result<(), StateError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("restore {id} {name} {path}"));
        Ok(())
    }

    fn create_database(
        &self,
        _scope: &OperationScope<'_>,
        id: &str,
        name: &str,
    ) -> Result<(), StateError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("database {id} {name}"));
        Ok(())
    }

    fn backup(
        &self,
        _scope: &OperationScope<'_>,
        id: &str,
        name: &str,
    ) -> Result<String, StateError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("backup {id} {name}"));
        Ok("/tmp/output.dump".into())
    }
}

#[test]
fn live_lease_results_and_redaction() {
    let root = temp_root();
    let journal = journal(root.path()).with_actions(Arc::new(FakeActions::new()));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let worker = journal.clone();
    let handle = std::thread::spawn(move || {
        worker.run_operation(
            OperationRequest {
                kind: "up".into(),
                resource: "project".into(),
                ..OperationRequest::default()
            },
            "",
            &|| false,
            move |scope| {
                scope.emit("Starting api password=must-not-persist");
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Err(StateError::invalid("secret-password must-not-persist"))
            },
        )
    });
    entered_rx.recv().unwrap();
    let records = journal.operations(&|| false).unwrap();
    assert_eq!(records.len(), 1);
    let live = &records[0];
    assert!(live.busy);
    assert_eq!(live.status, STATUS_RUNNING);
    assert_eq!(live.phase, "Starting");
    assert!(!live.revision.is_empty());
    assert!(
        journal
            .dismiss_operation(&live.id, &live.revision, &|| false)
            .is_err()
    );
    assert!(
        journal
            .retry_operation(&live.id, &live.revision, &|| false)
            .is_err()
    );
    release_tx.send(()).unwrap();
    assert!(handle.join().unwrap().is_err());

    let records = journal.operations(&|| false).unwrap();
    assert_eq!(records.len(), 1);
    let finished = &records[0];
    assert!(!finished.busy);
    assert_eq!(finished.status, STATUS_FAILED);
    assert!(
        finished.result.starts_with("Stopped with an error."),
        "{}",
        finished.result
    );

    let path = journal
        .operation_root()
        .join(&finished.id)
        .join("record.json");
    let bytes = std::fs::read(&path).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("must-not-persist"));
    assert!(!text.contains("password"));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    assert!(
        journal
            .dismiss_operation(&live.id, &live.revision, &|| false)
            .is_err()
    );
    journal
        .dismiss_operation(&finished.id, &finished.revision, &|| false)
        .unwrap();
    assert!(journal.operations(&|| false).unwrap().is_empty());
}

#[test]
fn cancellation_and_concurrent_callers() {
    let root = temp_root();
    let journal = journal(root.path());
    let mut handles = Vec::new();
    for _ in 0..12 {
        let worker = journal.clone();
        handles.push(std::thread::spawn(move || {
            worker.run_operation(
                OperationRequest {
                    kind: "up".into(),
                    resource: "project".into(),
                    ..OperationRequest::default()
                },
                "",
                &|| false,
                |_| Err(StateError::Cancelled),
            )
        }));
    }
    for handle in handles {
        let error = handle.join().unwrap().unwrap_err();
        assert!(matches!(error, StateError::Cancelled));
    }
    let records = journal.operations(&|| false).unwrap();
    assert_eq!(records.len(), 12);
    let mut ids = std::collections::HashSet::new();
    for record in &records {
        assert_eq!(record.status, STATUS_CANCELLED);
        assert!(!record.busy);
        assert!(ids.insert(record.id.clone()));
    }
}

#[test]
fn retry_links_lineage_and_dismiss_removes_only_the_record() {
    let root = temp_root();
    let actions = Arc::new(FakeActions::failing_first());
    let journal = journal(root.path()).with_actions(actions.clone());
    assert!(journal.instance_action("api", "stop", &|| false).is_err());
    let records = journal.operations(&|| false).unwrap();
    assert_eq!(records.len(), 1);
    let parent = records[0].clone();
    assert_eq!(parent.status, STATUS_FAILED);
    assert!(operation_can_retry(&parent));

    journal
        .retry_operation(&parent.id, &parent.revision, &|| false)
        .unwrap();
    let records = journal.operations(&|| false).unwrap();
    assert_eq!(records.len(), 2);
    let child = records
        .iter()
        .find(|record| record.retry_of == parent.id)
        .expect("child record");
    let updated_parent = records
        .iter()
        .find(|record| record.id == parent.id)
        .expect("parent record");
    assert_eq!(child.status, STATUS_SUCCEEDED);
    assert_eq!(updated_parent.followup, child.id);
    assert!(!operation_can_retry(updated_parent));
    assert_eq!(actions.calls(), ["stop api", "stop api"]);

    assert!(
        journal
            .retry_operation(&parent.id, &parent.revision, &|| false)
            .is_err()
    );
    let kept = root.path().join("workspace/instances/keep");
    std::fs::create_dir_all(&kept).unwrap();
    journal
        .dismiss_operation(&child.id, &child.revision, &|| false)
        .unwrap();
    assert!(kept.exists());
    assert_eq!(journal.operations(&|| false).unwrap().len(), 1);
}

#[test]
fn create_user_never_records_the_password() {
    let root = temp_root();
    let actions = Arc::new(FakeActions::new());
    let journal = journal(root.path()).with_actions(actions);
    journal
        .create_user("api", "devbase", "must-not-persist", "app", &|| false)
        .unwrap();
    let records = journal.operations(&|| false).unwrap();
    let path = journal
        .operation_root()
        .join(&records[0].id)
        .join("record.json");
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("must-not-persist"));
    assert!(!text.contains("devbase"));
}

#[test]
fn released_owner_of_a_running_record_reports_interrupted() {
    let root = temp_root();
    let journal = journal(root.path());
    let ops = journal.operation_root();
    let id = "0123456789abcdef";
    let dir = ops.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let record = OperationRecord {
        schema: 1,
        id: id.into(),
        request: OperationRequest {
            kind: "db-start".into(),
            resource: "api".into(),
            ..OperationRequest::default()
        },
        phase: "Starting".into(),
        status: STATUS_RUNNING.into(),
        started_at: "2026-11-20T10:00:00Z".into(),
        ..OperationRecord::default()
    };
    std::fs::write(
        dir.join("record.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("owner.lock"), b"").unwrap();
    let records = journal.operations(&|| false).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, STATUS_INTERRUPTED);
    assert!(!records[0].busy);
    assert!(!records[0].revision.is_empty());
    assert!(records[0].result.starts_with("Owner released"));
}

#[test]
fn listing_a_new_root_is_read_only_and_refuses_a_linked_store() {
    let root = temp_root();
    let missing = root.path().join("new-root");
    let journal = journal(&missing);
    assert!(journal.operations(&|| false).unwrap().is_empty());
    assert!(!missing.exists());

    let workspace = journal.store().root().to_path_buf();
    std::fs::create_dir_all(&workspace).unwrap();
    let outside = root.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, workspace.join("operations")).unwrap();
    assert!(
        journal
            .run_operation(
                OperationRequest {
                    kind: "up".into(),
                    resource: "project".into(),
                    ..OperationRequest::default()
                },
                "",
                &|| false,
                |_| Ok(()),
            )
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn metadata_validation_rejects_control_characters() {
    let root = temp_root();
    let journal = journal(root.path());
    let error = journal
        .run_operation(
            OperationRequest {
                kind: "up".into(),
                resource: "bad\nresource".into(),
                ..OperationRequest::default()
            },
            "",
            &|| false,
            |_| Ok(()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("metadata is invalid"));
}

#[test]
fn invalid_instance_actions_run_directly_without_a_record() {
    let root = temp_root();
    let actions = Arc::new(FakeActions::new());
    let journal = journal(root.path()).with_actions(actions.clone());
    journal.instance_action("api", "remove", &|| false).unwrap();
    assert_eq!(actions.calls(), ["direct remove api"]);
    assert!(journal.operations(&|| false).unwrap().is_empty());
}

#[test]
fn backup_returns_and_records_the_artifact() {
    let root = temp_root();
    let actions = Arc::new(FakeActions::new());
    let journal = journal(root.path()).with_actions(actions);
    let path = journal.backup("api", "app", &|| false).unwrap();
    assert_eq!(path, "/tmp/output.dump");
    let records = journal.operations(&|| false).unwrap();
    assert_eq!(records[0].artifact, "/tmp/output.dump");
    assert_eq!(records[0].status, STATUS_SUCCEEDED);
}

#[test]
fn restore_rejects_partial_outputs_before_journaling() {
    let root = temp_root();
    let journal = journal(root.path());
    let error = journal
        .restore("api", "app", "/tmp/output.partial", &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("incomplete backup output"));
    assert!(journal.operations(&|| false).unwrap().is_empty());
}

#[test]
fn install_records_the_full_source_identity() {
    let root = temp_root();
    let actions = Arc::new(FakeActions::new());
    let journal = journal(root.path()).with_actions(actions.clone());
    let package = Package {
        name: "node".into(),
        version: "22.14.0".into(),
        arch: "arm64".into(),
        sha256: "a".repeat(64),
        ..Package::default()
    };
    journal.install(&package, &|| false).unwrap();
    let record = journal.operations(&|| false).unwrap().remove(0);
    assert_eq!(record.request.kind, "install");
    assert_eq!(record.request.resource, package.id());
    assert_eq!(record.request.sha256, package.sha256);
    assert_eq!(actions.calls(), [format!("install {}", package.id())]);
}

#[test]
fn retry_install_rejects_a_changed_package_source() {
    let root = temp_root();
    let mut actions = FakeActions::failing_first();
    let package = Package {
        name: "node".into(),
        version: "22.14.0".into(),
        arch: "arm64".into(),
        sha256: "a".repeat(64),
        ..Package::default()
    };
    actions.resolve = Some(Package {
        sha256: "b".repeat(64),
        ..package.clone()
    });
    let journal = journal(root.path()).with_actions(Arc::new(actions));
    assert!(journal.install(&package, &|| false).is_err());
    let records = journal.operations(&|| false).unwrap();
    let parent = &records[0];
    assert!(
        journal
            .retry_operation(&parent.id, &parent.revision, &|| false)
            .is_err()
    );
    let records = journal.operations(&|| false).unwrap();
    let child = records
        .iter()
        .find(|record| record.retry_of == parent.id)
        .unwrap();
    assert_eq!(child.status, STATUS_FAILED);
    assert!(child.result.contains("Stopped with an error"));
}

#[test]
fn timestamps_sort_by_instant_not_by_text() {
    let whole = parse_rfc3339_nanos("2026-01-01T00:00:00Z").unwrap();
    let fraction = parse_rfc3339_nanos("2026-01-01T00:00:00.5Z").unwrap();
    assert!(fraction > whole);
    let offset = parse_rfc3339_nanos("2026-01-01T01:00:00+01:00").unwrap();
    assert_eq!(offset, whole);
    let later = parse_rfc3339_nanos("2026-01-01T00:00:00.750000001Z").unwrap();
    assert!(later > fraction);
    assert!(parse_rfc3339_nanos("not a time").is_none());

    let now = now_rfc3339();
    assert!(now.ends_with('Z'));
    assert!(parse_rfc3339_nanos(&now).is_some());
}

#[test]
fn operation_phase_mapping_matches_the_go_prefixes() {
    assert_eq!(operation_phase("Downloading node"), Some("Downloading"));
    assert_eq!(operation_phase("Starting api"), Some("Starting"));
    assert_eq!(operation_phase("Installed node"), Some("Committed"));
    assert_eq!(operation_phase("unrelated"), None);
    assert_eq!(operation_work_phase("initialize"), "Initializing");
    assert_eq!(operation_work_phase("unknown"), "Working");
}

#[test]
fn source_paths_must_be_bounded_and_nul_free() {
    let root = temp_root();
    let journal = journal(root.path());
    let long = std::ffi::CString::new(vec![b'a'; 4097]).unwrap();
    let path = long.into_string().unwrap();
    let error = journal
        .run_operation(
            OperationRequest {
                kind: "restore".into(),
                source_path: path,
                ..OperationRequest::default()
            },
            "",
            &|| false,
            |_| Ok(()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("source path is invalid"));

    let max = "a".repeat(4096);
    journal
        .run_operation(
            OperationRequest {
                kind: "restore".into(),
                source_path: max,
                ..OperationRequest::default()
            },
            "",
            &|| false,
            |_| Ok(()),
        )
        .unwrap();
}
