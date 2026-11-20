use std::path::{Path, PathBuf};

use plist::Value;

use super::server::ServerConfig;
use super::socket_job::{
    DEFAULT_SOCKET_JOB_DIR, build_socket_job, equivalent_plists, install_socket_job,
    kickstart_socket_job, socket_job_path, socket_label, socket_target, stop_socket_job,
};
use super::test_support::{FakeRunner, server_config};
use super::testenv::TestHelper;
use crate::engine::label_prefix;

fn root() -> PathBuf {
    PathBuf::from("/Users/alice/Library/Application Support/Devbase")
}

#[test]
fn label_target_and_path_derive_from_the_workspace_root() {
    let root = root();
    let prefix = label_prefix(&root);
    let label = socket_label(&root);
    assert_eq!(
        label,
        format!("xyz.jyeu.devbase.dev.{prefix}.proxy-sockets")
    );
    assert_eq!(socket_target(&root), format!("system/{label}"));
    assert_eq!(
        socket_job_path(&root, None),
        Path::new(DEFAULT_SOCKET_JOB_DIR).join(format!("{label}.plist"))
    );
    assert_eq!(
        socket_job_path(&root, Some(Path::new("/tmp/jobs"))),
        Path::new("/tmp/jobs").join(format!("{label}.plist"))
    );
}

fn job_for(dir: &Path, ports: &[i64]) -> super::socket_job::SocketJob {
    build_socket_job(
        &root(),
        Some(dir),
        Path::new("/Applications/Devbase.app/Contents/MacOS/Devbase"),
        "alice",
        ports,
        "/Users/alice",
    )
    .expect("build")
}

#[test]
fn the_plist_describes_a_user_proxy_with_socket_activation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let job = job_for(dir.path(), &[8443, 443, 8443]);
    let value = Value::from_reader(std::io::Cursor::new(&job.plist)).expect("plist");
    let Value::Dictionary(doc) = value else {
        panic!("top level must be a dictionary");
    };
    assert_eq!(
        doc.get("Label"),
        Some(&Value::String(socket_label(&root())))
    );
    assert_eq!(doc.get("UserName"), Some(&Value::String("alice".into())));
    assert_eq!(
        doc.get("WorkingDirectory"),
        Some(&Value::String(root().to_str().expect("utf8").to_string()))
    );
    assert_eq!(doc.get("RunAtLoad"), Some(&Value::Boolean(true)));
    assert_eq!(
        doc.get("ProcessType"),
        Some(&Value::String("Background".into()))
    );
    assert_eq!(
        doc.get("StandardOutPath"),
        Some(&Value::String("/dev/null".into()))
    );
    assert_eq!(
        doc.get("StandardErrorPath"),
        Some(&Value::String("/dev/null".into()))
    );
    let Some(Value::Array(arguments)) = doc.get("ProgramArguments") else {
        panic!("ProgramArguments");
    };
    assert_eq!(
        arguments,
        &vec![
            Value::String("/Applications/Devbase.app/Contents/MacOS/Devbase".into()),
            Value::String("--dev".into()),
            Value::String("serve".into()),
            Value::String(root().to_str().expect("utf8").to_string()),
        ]
    );
    let Some(Value::Dictionary(environment)) = doc.get("EnvironmentVariables") else {
        panic!("EnvironmentVariables");
    };
    assert_eq!(
        environment.get("MSM_DEV_SOCKET_ACTIVATION"),
        Some(&Value::String("1".into()))
    );
    assert_eq!(
        environment.get("HOME"),
        Some(&Value::String("/Users/alice".into()))
    );
    let Some(Value::Dictionary(sockets)) = doc.get("Sockets") else {
        panic!("Sockets");
    };
    let keys: Vec<&str> = sockets.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["HTTPS-443", "HTTPS-8443"]);
    for (key, port) in [("HTTPS-443", "443"), ("HTTPS-8443", "8443")] {
        let Some(Value::Dictionary(entry)) = sockets.get(key) else {
            panic!("socket {key}");
        };
        assert_eq!(
            entry.get("SockServiceName"),
            Some(&Value::String(port.into()))
        );
        assert_eq!(
            entry.get("SockNodeName"),
            Some(&Value::String("127.0.0.1".into()))
        );
        assert_eq!(entry.get("SockFamily"), Some(&Value::String("IPv4".into())));
        assert_eq!(entry.get("SockType"), Some(&Value::String("stream".into())));
        assert_eq!(
            entry.get("SockProtocol"),
            Some(&Value::String("TCP".into()))
        );
        assert_eq!(entry.get("SockPassive"), Some(&Value::Boolean(true)));
    }
    assert_eq!(job.target, socket_target(&root()));
    assert_eq!(job.path, socket_job_path(&root(), Some(dir.path())));
}

