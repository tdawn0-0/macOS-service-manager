use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::*;
use crate::health::{JobState, ServiceProbe, parse_job_state, validate_health, wait_service};
use crate::test_support_engine::{FakeCommands, FakeProbe};

fn temp_root() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp dir")
}

#[test]
fn label_and_domain_derive_from_the_cleaned_root() {
    let root = temp_root();
    let path = root.path().join("workspace");
    let engine = Engine::new(
        path.clone(),
        Arc::new(FakeCommands::new(|_, _| Ok(String::new()))),
    )
    .with_uid(501);
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    let prefix = crate::fs::encode_hex(&digest[..4]);
    assert_eq!(engine.domain(), "gui/501");
    assert_eq!(
        engine.label("db-api"),
        format!("xyz.jyeu.devbase.dev.{prefix}.db-api")
    );
    assert_eq!(
        engine.target("db-api"),
        format!("gui/501/xyz.jyeu.devbase.dev.{prefix}.db-api")
    );
    assert_eq!(label_prefix(&path), prefix);

    // `..` components do not change the label, matching filepath.Clean.
    let uncleaned = path.join("nested").join("..");
    assert_eq!(
        engine.label("x"),
        format!("xyz.jyeu.devbase.dev.{prefix}.x")
    );
    assert_eq!(label_prefix(&uncleaned), prefix);
}

#[test]
fn start_job_writes_a_private_plist_and_bootstraps() {
    let root = temp_root();
    let runner = Arc::new(FakeCommands::new(|program, args| {
        match args.first().map(String::as_str) {
            Some("print") => Err("Could not find service".to_string()),
            Some("bootstrap") => {
                assert_eq!(program, "launchctl");
                Ok(String::new())
            }
            other => panic!("unexpected launchctl call {other:?}"),
        }
    }));
    let engine = Engine::new(root.path(), runner.clone()).with_uid(501);
    let job = JobSpec {
        id: "db-api".into(),
        args: vec!["/bin/sleep".into(), "30".into()],
        working_dir: root.path().to_string_lossy().into_owned(),
        env: BTreeMap::from([("LANG".to_string(), "C".to_string())]),
        restart: true,
    };
    assert!(engine.start_job(&job, &|| false).unwrap());
    let path = engine.job_path("db-api");
    assert!(path.exists());
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);

    let plist = plist::Value::from_file(&path)
        .unwrap()
        .into_dictionary()
        .unwrap();
    assert_eq!(
        plist.get("Label").and_then(|value| value.as_string()),
        Some(engine.label("db-api").as_str())
    );
    assert_eq!(
        plist
            .get("WorkingDirectory")
            .and_then(|value| value.as_string()),
        Some(root.path().to_string_lossy().as_ref())
    );
    assert_eq!(
        plist.get("RunAtLoad").and_then(|value| value.as_boolean()),
        Some(true)
    );
    assert_eq!(
        plist.get("KeepAlive").and_then(|value| value.as_boolean()),
        Some(true)
    );
    assert_eq!(
        plist
            .get("StandardOutPath")
            .and_then(|value| value.as_string()),
        Some(
            engine
                .root()
                .join("logs/db-api.log")
                .to_string_lossy()
                .as_ref()
        )
    );

    let calls = runner.calls();
    assert_eq!(calls[1][0], "launchctl");
    assert_eq!(calls[1][1], "bootstrap");
    assert_eq!(calls[1][2], "gui/501");
}

