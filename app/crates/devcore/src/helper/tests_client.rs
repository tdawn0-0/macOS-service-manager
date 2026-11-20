use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::client::{BinaryInfo, Client, ClientConfig};
use super::protocol::LaunchdControlRequest;
use super::server::ServerConfig;
use super::test_support::{FakePeerAuth, FakeRunner, euid, server_config};
use super::testenv::TestHelper;
use super::{HelperError, PEER_REJECTED_MESSAGE};
use crate::launchd::{
    ErrorCode, HelperApi, LaunchdRemoveRequest, LaunchdSaveRequest, PrivilegedHelper,
};

struct RawServer {
    dir: tempfile::TempDir,
    socket: PathBuf,
    thread: std::thread::JoinHandle<Vec<u8>>,
}

fn raw_server(response: Vec<u8>) -> RawServer {
    let dir = tempfile::Builder::new()
        .prefix("devbase-ph-")
        .tempdir_in("/tmp")
        .expect("short socket dir");
    let socket = dir.path().join("raw.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let request = read_raw_request(&mut stream);
        stream.write_all(&response).expect("write response");
        stream.flush().expect("flush");
        request
    });
    RawServer {
        dir,
        socket,
        thread,
    }
}

fn read_raw_request(stream: &mut UnixStream) -> Vec<u8> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let header_end = loop {
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        let read = stream.read(&mut buffer).expect("read headers");
        if read == 0 {
            return data;
        }
        data.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8_lossy(&data[..header_end]).into_owned();
    let length = head
        .split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while data.len() < header_end + length {
        let read = stream.read(&mut buffer).expect("read body");
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
    }
    data
}

fn client_for_path(socket: &Path) -> Client {
    Client::new(ClientConfig {
        socket_path: socket.to_path_buf(),
        expected: BinaryInfo {
            executable: "/Applications/Devbase.app/Contents/MacOS/Devbase".into(),
            bundle_path: None,
            owner_uid: euid(),
        },
        ..ClientConfig::default()
    })
}

#[test]
fn get_request_bytes_match_the_go_client() {
    let server = raw_server(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 38\r\nConnection: close\r\n\r\n{\"protocolVersion\":1,\"ownerUid\":501}".to_vec(),
    );
    let client = client_for_path(&server.socket);
    let response = client.ping().expect("ping");
    assert_eq!(response.protocol_version, 1);
    assert_eq!(response.owner_uid, 501);
    let raw = server.thread.join().expect("server thread");
    let text = String::from_utf8_lossy(&raw);
    assert!(text.starts_with("GET /v1/status HTTP/1.1\r\n"), "{text}");
    assert!(text.contains("Host: unix\r\n"), "{text}");
    assert!(text.contains("Connection: close\r\n"), "{text}");
    assert!(!text.contains("Content-Type"), "{text}");
    assert!(!text.contains("Content-Length"), "{text}");
    drop(server.dir);
}

#[test]
fn post_request_bytes_carry_compact_json() {
    let server = raw_server(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}".to_vec(),
    );
    let client = client_for_path(&server.socket);
    client
        .launchd_control(&[vec!["print".to_string(), "system/x".to_string()]])
        .expect("control");
    let raw = server.thread.join().expect("server thread");
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").expect("headers");
    assert!(head.starts_with("POST /v1/launchd/control HTTP/1.1\r\n"));
    assert!(head.contains("Content-Type: application/json\r\n"));
    let expected_body = serde_json::to_string(&LaunchdControlRequest {
        commands: vec![vec!["print".to_string(), "system/x".to_string()]],
    })
    .expect("json");
    assert_eq!(body, expected_body);
    assert!(head.contains(&format!("Content-Length: {}", expected_body.len())));
    drop(server.dir);
}

