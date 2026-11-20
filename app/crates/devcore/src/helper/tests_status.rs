use std::path::{Path, PathBuf};

use plist::{Dictionary, Value};

use super::client::{BinaryInfo, Client, ClientConfig};
use super::test_support::{FakeAdmin, FakePeerAuth, euid, server_config};
use super::testenv::TestHelper;
use super::{HELPER_DIR, HelperState, LABEL, PLIST_PATH, SOCKET_PATH, needs_reinstall_message};

fn client(dir: &Path, owner_uid: u32) -> Client {
    Client::new(ClientConfig {
        socket_path: dir.join("helper.sock"),
        plist_path: dir.join(format!("{LABEL}.plist")),
        helper_dir: dir.join("helper"),
        expected: BinaryInfo {
            executable: PathBuf::from("/Applications/Devbase.app/Contents/MacOS/Devbase"),
            bundle_path: None,
            owner_uid,
        },
        admin: FakeAdmin::succeeding(),
        ..ClientConfig::default()
    })
}

fn write_plist(
    path: &Path,
    label: &str,
    arguments: &[String],
    socket_name: &str,
    socket_path: &str,
) {
    let mut socket = Dictionary::new();
    socket.insert(
        "SockPathName".to_string(),
        Value::String(socket_path.to_string()),
    );
    socket.insert("SockPathMode".to_string(), Value::Integer(0o666.into()));
    let mut sockets = Dictionary::new();
    sockets.insert(socket_name.to_string(), Value::Dictionary(socket));
    let mut doc = Dictionary::new();
    doc.insert("Label".to_string(), Value::String(label.to_string()));
    doc.insert(
        "ProgramArguments".to_string(),
        Value::Array(arguments.iter().cloned().map(Value::String).collect()),
    );
    doc.insert("Sockets".to_string(), Value::Dictionary(sockets));
    let mut out = Vec::new();
    Value::Dictionary(doc).to_writer_xml(&mut out).expect("xml");
    std::fs::write(path, out).expect("write plist");
}

fn installed_plist(dir: &Path, owner_uid: u32) -> Vec<String> {
    vec![
        dir.join("helper/devbase-helper")
            .to_str()
            .expect("utf8")
            .to_string(),
        "--dev".to_string(),
        "helper".to_string(),
        "--owner-uid".to_string(),
        owner_uid.to_string(),
    ]
}

#[test]
fn passive_status_transitions_and_messages() {
    let dir = tempfile::tempdir().expect("temp dir");
    let owner_uid = euid();
    let client = client(dir.path(), owner_uid);
    let plist_path = client.config().plist_path.clone();

    let status = client.status();
    assert_eq!(status.state, HelperState::NotInstalled);
    assert_eq!(
        status.message,
        "Not installed. Root operations need it; installing asks for your administrator password once."
    );

    write_plist(
        &plist_path,
        LABEL,
        &installed_plist(dir.path(), owner_uid + 1),
        "helper",
        client.config().socket_path.to_str().expect("utf8"),
    );
    assert_eq!(client.status().state, HelperState::NotInstalled);
    assert_eq!(
        client.status().message,
        "Installed helper is configured for a different user and needs to be reinstalled."
    );

    write_plist(
        &plist_path,
        LABEL,
        &installed_plist(dir.path(), owner_uid),
        "helper",
        client.config().socket_path.to_str().expect("utf8"),
    );
    assert_eq!(
        client.status().message,
        "Installed helper binary is missing and needs to be reinstalled."
    );

    write_plist(
        &plist_path,
        "com.example.other",
        &installed_plist(dir.path(), owner_uid),
        "helper",
        client.config().socket_path.to_str().expect("utf8"),
    );
    assert_eq!(
        client.status().message,
        "Installed helper points to a different location and needs to be reinstalled."
    );

    write_plist(
        &plist_path,
        LABEL,
        &installed_plist(dir.path(), owner_uid),
        "helper",
        "/var/run/other.sock",
    );
    assert_eq!(
        client.status().message,
        "Installed helper socket configuration is invalid and needs to be reinstalled."
    );

    write_plist(
        &plist_path,
        LABEL,
        &installed_plist(dir.path(), owner_uid),
        "other",
        client.config().socket_path.to_str().expect("utf8"),
    );
    assert_eq!(
        client.status().message,
        "Installed helper socket configuration is invalid and needs to be reinstalled."
    );

    std::fs::write(&plist_path, b"not a plist").expect("write bad plist");
    assert_eq!(
        client.status().message,
        "Installed helper configuration is invalid and needs to be reinstalled."
    );

    write_plist(
        &plist_path,
        LABEL,
        &installed_plist(dir.path(), owner_uid),
        "helper",
        client.config().socket_path.to_str().expect("utf8"),
    );
    std::fs::create_dir_all(dir.path().join("helper")).expect("helper dir");
    let staged = dir.path().join("helper/devbase-helper");
    std::fs::write(&staged, b"bin").expect("staged");
    let status = client.status();
    assert_eq!(status.state, HelperState::Installed);
    assert_eq!(
        status.message,
        "Installed. Root operations run through it without password prompts."
    );
    assert!(client.ready());
}

