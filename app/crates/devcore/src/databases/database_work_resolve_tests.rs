//! Resolve tests: temp roots, a fake launchd runner, real flocks.

use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::engine::CommandRunner;
use crate::launchd::CommandError;
use crate::state::Store;

struct FakeRunner;

impl CommandRunner for FakeRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<String, CommandError> {
        Err(CommandError::new(
            program,
            args.iter().map(|arg| (*arg).to_string()).collect(),
            "Could not find service",
        ))
    }
}

struct Fixture {
    _temp: TempDir,
    databases: Databases,
    dir: PathBuf,
    output: PathBuf,
    revision: String,
}

fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("dev");
    fs::create_dir_all(&root).unwrap();
    let store = Store::new(root.clone());
    store.init().unwrap();
    let databases = Databases::with_runner(store, Arc::new(FakeRunner));

    let id = "0123456789abcdef";
    let dir = root.join("database-work").join(id);
    fs::create_dir_all(&dir).unwrap();
    let identity = crate::databases::deletion_identity(&fs::metadata(&dir).unwrap()).unwrap();
    let record = DatabaseWorkRecord {
        schema: 1,
        id: id.into(),
        identity,
        job_digest: String::new(),
        parent_operation: String::new(),
        instance: Instance {
            id: "pg-main".into(),
            kind: "postgresql".into(),
            package: "postgresql@17-arm64".into(),
            port: 55432,
            ..Instance::default()
        },
        request: DatabaseWorkRequest {
            kind: "backup".into(),
            instance_id: "pg-main".into(),
            database: "blog".into(),
            source_path: String::new(),
        },
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let record_bytes = serde_json::to_vec(&record).unwrap();
    fs::write(dir.join("record.json"), &record_bytes).unwrap();
    fs::write(dir.join("control.lock"), b"").unwrap();
    fs::write(dir.join("execution.lock"), b"").unwrap();

    let backups = root.join("backups");
    fs::create_dir_all(&backups).unwrap();
    let output = backups.join("pg-main-blog-0123456789abcdef.dump");
    fs::write(&output, b"dump").unwrap();
    let artifact = serde_json::json!({"Path": output.to_string_lossy()});
    fs::write(
        dir.join("artifact.json"),
        serde_json::to_vec(&artifact).unwrap(),
    )
    .unwrap();

    let revision = digest_bytes(&[record_bytes, Vec::new()].concat());
    Fixture {
        _temp: temp,
        databases,
        dir,
        output,
        revision,
    }
}

fn hold_control(path: &Path) -> fs::File {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .unwrap();
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(result, 0);
    file
}

#[test]
fn resolve_is_refused_while_the_caller_holds_control() {
    let fixture = fixture();
    let _control = hold_control(&fixture.dir.join("control.lock"));
    let error = fixture
        .databases
        .resolve_database_work("0123456789abcdef", &fixture.revision)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "database task still has a caller; cancel it there or wait"
    );
    assert!(fixture.dir.exists());
}

#[test]
fn resolve_rejects_a_changed_revision() {
    let fixture = fixture();
    let error = fixture
        .databases
        .resolve_database_work("0123456789abcdef", "stale")
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "database task changed; refresh and review its current result"
    );
    assert!(fixture.dir.exists());
}

#[test]
fn resolve_clears_the_record_and_records_the_retained_artifact() {
    let fixture = fixture();
    fixture
        .databases
        .resolve_database_work("0123456789abcdef", &fixture.revision)
        .unwrap();
    assert!(!fixture.dir.exists());
    assert_eq!(fs::read(&fixture.output).unwrap(), b"dump");
    let operations = fixture.databases.store().root().join("operations");
    let mut recorded = false;
    for entry in fs::read_dir(operations).unwrap() {
        let entry = entry.unwrap();
        let bytes = fs::read(entry.path().join("record.json")).unwrap();
        let record: crate::operations::OperationRecord = serde_json::from_slice(&bytes).unwrap();
        if record.request.kind == "reconcile-database-work" {
            assert_eq!(record.request.resource, "0123456789abcdef");
            assert_eq!(record.artifact, fixture.output.to_string_lossy());
            assert_eq!(record.status, "Succeeded");
            recorded = true;
        }
    }
    assert!(recorded, "operation journal did not record the resolve");
}
