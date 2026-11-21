use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::activate::{SocketActivator, parse_owner_uid, serve_activated_with};
use super::client::{BinaryInfo, Client, ClientConfig};
use super::server::{CommandOutput, Server, ServerConfig};
use super::test_support::{FakeRunner, euid, server_config};

fn short_socket_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("devbase-ph-")
        .tempdir_in("/tmp")
        .expect("short socket directory")
}

fn client_for(socket: &std::path::Path) -> Client {
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

type Serving = (
    Arc<AtomicBool>,
    std::thread::JoinHandle<std::io::Result<()>>,
);

fn start(config: ServerConfig, listener: UnixListener) -> Serving {
    let server = Arc::new(Server::new(config));
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let handle = std::thread::spawn(move || server.serve_until(listener, thread_stop));
    (stop, handle)
}

#[test]
fn idle_shutdown_leaves_the_socket_file_in_place() {
    let dir = short_socket_dir();
    let socket = dir.path().join("idle.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let config = ServerConfig {
        idle_timeout: Duration::from_millis(250),
        ..server_config()
    };
    let (_stop, handle) = start(config, listener);
    let client = client_for(&socket);
    let response = client.ping().expect("ping before idle exit");
    assert_eq!(response.protocol_version, super::PROTOCOL_VERSION);
    assert_eq!(response.owner_uid, i64::from(euid()));

    let started = Instant::now();
    let result = handle.join().expect("server thread");
    assert!(result.is_ok(), "idle shutdown must be a clean exit");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        socket.exists(),
        "the socket file must survive idle shutdown"
    );
}

#[test]
fn every_request_resets_the_idle_timer() {
    let dir = short_socket_dir();
    let socket = dir.path().join("reset.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let config = ServerConfig {
        idle_timeout: Duration::from_millis(400),
        ..server_config()
    };
    let (_stop, handle) = start(config, listener);
    let client = client_for(&socket);

    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(150));
        client.ping().expect("requests keep the helper alive");
    }
    let result = handle.join().expect("server thread");
    assert!(result.is_ok());
    let _ = client.ping().expect_err("the helper has exited");
}

#[test]
fn an_in_flight_request_delays_the_idle_exit() {
    let dir = short_socket_dir();
    let socket = dir.path().join("busy.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let running = Arc::new(AtomicBool::new(false));
    let running_in_runner = Arc::clone(&running);
    let runner = FakeRunner::new(move |_, _, _| {
        running_in_runner.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        CommandOutput::success(b"p1\x00".to_vec())
    });
    let config = ServerConfig {
        idle_timeout: Duration::from_millis(150),
        runner,
        ..server_config()
    };
    let server_started = Instant::now();
    let (_stop, handle) = start(config, listener);
    let client = client_for(&socket);

    let request = std::thread::spawn(move || client.inspect_port(443));
    let wait_started = Instant::now();
    while !running.load(Ordering::SeqCst) {
        assert!(
            wait_started.elapsed() < Duration::from_secs(2),
            "the request never reached the runner"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // The idle timer would have expired here if it ignored in-flight work.
    std::thread::sleep(Duration::from_millis(180));
    let response = request
        .join()
        .expect("request thread")
        .expect("slow request completes");
    assert_eq!(response.as_slice(), b"p1\x00");
    let result = handle.join().expect("server thread");
    assert!(result.is_ok());
    // The server cannot have exited before the 300 ms request completed; if
    // the idle loop ignored in-flight work the request itself would have
    // failed above.
    assert!(
        server_started.elapsed() >= Duration::from_millis(300),
        "the helper must not exit while a request is in flight"
    );
}

#[derive(Debug)]
struct FixedActivator {
    name: Mutex<Option<String>>,
    listener: Mutex<Option<UnixListener>>,
}

impl FixedActivator {
    fn new(listener: UnixListener) -> Arc<Self> {
        Arc::new(FixedActivator {
            name: Mutex::new(None),
            listener: Mutex::new(Some(listener)),
        })
    }

    fn name(&self) -> Option<String> {
        self.name.lock().expect("name").clone()
    }
}

impl SocketActivator for FixedActivator {
    fn take(&self, name: &str) -> std::io::Result<UnixListener> {
        *self.name.lock().expect("name") = Some(name.to_string());
        self.listener
            .lock()
            .expect("listener")
            .take()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "already taken"))
    }
}

#[test]
fn serve_activated_rejects_an_invalid_owner_uid_before_claiming_the_socket() {
    let dir = short_socket_dir();
    let socket = dir.path().join("owner.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let activator = FixedActivator::new(listener);
    let error = serve_activated_with(0, &*activator, server_config()).expect_err("zero uid");
    assert_eq!(error.to_string(), "invalid owner uid 0");
    let error = serve_activated_with(-3, &*activator, server_config()).expect_err("negative uid");
    assert_eq!(error.to_string(), "invalid owner uid -3");
    assert_eq!(activator.name(), None, "the socket must not be claimed");
}

#[test]
fn serve_activated_claims_the_helper_socket_and_serves_until_idle() {
    let dir = short_socket_dir();
    let socket = dir.path().join("activated.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let activator = FixedActivator::new(listener);
    let config = ServerConfig {
        idle_timeout: Duration::from_millis(60),
        ..server_config()
    };
    let started = Instant::now();
    serve_activated_with(i64::from(euid()), &*activator, config)
        .expect("activated serving exits cleanly on idle");
    assert_eq!(activator.name().as_deref(), Some("helper"));
    assert!(started.elapsed() >= Duration::from_millis(40));
    assert!(socket.exists());
}

#[test]
fn owner_uid_parsing_matches_the_go_helper_flag() {
    assert_eq!(parse_owner_uid(&[]).expect("default"), -1);
    assert_eq!(
        parse_owner_uid(&[
            "--dev".to_string(),
            "--owner-uid".to_string(),
            "501".to_string()
        ])
        .expect("parsed"),
        501
    );
    assert_eq!(
        parse_owner_uid(&["--owner-uid".to_string()]).expect("dangling"),
        -1
    );
    let error = parse_owner_uid(&["--owner-uid".to_string(), "nope".to_string()])
        .expect_err("invalid number");
    assert!(error.to_string().contains("invalid --owner-uid"));
}

#[test]
fn a_stop_request_ends_serving_without_waiting_for_idle() {
    let dir = short_socket_dir();
    let socket = dir.path().join("stop.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let (stop, handle) = start(server_config(), listener);
    std::thread::sleep(Duration::from_millis(30));
    stop.store(true, Ordering::SeqCst);
    let started = Instant::now();
    handle.join().expect("joined").expect("clean");
    assert!(started.elapsed() < Duration::from_secs(1));
}
