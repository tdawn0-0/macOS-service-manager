//! Tests for Linux privileged helper components (`PkexecAdmin`, `LinuxPeerAuth`
//! hash verification, `sd_listen_fds`, systemd/CA/proxy-socket helper endpoints,
//! and Linux systemd unit status/install/uninstall).

use tempfile::TempDir;

use super::admin::{AdminError, translate_pkexec_exit};
use super::auth::verify_peer_exe_hash;
use super::client::{BinaryInfo, Client, ClientConfig};
use super::protocol::{
    CaInstallRequest, CaRemoveRequest, LinuxProxySocketInstallRequest, LinuxProxySocketStopRequest,
    SystemdRemoveRequest, SystemdSaveRequest,
};
use super::server::{CaTrustTarget, ServerConfig};
use super::systemd_unit::{
    build_helper_service_unit, build_helper_socket_unit, build_linux_proxy_socket_units,
};
use super::test_support::{FakeAdmin, FakeRunner, server_config};
use super::testenv::TestHelper;
use super::{HelperState, SD_LISTEN_FDS_START, parse_sd_listen_fds};

#[test]
fn pkexec_exit_codes_map_cancellation_and_unavailable() {
    assert_eq!(
        translate_pkexec_exit(Some(126), "dismissed", ""),
        AdminError::Canceled
    );
    assert_eq!(
        translate_pkexec_exit(Some(127), "", ""),
        AdminError::Message(
            "pkexec authorization failed or polkit authentication agent is unavailable".into()
        )
    );
    assert_eq!(
        translate_pkexec_exit(Some(1), "permission denied\n", ""),
        AdminError::Message("permission denied".into())
    );
}

#[test]
fn verify_peer_exe_hash_accepts_identical_and_rejects_mismatched_binaries() {
    let temp = TempDir::new().unwrap();
    let a = temp.path().join("devbase");
    let b = temp.path().join("devbase-helper");
    let c = temp.path().join("other");
    std::fs::write(&a, b"elf-binary-v1").unwrap();
    std::fs::write(&b, b"elf-binary-v1").unwrap();
    std::fs::write(&c, b"elf-binary-v2").unwrap();

    verify_peer_exe_hash(&a, &b).unwrap();
    assert!(verify_peer_exe_hash(&c, &b).is_err());
}

#[test]
fn parse_sd_listen_fds_handles_pid_matching_and_fdnames() {
    assert_eq!(
        parse_sd_listen_fds(42, Some("42"), Some("1"), Some("helper"), "helper").unwrap(),
        (SD_LISTEN_FDS_START, vec![])
    );
    assert_eq!(
        parse_sd_listen_fds(
            42,
            Some("42"),
            Some("3"),
            Some("other:helper:extra"),
            "helper"
        )
        .unwrap(),
        (
            SD_LISTEN_FDS_START + 1,
            vec![SD_LISTEN_FDS_START, SD_LISTEN_FDS_START + 2]
        )
    );
    // Wrong PID is rejected.
    assert!(parse_sd_listen_fds(42, Some("99"), Some("1"), None, "helper").is_err());
    // Missing or zero LISTEN_FDS is rejected.
    assert!(parse_sd_listen_fds(42, Some("42"), Some("0"), None, "helper").is_err());
    assert!(parse_sd_listen_fds(42, None, Some("1"), None, "helper").is_err());
}

