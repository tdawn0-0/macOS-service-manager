use std::os::unix::net::UnixStream;

use super::auth::PeerAuth;
use super::http::Request;
use super::protocol::ErrorResponse;
use super::server::Server;
use super::test_support::{FakePeerAuth, euid, server_config};
use super::{PEER_REJECTED_MESSAGE, PROTOCOL_VERSION, ServerConfig};

fn pair() -> (UnixStream, UnixStream) {
    UnixStream::pair().expect("socket pair")
}

fn request(method: &str, path: &str) -> Request {
    Request::new(method, path)
}

fn body_json(response: &super::http::Response) -> serde_json::Value {
    serde_json::from_slice(&response.body).expect("JSON body")
}

fn error_of(response: &super::http::Response) -> String {
    serde_json::from_slice::<ErrorResponse>(&response.body)
        .expect("error body")
        .error
}

#[test]
fn missing_peer_connection_is_forbidden() {
    let server = Server::new(server_config());
    let response = server.handle(&request("GET", "/v1/status"), None);
    assert_eq!(response.status, 403);
    assert_eq!(error_of(&response), "missing peer connection");
}

#[test]
fn a_peer_uid_that_cannot_be_read_is_forbidden() {
    let server = Server::new(ServerConfig {
        peer_auth: FakePeerAuth::uid_error(),
        ..server_config()
    });
    let (stream, _peer) = pair();
    let response = server.handle(&request("GET", "/v1/status"), Some(&stream));
    assert_eq!(response.status, 403);
    assert_eq!(error_of(&response), "failed to verify peer uid");
}

#[test]
fn a_different_uid_is_rejected_before_the_code_check() {
    let auth = FakePeerAuth::allowing(euid() + 1);
    let server = Server::new(ServerConfig {
        peer_auth: auth.clone(),
        ..server_config()
    });
    let (stream, _peer) = pair();
    let response = server.handle(&request("GET", "/v1/status"), Some(&stream));
    assert_eq!(response.status, 403);
    assert_eq!(
        error_of(&response),
        format!("peer uid {} is not authorized", euid() + 1)
    );
    assert_eq!(auth.calls(), vec!["uid"]);
}

#[test]
fn a_failed_code_check_returns_the_reinstall_message() {
    let auth = FakePeerAuth::rejecting_code(euid());
    let server = Server::new(ServerConfig {
        peer_auth: auth.clone(),
        ..server_config()
    });
    let (stream, _peer) = pair();
    let response = server.handle(&request("GET", "/v1/status"), Some(&stream));
    assert_eq!(response.status, 403);
    assert_eq!(error_of(&response), PEER_REJECTED_MESSAGE);
    assert_eq!(auth.calls(), vec!["uid", "code"]);
}

#[test]
fn authorization_runs_before_routing() {
    let server = Server::new(ServerConfig {
        peer_auth: FakePeerAuth::rejecting_code(euid()),
        ..server_config()
    });
    let (stream, _peer) = pair();
    let response = server.handle(&request("GET", "/not-a-route"), Some(&stream));
    assert_eq!(response.status, 403);
    assert_eq!(error_of(&response), PEER_REJECTED_MESSAGE);

    let server = Server::new(server_config());
    let response = server.handle(&request("GET", "/not-a-route"), Some(&stream));
    assert_eq!(response.status, 404);
    assert_eq!(response.body, b"404 page not found\n");
}

#[test]
fn every_endpoint_requires_the_code_check() {
    let server = Server::new(ServerConfig {
        peer_auth: FakePeerAuth::rejecting_code(euid()),
        ..server_config()
    });
    let (stream, _peer) = pair();
    for (method, path) in [
        ("POST", "/v1/proxy-socket/install"),
        ("POST", "/v1/proxy-socket/stop"),
        ("POST", "/v1/proxy-socket/kickstart"),
        ("POST", "/v1/ports/inspect"),
        ("POST", "/v1/launchd/control"),
        ("POST", "/v1/launchd/save"),
        ("POST", "/v1/launchd/remove"),
    ] {
        let response = server.handle(&request(method, path), Some(&stream));
        assert_eq!(response.status, 403, "{method} {path}");
        assert_eq!(error_of(&response), PEER_REJECTED_MESSAGE);
    }
}

#[test]
fn status_reports_the_owner_uid_after_authorization() {
    let owner = i64::from(euid());
    let server = Server::new(server_config());
    let (stream, _peer) = pair();
    let response = server.handle(&request("GET", "/v1/status"), Some(&stream));
    assert_eq!(response.status, 200);
    let value = body_json(&response);
    assert_eq!(value["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(value["ownerUid"], owner);
    assert_eq!(
        response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.as_str()),
        Some("application/json")
    );
}

#[test]
fn the_real_audit_token_identifies_this_process() {
    let (stream, _peer) = pair();
    let token = super::auth::peer_audit_token(&stream).expect("audit token");
    assert_eq!(token.len(), 32);
    assert_ne!(token, [0u8; 32]);
    super::auth::peer_runs_helper_code(&stream).expect("self must satisfy our requirement");
}

#[test]
fn self_requirement_accepts_self_and_rejects_foreign_code() {
    let pid = std::process::id() as i32;
    super::auth::pid_satisfies_self(pid).expect("own process satisfies own requirement");
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let foreign = super::auth::pid_satisfies_self(child.id() as i32);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        foreign.is_err(),
        "/bin/sleep must not satisfy the test binary"
    );
}

#[test]
fn system_peer_auth_reads_this_process_uid() {
    let (stream, _peer) = pair();
    let uid = super::auth::SystemPeerAuth
        .peer_uid(&stream)
        .expect("getpeereid");
    assert_eq!(uid, euid());
}
