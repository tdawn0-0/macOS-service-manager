use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::admin::AdminError;
use super::client::{
    BinaryInfo, Client, ClientConfig, install_helper_script, uninstall_helper_script,
};
use super::plist_doc::{app_bundle_for_executable, build_helper_plist, expected_staged_executable};
use super::server::Server;
use super::test_support::{FakeAdmin, euid, server_config};
use super::{HELPER_DIR, LABEL, PLIST_PATH, SOCKET_PATH, parse_helper_plist};

const EXE: &str = "/Applications/Devbase.app/Contents/MacOS/Devbase";
const TMP_PLIST: &str = "/tmp/devbase-helper-install-x/xyz.jyeu.devbase.helper.plist";
const STAGED: &str = "/Library/PrivilegedHelperTools/xyz.jyeu.devbase.helper/devbase-helper";

fn binary_info() -> BinaryInfo {
    BinaryInfo {
        executable: PathBuf::from(EXE),
        bundle_path: None,
        owner_uid: euid(),
    }
}

#[test]
fn helper_plist_has_the_exact_launchd_document() {
    let plist = build_helper_plist(Path::new(STAGED), 501, SOCKET_PATH).expect("build");
    let text = String::from_utf8(plist.clone()).expect("XML is UTF-8");
    let label = text.find("<key>Label</key>").expect("Label");
    let arguments = text.find("<key>ProgramArguments</key>").expect("arguments");
    let sockets = text.find("<key>Sockets</key>").expect("sockets");
    let stderr = text.find("<key>StandardErrorPath</key>").expect("stderr");
    assert!(label < arguments && arguments < sockets && sockets < stderr);
    let dev = text.find("<string>--dev</string>").expect("--dev");
    let helper = text.find("<string>helper</string>").expect("helper");
    let owner = text
        .find("<string>--owner-uid</string>")
        .expect("--owner-uid");
    let uid = text.find("<string>501</string>").expect("uid");
    assert!(dev < helper && helper < owner && owner < uid);
    let mode_key = text.find("<key>SockPathMode</key>").expect("mode key");
    let mode_value = text.find("<integer>438</integer>").expect("mode value");
    let path_key = text.find("<key>SockPathName</key>").expect("name key");
    assert!(mode_key < mode_value && mode_value < path_key);
    assert!(text.contains(&format!("<string>{SOCKET_PATH}</string>")));
    assert!(text.contains("<string>/var/log/xyz.jyeu.devbase.helper.log</string>"));
    assert!(!text.contains("<key>RunAtLoad</key>"));
    assert!(!text.contains("<key>KeepAlive</key>"));
    assert!(!text.contains("<key>UserName</key>"));

    let doc = parse_helper_plist(&plist).expect("parse");
    assert_eq!(doc.label, LABEL);
    assert_eq!(
        doc.program_arguments,
        vec![STAGED, "--dev", "helper", "--owner-uid", "501"]
    );
    assert_eq!(
        doc.sockets.get("helper").expect("socket").sock_path_name,
        SOCKET_PATH
    );
    assert_eq!(
        doc.sockets.get("helper").expect("socket").sock_path_mode,
        0o666
    );
}

#[test]
fn install_script_is_byte_exact_with_no_ditto_for_a_plain_binary() {
    let script = install_helper_script(
        &binary_info(),
        Path::new(HELPER_DIR),
        Path::new(PLIST_PATH),
        Path::new(SOCKET_PATH),
        Path::new(TMP_PLIST),
        Path::new(STAGED),
    );
    let expected = format!(
        "set -eu; /bin/launchctl bootout 'system/{LABEL}' >/dev/null 2>&1 || true; /bin/rm -f '{SOCKET_PATH}'; /bin/mkdir -p '/Library/PrivilegedHelperTools'; /usr/sbin/chown root:wheel '/Library/PrivilegedHelperTools'; /bin/chmod 0755 '/Library/PrivilegedHelperTools'; /bin/rm -rf '{HELPER_DIR}'; /bin/mkdir -p '{HELPER_DIR}'; /usr/bin/install -m 0755 '{EXE}' '{STAGED}'; /usr/sbin/chown -R root:wheel '{HELPER_DIR}'; /bin/chmod 0755 '{HELPER_DIR}'; /usr/bin/install -m 0644 '{TMP_PLIST}' '{PLIST_PATH}'; /usr/sbin/chown root:wheel '{PLIST_PATH}'; /bin/launchctl bootstrap system '{PLIST_PATH}'; i=0; while [ ! -S '{SOCKET_PATH}' ] && [ \"$i\" -lt 50 ]; do /bin/sleep 0.1; i=$((i + 1)); done"
    );
    assert_eq!(script, expected);
    assert!(!script.contains("ditto"));
    assert!(!script.contains("codesign"));
}

