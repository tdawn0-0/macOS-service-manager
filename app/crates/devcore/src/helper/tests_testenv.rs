use std::time::Duration;

use super::server::ServerConfig;
use super::test_support::server_config;
use super::testenv::TestHelper;
use super::{HelperState, PROTOCOL_VERSION};

#[test]
fn the_in_process_helper_reports_installed_and_serves_status() {
    let helper = TestHelper::install(ServerConfig {
        idle_timeout: Duration::from_secs(10),
        ..server_config()
    });
    let client = helper.client();
    assert_eq!(client.status().state, HelperState::Installed);
    let response = client.ping().expect("ping");
    assert_eq!(response.protocol_version, PROTOCOL_VERSION);
    assert_eq!(response.owner_uid, i64::from(helper.owner_uid()));
    assert!(helper.socket_path().exists());
    assert!(helper.plist_path().exists());
    assert!(helper.helper_dir().join("devbase-helper").exists());
}

#[test]
fn dropping_the_test_helper_stops_it_and_cleans_up() {
    let socket;
    let plist;
    {
        let helper = TestHelper::install(server_config());
        socket = helper.socket_path();
        plist = helper.plist_path();
        assert!(socket.exists());
    }
    assert!(!socket.exists(), "the temp socket directory is removed");
    assert!(!plist.exists(), "the temp plist directory is removed");
}

#[test]
fn the_test_helper_stop_is_idempotent() {
    let mut helper = TestHelper::install(server_config());
    helper.stop();
    helper.stop();
    let _ = helper.client().ping().expect_err("server stopped");
}