#[test]
fn start_job_does_not_rewrite_a_running_job() {
    let root = temp_root();
    let runner = Arc::new(FakeCommands::new(|_, args| {
        assert_eq!(args.first().map(String::as_str), Some("print"));
        Ok("state = running\npid = 12\n".to_string())
    }));
    let engine = Engine::new(root.path(), runner.clone()).with_uid(501);
    let job = JobSpec {
        id: "db-api".into(),
        args: vec!["/bin/sleep".into(), "30".into()],
        ..JobSpec::default()
    };
    assert!(!engine.start_job(&job, &|| false).unwrap());
    assert!(!engine.job_path("db-api").exists());
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn stop_job_boots_out_and_waits_for_absence() {
    let root = temp_root();
    let loaded = Arc::new(Mutex::new(BTreeMap::new()));
    let state = loaded.clone();
    let runner = Arc::new(FakeCommands::new(move |_, args| {
        let target = args[1].clone();
        match args[0].as_str() {
            "print" => {
                if state.lock().unwrap().get(&target).copied().unwrap_or(false) {
                    Ok("state = running\npid = 0\n".to_string())
                } else {
                    Err("Could not find service".to_string())
                }
            }
            "bootout" => {
                state.lock().unwrap().remove(&target);
                Ok(String::new())
            }
            other => panic!("unexpected {other}"),
        }
    }));
    let engine = Engine::new(root.path(), runner.clone())
        .with_uid(501)
        .with_poll_interval(std::time::Duration::from_millis(1));
    loaded.lock().unwrap().insert(engine.target("db-api"), true);
    engine.stop_job("db-api", &|| false).unwrap();
    assert!(
        !loaded
            .lock()
            .unwrap()
            .contains_key(&engine.target("db-api"))
    );
    let calls = runner.calls();
    assert_eq!(
        calls
            .iter()
            .map(|call| call[1].as_str())
            .collect::<Vec<_>>(),
        ["print", "bootout", "print"]
    );
}

#[test]
fn stop_job_treats_an_absent_job_as_success() {
    let root = temp_root();
    let runner = Arc::new(FakeCommands::new(|_, _| {
        Err("Could not find service".to_string())
    }));
    let engine = Engine::new(root.path(), runner.clone()).with_uid(501);
    engine.stop_job("db-api", &|| false).unwrap();
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn parse_job_state_ignores_nested_coalitions() {
    let state = parse_job_state(
        "gui/501/example = {\n\tstate = running\n\truns = 2\n\tpid = 123\n\tlast exit code = 7\n\tlast terminating signal = Killed: 9\n\tresource coalition = {\n\t\tstate = active\n\t\tpid = 999\n\t}\n}",
    );
    assert_eq!(
        state,
        JobState {
            running: true,
            pid: 123,
            runs: 2,
            last_exit_code: Some(7),
            last_signal: "9".into(),
            exit_reason: false,
        }
    );
}

#[test]
fn job_state_finished_and_successful_exit() {
    let mut state = JobState {
        runs: 1,
        last_exit_code: Some(0),
        ..JobState::default()
    };
    assert!(state.finished());
    assert!(state.successful_exit());
    state.last_signal = "9".into();
    assert!(state.finished());
    assert!(!state.successful_exit());
}

#[test]
fn health_validation_accepts_only_the_documented_shapes() {
    for address in [
        "",
        "process",
        "tcp://127.0.0.1:3000",
        "http://localhost:8080/ready",
    ] {
        assert!(validate_health(address).is_ok(), "{address}");
    }
    for address in [
        "shell:echo ok",
        "tcp://localhost:0",
        "http://user:password@localhost/",
    ] {
        assert!(validate_health(address).is_err(), "{address}");
    }
}

#[test]
fn readiness_waits_for_a_stable_process() {
    let root = temp_root();
    let runner = Arc::new(FakeCommands::new(|_, _| {
        Ok("state = running\npid = 42\n".to_string())
    }));
    let engine = Engine::new(root.path(), runner).with_uid(501);
    let probe = FakeProbe::passing();
    let service = ServiceProbe {
        id: "api".into(),
        health: "process".into(),
        readiness_timeout_seconds: 2,
    };
    let started = std::time::Instant::now();
    wait_service(&engine, &probe, "db-api", &service, &|| false).unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_millis(190));
}

#[test]
fn readiness_fails_fast_after_a_recorded_exit() {
    let root = temp_root();
    let runner = Arc::new(FakeCommands::new(|_, _| {
        Ok("state = exited\nruns = 1\nlast exit code = 3\n".to_string())
    }));
    let engine = Engine::new(root.path(), runner).with_uid(501);
    let probe = FakeProbe::passing();
    let service = ServiceProbe {
        id: "api".into(),
        health: "process".into(),
        readiness_timeout_seconds: 2,
    };
    let error = wait_service(&engine, &probe, "db-api", &service, &|| false).unwrap_err();
    assert!(error.to_string().contains("exited with code 3"), "{error}");
}

#[test]
fn readiness_times_out_when_the_probe_never_passes() {
    let root = temp_root();
    let runner = Arc::new(FakeCommands::new(|_, _| {
        Ok("state = running\npid = 42\n".to_string())
    }));
    let engine = Engine::new(root.path(), runner).with_uid(501);
    let probe = FakeProbe::failing("refused");
    let service = ServiceProbe {
        id: "api".into(),
        health: "tcp://127.0.0.1:9".into(),
        readiness_timeout_seconds: 1,
    };
    let error = wait_service(&engine, &probe, "db-api", &service, &|| false).unwrap_err();
    assert!(
        error.to_string().contains("did not become ready"),
        "{error}"
    );
    assert!(!probe.calls().is_empty());
}

#[test]
fn read_job_state_reports_observation_failures() {
    let root = temp_root();
    let runner = Arc::new(FakeCommands::new(|_, _| {
        Err("launchd observation unavailable".into())
    }));
    let engine = Engine::new(root.path(), runner).with_uid(501);
    let (state, result) = crate::health::read_job_state(&engine, "db-api", &|| false);
    assert_eq!(state, JobState::default());
    assert!(result.is_err());
}

#[test]
fn systemd_user_backend_starts_stops_verifies_and_schedules_periodic_jobs() {
    let root = temp_root();
    let unit_dir = root.path().join("xdg-systemd-user");
    let active_units: Arc<Mutex<BTreeMap<String, bool>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let state_ref = active_units.clone();

    let runner = Arc::new(FakeCommands::new(move |program, args| {
        assert_eq!(program, "systemctl");
        assert_eq!(args.first().map(String::as_str), Some("--user"));
        match args.get(1).map(String::as_str) {
            Some("show") => {
                let unit = args.last().cloned().unwrap_or_default();
                let is_active = state_ref
                    .lock()
                    .unwrap()
                    .get(&unit)
                    .copied()
                    .unwrap_or(false);
                if is_active {
                    Ok("LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=0\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n".into())
                } else {
                    Ok("LoadState=not-found\nActiveState=inactive\nSubState=dead\nMainPID=0\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n".into())
                }
            }
            Some("start") => {
                let unit = args.get(2).cloned().unwrap_or_default();
                state_ref.lock().unwrap().insert(unit, true);
                Ok(String::new())
            }
            Some("stop") => {
                let unit = args.get(2).cloned().unwrap_or_default();
                state_ref.lock().unwrap().remove(&unit);
                Ok(String::new())
            }
            Some("daemon-reload") | Some("reset-failed") => Ok(String::new()),
            other => panic!("unexpected systemctl call {other:?}: {args:?}"),
        }
    }));

    let backend = Arc::new(SystemdUserBackend::with_unit_dir(&unit_dir));
    let engine = Engine::new(root.path(), runner.clone())
        .with_backend(backend.clone())
        .with_poll_interval(std::time::Duration::from_millis(1));

    let job = JobSpec {
        id: "db-pg".into(),
        args: vec![
            "/opt/pg/bin/postgres".into(),
            "-D".into(),
            "/data/pg".into(),
        ],
        working_dir: root.path().to_string_lossy().into_owned(),
        env: BTreeMap::from([("PGDATA".into(), "/data/pg".into())]),
        restart: true,
    };

    assert!(engine.verify_stopped("db-pg").unwrap());
    assert!(engine.start_job(&job, &|| false).unwrap());
    // Starting an already-running job is a no-op.
    assert!(!engine.start_job(&job, &|| false).unwrap());

    let unit_path = engine.job_path("db-pg");
    assert!(unit_path.exists());
    let mode = std::os::unix::fs::PermissionsExt::mode(
        &std::fs::metadata(&unit_path).unwrap().permissions(),
    );
    assert_eq!(mode & 0o777, 0o600);
    let link = unit_dir.join(backend.unit_name(&engine, "db-pg"));
    assert!(std::fs::symlink_metadata(&link).is_ok());

    let observed = engine.job_state("db-pg").unwrap().expect("running");
    assert!(observed.loaded);
    assert!(observed.running);
    assert!(!engine.verify_stopped("db-pg").unwrap());

    // jobs_referencing finds the systemd service referencing `/opt/pg`.
    assert_eq!(
        engine.jobs_referencing(Path::new("/opt/pg")).unwrap(),
        vec!["db-pg".to_string()]
    );

    engine.stop_job("db-pg", &|| false).unwrap();
    assert!(engine.verify_stopped("db-pg").unwrap());
    assert!(std::fs::symlink_metadata(&link).is_err());

    // Periodic job writes both .service and .timer and starts the .timer.
    engine
        .ensure_periodic_job(
            "maintenance",
            &["/usr/bin/devbase".into(), "--dev".into(), "maintain".into()],
            60,
            Path::new(""),
        )
        .unwrap();
    let timer_path = engine
        .jobs_directory()
        .join(backend.timer_name(&engine, "maintenance"));
    assert!(engine.job_path("maintenance").exists());
    assert!(timer_path.exists());
    let timer_body = std::fs::read_to_string(&timer_path).unwrap();
    assert!(timer_body.contains("OnUnitActiveSec=60s"));
    assert!(
        active_units
            .lock()
            .unwrap()
            .get(&backend.timer_name(&engine, "maintenance"))
            .copied()
            .unwrap_or(false)
    );

    engine.stop_periodic_job("maintenance", &|| false).unwrap();
    assert!(!engine.job_path("maintenance").exists());
    assert!(!timer_path.exists());
}

#[test]
fn jobs_referencing_scans_launchd_plists_and_systemd_units() {
    let root = temp_root();
    let runner = Arc::new(FakeCommands::new(|_, _| Ok(String::new())));
    let engine = Engine::new(root.path(), runner);
    std::fs::create_dir_all(engine.jobs_directory()).unwrap();
    let prefix = label_prefix(root.path());
    std::fs::write(
        engine
            .jobs_directory()
            .join(format!("xyz.jyeu.devbase.dev.{prefix}.db-a.plist")),
        "<string>/packages/postgres@17/bin/postgres</string>",
    )
    .unwrap();
    std::fs::write(
        engine
            .jobs_directory()
            .join(format!("devbase-{prefix}-db-b.service")),
        "ExecStart=/packages/postgres@17/bin/postgres\n",
    )
    .unwrap();
    std::fs::write(
        engine
            .jobs_directory()
            .join(format!("devbase-{prefix}-db-c.service")),
        "ExecStart=/packages/redis@8/bin/redis-server\n",
    )
    .unwrap();
    assert_eq!(
        engine
            .jobs_referencing(Path::new("/packages/postgres@17"))
            .unwrap(),
        vec!["db-a".to_string(), "db-b".to_string()]
    );
}