#[test]
fn bundle_install_stages_with_ditto_and_verifies_the_signature_first() {
    let info = BinaryInfo {
        executable: PathBuf::from(EXE),
        bundle_path: Some(PathBuf::from("/Applications/Devbase.app")),
        owner_uid: euid(),
    };
    let staged = expected_staged_executable(Path::new(HELPER_DIR), &info);
    assert_eq!(
        staged,
        Path::new(HELPER_DIR).join("bundle/Contents/MacOS/Devbase")
    );
    assert!(!staged.to_string_lossy().contains(".app"));
    let script = install_helper_script(
        &info,
        Path::new(HELPER_DIR),
        Path::new(PLIST_PATH),
        Path::new(SOCKET_PATH),
        Path::new(TMP_PLIST),
        &staged,
    );
    let ditto = format!("/usr/bin/ditto --noqtn '/Applications/Devbase.app' '{HELPER_DIR}/bundle'");
    let codesign = format!("/usr/bin/codesign --verify --deep --strict '{HELPER_DIR}/bundle'");
    let ditto_at = script.find(&ditto).expect("ditto step");
    let codesign_at = script.find(&codesign).expect("codesign step");
    let plist_at = script
        .find(&format!("/usr/bin/install -m 0644 '{TMP_PLIST}'"))
        .expect("plist write");
    assert!(ditto_at < codesign_at && codesign_at < plist_at);
    assert!(!script.contains("/usr/bin/install -m 0755"));
}

#[test]
fn uninstall_script_is_byte_exact_and_has_no_health_check() {
    let script = uninstall_helper_script(
        Path::new(PLIST_PATH),
        Path::new(HELPER_DIR),
        Path::new(SOCKET_PATH),
    );
    assert_eq!(
        script,
        format!(
            "set -eu; /bin/launchctl bootout 'system/{LABEL}' >/dev/null 2>&1 || true; /bin/rm -f '{PLIST_PATH}'; /bin/rm -rf '{HELPER_DIR}'; /bin/rm -f '{SOCKET_PATH}'"
        )
    );
    assert!(!script.contains("status"));
}

#[test]
fn app_bundle_detection_requires_contents_info_plist() {
    let dir = tempfile::tempdir().expect("temp dir");
    let bundle = dir.path().join("Devbase.app");
    let macos = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).expect("bundle dirs");
    let exe = macos.join("Devbase");
    std::fs::write(&exe, b"payload").expect("write exe");
    assert_eq!(app_bundle_for_executable(&exe), None);
    std::fs::write(bundle.join("Contents/Info.plist"), b"<plist/>").expect("info plist");
    assert_eq!(app_bundle_for_executable(&exe), Some(bundle.clone()));
    let info = BinaryInfo {
        executable: exe.clone(),
        bundle_path: Some(bundle),
        owner_uid: euid(),
    };
    let staged = expected_staged_executable(Path::new(HELPER_DIR), &info);
    assert_eq!(
        staged,
        Path::new(HELPER_DIR).join("bundle/Contents/MacOS/Devbase")
    );
    assert!(!staged.to_string_lossy().contains(".app"));
}

#[test]
fn install_uses_one_admin_prompt_and_waits_for_the_socket() {
    let dir = tempfile::tempdir().expect("temp dir");
    let socket_dir = tempfile::Builder::new()
        .prefix("devbase-ph-")
        .tempdir_in("/tmp")
        .expect("short socket dir");
    let socket_path = socket_dir.path().join("delayed.sock");
    let plist_path = dir.path().join(format!("{LABEL}.plist"));
    let helper_dir = dir.path().join("helper");
    let fake_exe = dir.path().join("Devbase");
    std::fs::write(&fake_exe, b"#!/bin/sh\n").expect("fake executable");

    let stop = Arc::new(AtomicBool::new(false));
    let threads: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let mut config = server_config();
    config.owner_uid = i64::from(euid());
    let delayed_socket = socket_path.clone();
    let delayed_stop = Arc::clone(&stop);
    let delayed_threads = Arc::clone(&threads);
    let admin = FakeAdmin::with(move |program, args| {
        assert_eq!(program, "/bin/sh");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        let socket = delayed_socket.clone();
        let cfg = config.clone();
        let stop = Arc::clone(&delayed_stop);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind delayed");
            let server = Arc::new(Server::new(cfg));
            let _ = server.serve_until(listener, stop);
        });
        delayed_threads.lock().expect("threads").push(handle);
        (String::new(), None)
    });
    let client = Client::new(ClientConfig {
        socket_path: socket_path.clone(),
        plist_path: plist_path.clone(),
        helper_dir: helper_dir.clone(),
        expected: BinaryInfo {
            executable: fake_exe,
            bundle_path: None,
            owner_uid: euid(),
        },
        admin: admin.clone(),
        health_timeout: Duration::from_secs(2),
        health_poll_interval: Duration::from_millis(20),
    });

    client
        .install()
        .expect("install should succeed after the delayed socket appears");
    let calls = admin.calls();
    assert_eq!(calls.len(), 1, "install must prompt exactly once");
    assert_eq!(calls[0].0, "/bin/sh");
    assert_eq!(calls[0].1[0], "-c");
    let script = &calls[0].1[1];
    assert!(script.contains("bootstrap system"));
    assert!(script.contains(socket_path.to_str().expect("socket path")));
    assert!(script.contains("while [ ! -S"));

    stop.store(true, Ordering::SeqCst);
    for handle in threads.lock().expect("threads").drain(..) {
        let _ = handle.join();
    }
}