#[test]
fn http_errors_map_to_typed_and_message_errors() {
    let body = format!(r#"{{"error":"{PEER_REJECTED_MESSAGE}"}}"#);
    let server = raw_server(
        format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .into_bytes(),
    );
    let client = client_for_path(&server.socket);
    let error = client.ping().expect_err("rejected");
    assert_eq!(error, HelperError::PeerRejected);
    assert!(error.needs_reinstall());
    let _ = server.thread.join();

    let server = raw_server(
        b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"error\":\"bad\"}".to_vec(),
    );
    let error = client_for_path(&server.socket)
        .ping()
        .expect_err("bad request");
    assert_eq!(error.to_string(), "bad");
    let _ = server.thread.join();

    let server = raw_server(
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: 4\r\nConnection: close\r\n\r\nboom".to_vec(),
    );
    let error = client_for_path(&server.socket)
        .ping()
        .expect_err("server error");
    assert_eq!(
        error.to_string(),
        "helper GET /v1/status failed with HTTP 500"
    );
    let _ = server.thread.join();
}

#[test]
fn connect_retries_while_launchd_starts_the_helper() {
    let dir = tempfile::Builder::new()
        .prefix("devbase-ph-")
        .tempdir_in("/tmp")
        .expect("short socket dir");
    let socket = dir.path().join("late.sock");
    let socket_for_thread = socket.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(40));
        let listener = UnixListener::bind(&socket_for_thread).expect("bind");
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_raw_request(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 36\r\nConnection: close\r\n\r\n{\"protocolVersion\":1,\"ownerUid\":5}",
            )
            .expect("respond");
    });
    let response = client_for_path(&socket).ping().expect("retry connects");
    assert_eq!(response.owner_uid, 5);
    let _ = thread.join();
}

#[test]
fn host_launchd_requests_map_to_the_protocol() {
    let dir = tempfile::tempdir().expect("temp dir");
    let runner = FakeRunner::succeeding();
    let cfg = ServerConfig {
        managed_launchd_dirs: vec![dir.path().to_str().expect("utf8").to_string()],
        runner: runner.clone(),
        ..server_config()
    };
    let helper = TestHelper::install(cfg);
    let client = helper.client();
    let path = dir.path().join("com.example.worker.plist");
    let plist = br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>com.example.worker</string></dict></plist>"#.to_vec();
    let saved = HelperApi::launchd_save(
        &*client,
        LaunchdSaveRequest {
            plist: plist.clone(),
            path: path.clone(),
            domain: "system".to_string(),
            target: "system/com.example.worker".to_string(),
            existing_path: None,
            load: true,
        },
    )
    .expect("save");
    assert!(saved.saved);
    assert!(saved.error.is_none());
    assert_eq!(std::fs::read(&path).expect("saved"), plist);
    assert_eq!(
        runner.argv(),
        vec![vec![
            "/bin/launchctl".to_string(),
            "bootstrap".to_string(),
            "system".to_string(),
            path.to_str().expect("utf8").to_string()
        ]]
    );

    let trash = tempfile::tempdir().expect("trash");
    let removed = HelperApi::launchd_remove(
        &*client,
        LaunchdRemoveRequest {
            path: path.clone(),
            target: "system/com.example.worker".to_string(),
            trash_dir: trash.path().to_path_buf(),
        },
    );
    removed.expect("remove");
    assert!(!path.exists());
}

#[test]
fn helper_api_maps_rejection_to_a_launchd_error() {
    let cfg = ServerConfig {
        peer_auth: FakePeerAuth::rejecting_code(euid()),
        ..server_config()
    };
    let helper = TestHelper::install(cfg);
    let client = helper.client();
    let error = HelperApi::launchd_control(
        &*client,
        &[vec!["print".to_string(), "system/x".to_string()]],
    )
    .expect_err("rejected");
    assert_eq!(error.code, ErrorCode::Helper);
    assert_eq!(error.message, PEER_REJECTED_MESSAGE);
}

#[test]
fn privileged_helper_require_checks_readiness() {
    let helper = TestHelper::install(server_config());
    let client = helper.client();
    let api = PrivilegedHelper::require(&*client).expect("installed helper is ready");
    let _ = api;

    let absent = Client::new(ClientConfig {
        socket_path: PathBuf::from("/tmp/never.sock"),
        plist_path: PathBuf::from("/tmp/never.plist"),
        helper_dir: PathBuf::from("/tmp/never"),
        ..ClientConfig::default()
    });
    let error = PrivilegedHelper::require(&absent)
        .err()
        .expect("not installed");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);
}

#[test]
fn set_default_for_test_overrides_and_restores() {
    let previous = Client::default_client();
    assert!(previous.is_disabled(), "cfg(test) defaults to disabled");
    let replacement = Arc::new(Client::disabled());
    let guard = Client::set_default_for_test(Arc::clone(&replacement));
    assert!(Arc::ptr_eq(&Client::default_client(), &replacement));
    drop(guard);
    assert!(Client::default_client().is_disabled());
    let error = Client::require().expect_err("disabled default is not installed");
    assert_eq!(error, HelperError::NotInstalled);
}

#[test]
fn disabled_client_rejects_operations() {
    let client = Client::disabled();
    assert_eq!(
        client.ping().expect_err("disabled").to_string(),
        "privileged helper is not enabled"
    );
    assert!(!client.ready());
    assert_eq!(
        client.status().message,
        "Privileged helper is disabled in this environment."
    );
}
