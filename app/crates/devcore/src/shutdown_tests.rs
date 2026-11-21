use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::model::Instance;
use crate::state::ResourceKey;
use crate::test_support_engine::FakeCommands;

fn temp_root() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp dir")
}

fn instance(id: &str) -> Instance {
    Instance {
        id: id.into(),
        kind: "redis".into(),
        ..Instance::default()
    }
}

/// A launchd fake driven by a target -> loaded map.
fn runner(loaded: Arc<Mutex<BTreeMap<String, bool>>>) -> Arc<FakeCommands> {
    Arc::new(FakeCommands::new(move |_, args| {
        let target = args[1].clone();
        match args[0].as_str() {
            "print" => {
                if loaded
                    .lock()
                    .unwrap()
                    .get(&target)
                    .copied()
                    .unwrap_or(false)
                {
                    Ok("state = running\npid = 0\n".to_string())
                } else {
                    Err("Could not find service".to_string())
                }
            }
            "bootout" => {
                loaded.lock().unwrap().remove(&target);
                Ok(String::new())
            }
            other => panic!("unexpected launchctl command {other}"),
        }
    }))
}

#[test]
fn shutdown_stops_only_managed_database_jobs_and_keeps_data() {
    let root = temp_root();
    let store = Store::new(root.path().join("workspace"));
    let mut state = State::default();
    state.instances.insert("db".into(), instance("db"));
    store.write_state(&state).unwrap();
    let sentinel = store.root().join("instances/db/data/sentinel");
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&sentinel, b"retained data").unwrap();

    let loaded = Arc::new(Mutex::new(BTreeMap::new()));
    let engine = Engine::new(store.root(), runner(loaded.clone()))
        .with_backend(Arc::new(crate::engine::LaunchdBackend))
        .with_uid(501)
        .with_poll_interval(Duration::from_millis(1));
    for id in ["db-db", "proxy", "tunnel", "maintenance"] {
        loaded.lock().unwrap().insert(engine.target(id), true);
    }
    let shutdown = Shutdown::new(&store, &engine);
    let plan = shutdown.preview(&|| false).unwrap();
    assert_eq!(
        plan.instances,
        vec![ShutdownResource {
            id: "db".into(),
            name: "redis".into()
        }]
    );
    assert!(!plan.fingerprint.is_empty());

    shutdown
        .execute(&plan, &ShutdownOptions::default(), &|| false)
        .unwrap();
    let loaded = loaded.lock().unwrap();
    assert!(!loaded.contains_key(&engine.target("db-db")));
    assert!(loaded.contains_key(&engine.target("proxy")));
    assert!(loaded.contains_key(&engine.target("tunnel")));
    assert!(loaded.contains_key(&engine.target("maintenance")));
    drop(loaded);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"retained data");
    assert_eq!(store.read().unwrap(), state);
}

#[test]
fn shutdown_rejects_a_changed_workspace_without_touching_jobs() {
    let root = temp_root();
    let store = Store::new(root.path().join("workspace"));
    store.write_state(&State::default()).unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let touched = called.clone();
    let engine = Engine::new(
        store.root(),
        Arc::new(FakeCommands::new(move |_, _| {
            touched.store(true, Ordering::SeqCst);
            Ok(String::new())
        })),
    )
    .with_uid(501);
    let shutdown = Shutdown::new(&store, &engine);
    let plan = shutdown.preview(&|| false).unwrap();
    let mut state = store.read().unwrap();
    state.instances.insert("added".into(), instance("added"));
    store.write_state(&state).unwrap();
    let error = shutdown
        .execute(&plan, &ShutdownOptions::default(), &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("workspace changed"));
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn shutdown_cancellation_happens_before_any_mutation() {
    let root = temp_root();
    let store = Store::new(root.path().join("workspace"));
    let mut state = State::default();
    state.instances.insert("db".into(), instance("db"));
    store.write_state(&state).unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let touched = called.clone();
    let engine = Engine::new(
        store.root(),
        Arc::new(FakeCommands::new(move |_, _| {
            touched.store(true, Ordering::SeqCst);
            Ok(String::new())
        })),
    )
    .with_uid(501);
    let shutdown = Shutdown::new(&store, &engine);
    let plan = shutdown.preview(&|| false).unwrap();
    let error = shutdown
        .execute(&plan, &ShutdownOptions::default(), &|| true)
        .unwrap_err();
    assert!(matches!(error, StateError::Cancelled));
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn a_failed_observation_is_not_proof_that_a_job_stopped() {
    let root = temp_root();
    let store = Store::new(root.path().join("workspace"));
    let mut state = State::default();
    state.instances.insert("db".into(), instance("db"));
    store.write_state(&state).unwrap();
    let engine = Engine::new(
        store.root(),
        Arc::new(FakeCommands::new(|_, _| {
            Err("launchd observation unavailable".to_string())
        })),
    )
    .with_uid(501);
    let shutdown = Shutdown::new(&store, &engine);
    let plan = shutdown.preview(&|| false).unwrap();
    let error = shutdown
        .execute(&plan, &ShutdownOptions::default(), &|| false)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("could not verify stopped service"),
        "{error}"
    );
}

#[test]
fn shutdown_takes_instance_leases_and_reports_contention() {
    let root = temp_root();
    let store = Store::new(root.path().join("workspace"));
    let mut state = State::default();
    state.instances.insert("db".into(), instance("db"));
    store.write_state(&state).unwrap();
    let engine = Engine::new(
        store.root(),
        Arc::new(FakeCommands::new(|_, _| Ok(String::new()))),
    )
    .with_uid(501);
    let shutdown = Shutdown::new(&store, &engine);
    let plan = shutdown.preview(&|| false).unwrap();
    let _held = store
        .resource_leases(&[ResourceKey::new("database instance", "db")])
        .unwrap();
    let error = shutdown
        .execute(&plan, &ShutdownOptions::default(), &|| false)
        .unwrap_err();
    assert!(matches!(error, StateError::ResourceBusy(_)), "{error}");
}

#[test]
fn preview_lists_stopped_instances_and_binds_the_fingerprint() {
    let root = temp_root();
    let store = Store::new(root.path().join("workspace"));
    let mut state = State::default();
    state.instances.insert("b".into(), instance("b"));
    state.instances.insert("a".into(), instance("a"));
    store.write_state(&state).unwrap();
    let engine = Engine::new(
        store.root(),
        Arc::new(FakeCommands::new(|_, _| Ok(String::new()))),
    );
    let shutdown = Shutdown::new(&store, &engine);
    let plan = shutdown.preview(&|| false).unwrap();
    assert_eq!(
        plan.instances
            .iter()
            .map(|resource| resource.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(
        plan.fingerprint,
        shutdown_fingerprint(store.root(), &store.read().unwrap())
    );
    let mut changed = state;
    changed.instances.insert("c".into(), instance("c"));
    assert_ne!(
        shutdown_fingerprint(store.root(), &changed),
        plan.fingerprint
    );
}

#[test]
fn verify_target_accepts_only_a_definitive_absence() {
    let root = temp_root();
    let store = Store::new(root.path().join("workspace"));
    let engine = Engine::new(
        store.root(),
        Arc::new(FakeCommands::new(|_, args| {
            if args[1].ends_with("gone") {
                Err("Could not find specified service".to_string())
            } else {
                Err("operation not permitted".to_string())
            }
        })),
    );
    let shutdown = Shutdown::new(&store, &engine);
    shutdown.verify_target("gui/501/gone", &|| false).unwrap();
    let error = shutdown
        .verify_target("gui/501/unknown", &|| false)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("could not verify stopped service")
    );
}