#[test]
fn install_reports_a_health_check_timeout() {
    let dir = tempfile::tempdir().expect("temp dir");
    let socket_dir = tempfile::Builder::new()
        .prefix("devbase-ph-")
        .tempdir_in("/tmp")
        .expect("short socket dir");
    let fake_exe = dir.path().join("Devbase");
    std::fs::write(&fake_exe, b"#!/bin/sh\n").expect("fake executable");
    let admin = FakeAdmin::succeeding();
    let client = Client::new(ClientConfig {
        socket_path: socket_dir.path().join("missing.sock"),
        plist_path: dir.path().join(format!("{LABEL}.plist")),
        helper_dir: dir.path().join("helper"),
        expected: BinaryInfo {
            executable: fake_exe,
            bundle_path: None,
            owner_uid: euid(),
        },
        admin: admin.clone(),
        health_timeout: Duration::from_millis(120),
        health_poll_interval: Duration::from_millis(15),
    });

    let error = client.install().expect_err("no socket means no success");
    assert!(
        error
            .to_string()
            .contains("helper installed, but health check failed"),
        "unexpected error: {error}"
    );
    assert_eq!(admin.calls().len(), 1);
}

#[test]
fn install_surfaces_admin_output_and_cancel() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fake_exe = dir.path().join("Devbase");
    std::fs::write(&fake_exe, b"#!/bin/sh\n").expect("fake executable");
    let failing = FakeAdmin::with(|_, _| {
        (
            "  install: permission denied  ".to_string(),
            Some(AdminError::Message("exit status 1".to_string())),
        )
    });
    let client = Client::new(ClientConfig {
        socket_path: dir.path().join("h.sock"),
        plist_path: dir.path().join(format!("{LABEL}.plist")),
        helper_dir: dir.path().join("helper"),
        expected: BinaryInfo {
            executable: fake_exe,
            bundle_path: None,
            owner_uid: euid(),
        },
        admin: failing,
        ..ClientConfig::default()
    });
    let error = client.install().expect_err("admin failure");
    assert_eq!(
        error.to_string(),
        "helper install failed: install: permission denied: exit status 1"
    );

    let cancel = FakeAdmin::failing(AdminError::Canceled);
    let client = Client::new(ClientConfig {
        socket_path: dir.path().join("h.sock"),
        plist_path: dir.path().join(format!("{LABEL}.plist")),
        helper_dir: dir.path().join("helper"),
        expected: BinaryInfo {
            executable: dir.path().join("Devbase"),
            bundle_path: None,
            owner_uid: euid(),
        },
        admin: cancel,
        ..ClientConfig::default()
    });
    let error = client.install().expect_err("cancel");
    assert_eq!(error.to_string(), "authorization canceled");
}

#[test]
fn uninstall_runs_one_admin_script_and_no_health_check() {
    let dir = tempfile::tempdir().expect("temp dir");
    let admin = FakeAdmin::succeeding();
    let client = Client::new(ClientConfig {
        socket_path: dir.path().join("h.sock"),
        plist_path: dir.path().join(format!("{LABEL}.plist")),
        helper_dir: dir.path().join("helper"),
        admin: admin.clone(),
        ..ClientConfig::default()
    });
    client.uninstall().expect("uninstall");
    let calls = admin.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "/bin/sh");
    assert_eq!(calls[0].1[0], "-c");
    let script = &calls[0].1[1];
    assert!(script.starts_with("set -eu; /bin/launchctl bootout"));
    assert!(script.contains("/bin/rm -rf"));
    assert!(!script.contains("bootstrap"));
}

#[test]
fn disabled_client_never_prompts() {
    let admin = FakeAdmin::succeeding();
    let mut config = ClientConfig {
        admin: admin.clone(),
        ..ClientConfig::default()
    };
    config.socket_path = PathBuf::from("/tmp/unused.sock");
    let client = Client::disabled_with(config);
    assert_eq!(
        client.install().expect_err("disabled").to_string(),
        "privileged helper is disabled"
    );
    assert_eq!(
        client.uninstall().expect_err("disabled").to_string(),
        "privileged helper is disabled"
    );
    assert!(admin.calls().is_empty());
}