#[test]
fn unreadable_plist_reports_the_inspection_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = client(dir.path(), euid());
    // A directory at the plist path makes read() fail with EISDIR.
    std::fs::create_dir(client.config().plist_path.clone()).expect("dir at plist path");
    let status = client.status();
    assert_eq!(status.state, HelperState::NotInstalled);
    assert!(
        status
            .message
            .starts_with("Could not inspect helper configuration: open "),
        "unexpected message: {}",
        status.message
    );
}

#[test]
fn disabled_client_reports_the_disabled_message() {
    let status = Client::disabled().status();
    assert_eq!(status.state, HelperState::NotInstalled);
    assert_eq!(
        status.message,
        "Privileged helper is disabled in this environment."
    );
    assert!(!Client::disabled().ready());
}

#[test]
fn staged_paths_follow_the_bundle_envelope() {
    let info = BinaryInfo {
        executable: PathBuf::from("/Applications/Devbase.app/Contents/MacOS/Devbase"),
        bundle_path: Some(PathBuf::from("/Applications/Devbase.app")),
        owner_uid: 501,
    };
    assert_eq!(
        super::expected_staged_executable(Path::new(HELPER_DIR), &info),
        Path::new(HELPER_DIR).join("bundle/Contents/MacOS/Devbase")
    );
    assert_eq!(
        super::expected_staged_executable(
            Path::new("/tmp/h"),
            &super::BinaryInfo {
                executable: PathBuf::from("/Applications/Devbase.app/Contents/MacOS/Devbase"),
                bundle_path: None,
                owner_uid: 501,
            }
        ),
        PathBuf::from("/tmp/h/devbase-helper")
    );
}

#[test]
fn a_foreign_build_is_refused_by_every_endpoint() {
    let cfg = server_config();
    let helper = TestHelper::install(super::ServerConfig {
        peer_auth: FakePeerAuth::rejecting_code(euid()),
        ..cfg
    });
    let client = helper.client();
    let error = client.ping().expect_err("foreign build must be refused");
    assert!(error.is_peer_rejected(), "unexpected error: {error}");
    assert!(error.needs_reinstall());
    assert_eq!(
        error.to_string(),
        "peer is not the Devbase build this helper was installed from; reinstall the helper in Settings"
    );
    let error = client
        .launchd_control(&[vec!["print".to_string(), "system/x".to_string()]])
        .expect_err("control must be refused too");
    assert!(error.is_peer_rejected());
    assert!(needs_reinstall_message(&error.to_string()));
}

#[test]
fn needs_reinstall_recognizes_the_text_and_the_type() {
    use super::HelperError;
    assert!(!HelperError::Message("other".to_string()).needs_reinstall());
    assert!(!HelperError::NotInstalled.needs_reinstall());
    assert!(super::needs_reinstall_message(&format!(
        "apply: {PEER_REJECTED}",
        PEER_REJECTED = super::PEER_REJECTED_MESSAGE
    )));
    assert!(!super::needs_reinstall_message("plain failure"));
}

#[test]
fn helper_paths_match_the_go_identity() {
    assert_eq!(LABEL, "xyz.jyeu.devbase.helper");
    assert_eq!(
        PLIST_PATH,
        "/Library/LaunchDaemons/xyz.jyeu.devbase.helper.plist"
    );
    assert_eq!(
        HELPER_DIR,
        "/Library/PrivilegedHelperTools/xyz.jyeu.devbase.helper"
    );
    assert_eq!(SOCKET_PATH, "/var/run/xyz.jyeu.devbase.helper.sock");
}