#[test]
fn helper_server_handles_linux_systemd_ca_and_proxy_socket_endpoints() {
    let temp = TempDir::new().unwrap();
    let systemd_dir = temp.path().join("systemd");
    let ca_dir = temp.path().join("anchors");
    let trash_dir = temp.path().join("trash");
    std::fs::create_dir_all(&systemd_dir).unwrap();
    std::fs::create_dir_all(&ca_dir).unwrap();
    std::fs::create_dir_all(&trash_dir).unwrap();

    let runner = FakeRunner::succeeding();
    let helper = TestHelper::install(ServerConfig {
        managed_systemd_dirs: vec![systemd_dir.to_string_lossy().into_owned()],
        ca_trust_target: Some(CaTrustTarget {
            anchor_dir: ca_dir.clone(),
            extension: "crt".into(),
            update_program: "update-ca-trust".into(),
            update_args: vec![],
        }),
        runner: runner.clone(),
        ..server_config()
    });
    let client = helper.client();

    // 1. /v1/systemd/control allows validated commands and rejects helper self-targeting.
    client
        .systemd_control(&[vec!["restart".into(), "nginx.service".into()]])
        .unwrap();
    assert!(
        client
            .systemd_control(&[vec!["stop".into(), "devbase-helper.service".into()]])
            .is_err()
    );
    assert!(
        client
            .systemd_control(&[vec!["mask".into(), "nginx.service".into()]])
            .is_err()
    );

    // 2. /v1/systemd/save writes unit file and reloads/enables/starts.
    let unit_path = systemd_dir.join("myapp.service");
    client
        .systemd_save(&SystemdSaveRequest {
            unit_content: b"[Unit]\nDescription=MyApp\n[Service]\nExecStart=/bin/true\n".to_vec(),
            path: unit_path.to_string_lossy().into_owned(),
            unit_name: "myapp.service".into(),
            reload: true,
            enable: true,
            start: true,
        })
        .unwrap();
    assert!(unit_path.exists());

    // 3. /v1/systemd/remove stops, disables, and moves unit to trash_dir.
    client
        .systemd_remove(&SystemdRemoveRequest {
            path: unit_path.to_string_lossy().into_owned(),
            unit_name: "myapp.service".into(),
            trash_dir: trash_dir.to_string_lossy().into_owned(),
        })
        .unwrap();
    assert!(!unit_path.exists());
    assert!(trash_dir.join("myapp.service").exists());

    // 4. /v1/ca/install & /v1/ca/remove write/remove PEM anchor and run update-ca-trust.
    let pem = b"-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
    client
        .ca_install(&CaInstallRequest {
            cert_pem: pem.to_vec(),
            name: "devbase-local-ca".into(),
        })
        .unwrap();
    assert_eq!(
        std::fs::read(ca_dir.join("devbase-local-ca.crt")).unwrap(),
        pem
    );
    // Rejects invalid PEM or path traversal.
    assert!(
        client
            .ca_install(&CaInstallRequest {
                cert_pem: b"not a cert".to_vec(),
                name: "devbase-local-ca".into(),
            })
            .is_err()
    );
    assert!(
        client
            .ca_remove(&CaRemoveRequest {
                name: "../evil".into(),
            })
            .is_err()
    );
    client
        .ca_remove(&CaRemoveRequest {
            name: "devbase-local-ca".into(),
        })
        .unwrap();
    assert!(!ca_dir.join("devbase-local-ca.crt").exists());

    // 5. /v1/systemd/proxy-socket/install & stop.
    let units = build_linux_proxy_socket_units(
        temp.path(),
        Some(&systemd_dir),
        std::path::Path::new("/usr/bin/devbase"),
        "alice",
        &[443],
        "/home/alice",
    )
    .unwrap();
    client
        .linux_proxy_socket_install(&LinuxProxySocketInstallRequest {
            socket_unit_name: units.socket_unit_name.clone(),
            service_unit_name: units.service_unit_name.clone(),
            socket_content: units.socket_content.into_bytes(),
            service_content: units.service_content.into_bytes(),
        })
        .unwrap();
    assert!(systemd_dir.join(&units.socket_unit_name).exists());
    assert!(systemd_dir.join(&units.service_unit_name).exists());

    client
        .linux_proxy_socket_stop(&LinuxProxySocketStopRequest {
            socket_unit_name: units.socket_unit_name.clone(),
            service_unit_name: units.service_unit_name.clone(),
        })
        .unwrap();
    assert!(!systemd_dir.join(&units.socket_unit_name).exists());
    assert!(!systemd_dir.join(&units.service_unit_name).exists());

    let calls = runner.calls();
    assert!(
        calls
            .iter()
            .any(|(p, args)| p == "systemctl" && args == &["restart", "nginx.service"])
    );
    assert!(calls.iter().any(|(p, _)| p == "update-ca-trust"));
}

#[test]
fn linux_client_status_and_uninstall_inspect_systemd_units() {
    let temp = TempDir::new().unwrap();
    let helper_dir = temp.path().join("libexec");
    let systemd_dir = temp.path().join("systemd");
    std::fs::create_dir_all(&helper_dir).unwrap();
    std::fs::create_dir_all(&systemd_dir).unwrap();

    let staged = helper_dir.join("devbase-helper");
    std::fs::write(&staged, b"bin").unwrap();
    let socket_path = temp.path().join("devbase-helper.sock");
    let service_unit_path = systemd_dir.join("devbase-helper.service");
    let socket_unit_path = systemd_dir.join("devbase-helper.socket");

    std::fs::write(
        &service_unit_path,
        build_helper_service_unit(&staged, 1000).unwrap(),
    )
    .unwrap();
    std::fs::write(
        &socket_unit_path,
        build_helper_socket_unit(&socket_path.to_string_lossy()),
    )
    .unwrap();

    let admin = FakeAdmin::succeeding();
    let client = Client::new(ClientConfig {
        socket_path,
        plist_path: service_unit_path,
        helper_dir,
        expected: BinaryInfo {
            executable: std::path::PathBuf::from("/usr/bin/devbase"),
            bundle_path: None,
            owner_uid: 1000,
        },
        admin: admin.clone(),
        ..ClientConfig::default()
    });

    assert_eq!(client.status().state, HelperState::Installed);
    client.uninstall().unwrap();
    let calls = admin.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0]
            .1
            .join(" ")
            .contains("systemctl disable --now devbase-helper.socket")
    );
}