#[test]
fn building_rejects_root_or_relative_artifacts() {
    let dir = tempfile::tempdir().expect("temp dir");
    for username in ["", "root"] {
        let error = build_socket_job(
            &root(),
            Some(dir.path()),
            Path::new("/Applications/Devbase.app/Contents/MacOS/Devbase"),
            username,
            &[443],
            "/Users/alice",
        )
        .expect_err("username must be a normal user");
        assert!(error.to_string().contains("normal user"));
    }
    let error = build_socket_job(
        &root(),
        Some(dir.path()),
        Path::new("Devbase"),
        "alice",
        &[443],
        "/Users/alice",
    )
    .expect_err("relative executable");
    assert!(error.to_string().contains("absolute"));
    let error = build_socket_job(
        &root(),
        Some(dir.path()),
        Path::new("/Applications/Devbase.app/Contents/MacOS/Devbase"),
        "alice",
        &[0],
        "/Users/alice",
    )
    .expect_err("invalid port");
    assert!(error.to_string().contains("between 1 and 65535"));
}

#[test]
fn equivalent_plists_compares_documents_not_bytes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let job = job_for(dir.path(), &[443]);
    let reordered = job.plist.clone();
    assert!(equivalent_plists(&job.plist, &reordered));
    assert!(!equivalent_plists(&job.plist, b"not a plist"));
    let other = job_for(dir.path(), &[443, 8443]);
    assert!(!equivalent_plists(&job.plist, &other.plist));
}

#[test]
fn install_stop_and_kickstart_go_through_the_helper() {
    let dir = tempfile::tempdir().expect("temp dir");
    let runner = FakeRunner::succeeding();
    let cfg = ServerConfig {
        socket_job_dirs: vec![dir.path().to_str().expect("utf8").to_string()],
        runner: runner.clone(),
        ..server_config()
    };
    let helper = TestHelper::install(cfg);
    let client = helper.client();
    let job = job_for(dir.path(), &[443, 8443]);

    install_socket_job(&client, &job).expect("install socket job");
    assert_eq!(std::fs::read(&job.path).expect("saved"), job.plist);
    assert_eq!(
        runner.argv(),
        vec![
            vec![
                "/bin/launchctl".to_string(),
                "bootout".to_string(),
                job.target.clone()
            ],
            vec![
                "/bin/launchctl".to_string(),
                "bootstrap".to_string(),
                "system".to_string(),
                job.path.to_str().expect("utf8").to_string()
            ],
        ]
    );

    kickstart_socket_job(&client, &root()).expect("kickstart");
    assert_eq!(
        runner.argv().last().expect("last command"),
        &vec![
            "/bin/launchctl".to_string(),
            "kickstart".to_string(),
            "-k".to_string(),
            job.target.clone()
        ]
    );

    stop_socket_job(&client, &job).expect("stop");
    assert!(!job.path.exists());
    let argv = runner.argv();
    assert_eq!(
        &argv[argv.len() - 2..],
        &[
            vec![
                "/bin/launchctl".to_string(),
                "print".to_string(),
                job.target.clone()
            ],
            vec![
                "/bin/launchctl".to_string(),
                "bootout".to_string(),
                job.target.clone()
            ],
        ]
    );
}
