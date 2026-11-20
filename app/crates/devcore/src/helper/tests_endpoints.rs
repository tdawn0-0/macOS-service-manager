use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use super::http::{Request, Response};
use super::protocol::{
    LaunchdControlRequest, LaunchdRemoveRequest, LaunchdSaveRequest, PortInspectRequest,
    ProxySocketInstallRequest, ProxySocketKickstartRequest, ProxySocketStopRequest,
    encode_request_json,
};
use super::server::{CommandOutput, Server, ServerConfig};
use super::test_support::{FakeRunner, RecordingChown, euid, server_config};
use super::{HELPER_DIR, LABEL};

fn call(server: &Server, request: &Request) -> Response {
    let (stream, _peer) = std::os::unix::net::UnixStream::pair().expect("pair");
    server.handle(request, Some(&stream))
}

fn post<T: Serialize>(path: &str, value: &T) -> Request {
    Request::with_body(
        "POST",
        path,
        encode_request_json(value).expect("encode request"),
    )
}

fn error_of(response: &Response) -> String {
    serde_json::from_slice::<super::protocol::ErrorResponse>(&response.body)
        .map(|error| error.error)
        .unwrap_or_else(|_| String::from_utf8_lossy(&response.body).into_owned())
}

fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn plist(label: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>Label</key><string>{label}</string></dict></plist>"
    )
    .into_bytes()
}

fn paths(dir: &Path) -> (String, String) {
    let job = dir.join("xyz.jyeu.devbase.test.plist");
    let target = "system/xyz.jyeu.devbase.test".to_string();
    (job.to_str().expect("utf8").to_string(), target)
}

fn direct_server(runner: Option<Arc<FakeRunner>>) -> (Server, Arc<RecordingChown>) {
    let chown = Arc::new(RecordingChown::default());
    let mut config = server_config();
    if let Some(runner) = runner {
        config.runner = runner;
    }
    config.chown = chown.clone();
    (Server::new(config), chown)
}

#[test]
fn routing_matrix_matches_go_serve_mux() {
    let (server, _) = direct_server(None);
    let status = call(&server, &Request::new("GET", "/v1/status"));
    assert_eq!(status.status, 200);

    let wrong_method = call(&server, &Request::new("POST", "/v1/status"));
    assert_eq!(wrong_method.status, 405);
    assert_eq!(header(&wrong_method, "Allow"), Some("GET"));
    assert_eq!(wrong_method.body.as_slice(), b"Method Not Allowed\n");

    let wrong_method = call(&server, &Request::new("GET", "/v1/ports/inspect"));
    assert_eq!(wrong_method.status, 405);
    assert_eq!(header(&wrong_method, "Allow"), Some("POST"));

    let unknown = call(&server, &Request::new("GET", "/v1/nope"));
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.body, b"404 page not found\n");
    assert_eq!(
        header(&unknown, "Content-Type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(header(&unknown, "X-Content-Type-Options"), Some("nosniff"));

    let head = call(&server, &Request::new("HEAD", "/v1/status"));
    assert_eq!(head.status, 200);
}

#[test]
fn proxy_socket_install_validation_matrix() {
    let dir = tempfile::tempdir().expect("temp dir");
    let allowed = dir.path().to_str().expect("utf8").to_string();
    let mut config = server_config();
    config.socket_job_dirs = vec![allowed.clone()];
    config.chown = Arc::new(RecordingChown::default());
    let server = Server::new(config);
    let (job, target) = paths(dir.path());
    let good_plist = plist("xyz.jyeu.devbase.test");

    let cases: Vec<(ProxySocketInstallRequest, &str)> = vec![
        (
            ProxySocketInstallRequest {
                plist: good_plist.clone(),
                job_path: "relative.plist".to_string(),
                target: target.clone(),
            },
            "must be absolute",
        ),
        (
            ProxySocketInstallRequest {
                plist: good_plist.clone(),
                job_path: format!("{allowed}/./xyz.jyeu.devbase.test.plist"),
                target: target.clone(),
            },
            "not normalized",
        ),
        (
            ProxySocketInstallRequest {
                plist: good_plist.clone(),
                job_path: job.replace(".plist", ".txt"),
                target: target.clone(),
            },
            "must end with .plist",
        ),
        (
            ProxySocketInstallRequest {
                plist: good_plist.clone(),
                job_path: "/etc/xyz.jyeu.devbase.test.plist".to_string(),
                target: target.clone(),
            },
            "not permitted",
        ),
        (
            ProxySocketInstallRequest {
                plist: good_plist.clone(),
                job_path: dir
                    .path()
                    .join("com.example.job.plist")
                    .to_str()
                    .expect("utf8")
                    .to_string(),
                target: target.clone(),
            },
            "unexpected socket job path",
        ),
        (
            ProxySocketInstallRequest {
                plist: good_plist.clone(),
                job_path: job.clone(),
                target: "gui/501/xyz.jyeu.devbase.test".to_string(),
            },
            "unexpected proxy socket target",
        ),
        (
            ProxySocketInstallRequest {
                plist: good_plist.clone(),
                job_path: job.clone(),
                target: format!("system/{LABEL}"),
            },
            "cannot target privileged helper",
        ),
        (
            ProxySocketInstallRequest {
                plist: Vec::new(),
                job_path: job.clone(),
                target: target.clone(),
            },
            "plist data is empty",
        ),
        (
            ProxySocketInstallRequest {
                plist: b"not xml".to_vec(),
                job_path: job.clone(),
                target: target.clone(),
            },
            "invalid plist",
        ),
        (
            ProxySocketInstallRequest {
                plist: br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Foo</key><string>x</string></dict></plist>"#.to_vec(),
                job_path: job.clone(),
                target: target.clone(),
            },
            "plist is missing Label",
        ),
        (
            ProxySocketInstallRequest {
                plist: plist(LABEL),
                job_path: job.clone(),
                target: target.clone(),
            },
            "cannot write helper label",
        ),
    ];
    for (request, fragment) in cases {
        let response = call(&server, &post("/v1/proxy-socket/install", &request));
        assert_eq!(response.status, 400, "{request:?}");
        assert!(
            error_of(&response).contains(fragment),
            "{request:?} -> {}",
            error_of(&response)
        );
    }

    // A symlink at the job path is refused.
    let link = dir.path().join("xyz.jyeu.devbase.link.plist");
    std::os::unix::fs::symlink(dir.path().join("elsewhere"), &link).expect("symlink");
    let response = call(
        &server,
        &post(
            "/v1/proxy-socket/install",
            &ProxySocketInstallRequest {
                plist: good_plist,
                job_path: link.to_str().expect("utf8").to_string(),
                target,
            },
        ),
    );
    assert_eq!(response.status, 400);
    assert!(error_of(&response).contains("refusing symlink"));
}

#[test]
fn proxy_socket_install_writes_the_plist_and_bootstraps_in_order() {
    let dir = tempfile::tempdir().expect("temp dir");
    let allowed = dir.path().to_str().expect("utf8").to_string();
    let (job, target) = paths(dir.path());
    let runner = FakeRunner::succeeding();
    let mut config = server_config();
    config.socket_job_dirs = vec![allowed];
    config.runner = runner.clone();
    let server = Server::new(config);
    let job_plist = plist("xyz.jyeu.devbase.test");

    let response = call(
        &server,
        &post(
            "/v1/proxy-socket/install",
            &ProxySocketInstallRequest {
                plist: job_plist.clone(),
                job_path: job.clone(),
                target: target.clone(),
            },
        ),
    );
    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_slice(), br#"{"ok":true}"#);
    assert_eq!(std::fs::read(&job).expect("saved plist"), job_plist);
    assert_eq!(
        runner.argv(),
        vec![
            vec![
                "/bin/launchctl".to_string(),
                "bootout".to_string(),
                target.clone()
            ],
            vec![
                "/bin/launchctl".to_string(),
                "bootstrap".to_string(),
                "system".to_string(),
                job.clone()
            ],
        ]
    );
}

#[test]
fn proxy_socket_install_reports_bootstrap_failure() {
    let dir = tempfile::tempdir().expect("temp dir");
    let allowed = dir.path().to_str().expect("utf8").to_string();
    let (job, target) = paths(dir.path());
    let runner = FakeRunner::new(|program, args, _| {
        if args.first() == Some(&"bootstrap") {
            CommandOutput::failure(
                b"  Bootstrap failed: 5\n".to_vec(),
                format!("{program}: exit status 5"),
            )
        } else {
            CommandOutput::success(Vec::new())
        }
    });
    let mut config = server_config();
    config.socket_job_dirs = vec![allowed];
    config.runner = runner;
    let server = Server::new(config);

    let response = call(
        &server,
        &post(
            "/v1/proxy-socket/install",
            &ProxySocketInstallRequest {
                plist: plist("xyz.jyeu.devbase.test"),
                job_path: job,
                target,
            },
        ),
    );
    assert_eq!(response.status, 500);
    assert_eq!(error_of(&response), "Bootstrap failed: 5");
}

#[test]
fn proxy_socket_stop_boots_out_only_when_loaded_and_removes_the_plist() {
    let dir = tempfile::tempdir().expect("temp dir");
    let allowed = dir.path().to_str().expect("utf8").to_string();
    let (job, target) = paths(dir.path());
    std::fs::write(&job, plist("xyz.jyeu.devbase.test")).expect("job plist");

    // `print` fails: the file is removed without a bootout.
    let runner = FakeRunner::new(|_, args, _| {
        if args.first() == Some(&"print") {
            CommandOutput::failure(Vec::new(), "not loaded".to_string())
        } else {
            CommandOutput::success(Vec::new())
        }
    });
    let mut config = server_config();
    config.socket_job_dirs = vec![allowed.clone()];
    config.runner = runner.clone();
    let server = Server::new(config);
    let response = call(
        &server,
        &post(
            "/v1/proxy-socket/stop",
            &ProxySocketStopRequest {
                job_path: job.clone(),
                target: target.clone(),
            },
        ),
    );
    assert_eq!(response.status, 200);
    assert_eq!(
        runner.argv(),
        vec![vec![
            "/bin/launchctl".to_string(),
            "print".to_string(),
            target.clone()
        ]]
    );
    assert!(!Path::new(&job).exists());

    // `print` succeeds: bootout first, then remove an absent file (ENOENT ok).
    let runner = FakeRunner::succeeding();
    let mut config = server_config();
    config.socket_job_dirs = vec![allowed];
    config.runner = runner.clone();
    let server = Server::new(config);
    let response = call(
        &server,
        &post(
            "/v1/proxy-socket/stop",
            &ProxySocketStopRequest {
                job_path: job.clone(),
                target: target.clone(),
            },
        ),
    );
    assert_eq!(response.status, 200);
    assert_eq!(
        runner.argv(),
        vec![
            vec![
                "/bin/launchctl".to_string(),
                "print".to_string(),
                target.clone()
            ],
            vec![
                "/bin/launchctl".to_string(),
                "bootout".to_string(),
                target.clone()
            ],
        ]
    );
}

#[test]
fn proxy_socket_kickstart_uses_k_and_validates_the_target() {
    let dir = tempfile::tempdir().expect("temp dir");
    let allowed = dir.path().to_str().expect("utf8").to_string();
    let runner = FakeRunner::succeeding();
    let mut config = server_config();
    config.socket_job_dirs = vec![allowed];
    config.runner = runner.clone();
    let server = Server::new(config);
    let target = "system/xyz.jyeu.devbase.proxy-sockets".to_string();
    let response = call(
        &server,
        &post(
            "/v1/proxy-socket/kickstart",
            &ProxySocketKickstartRequest {
                target: target.clone(),
            },
        ),
    );
    assert_eq!(response.status, 200);
    assert_eq!(
        runner.argv(),
        vec![vec![
            "/bin/launchctl".to_string(),
            "kickstart".to_string(),
            "-k".to_string(),
            target
        ]]
    );

    for bad in [
        "system/xyz.jyeu.devbase.helper".to_string(),
        "gui/501/xyz.jyeu.devbase.proxy-sockets".to_string(),
        "system/other".to_string(),
        "nonsense".to_string(),
    ] {
        let response = call(
            &server,
            &post(
                "/v1/proxy-socket/kickstart",
                &ProxySocketKickstartRequest {
                    target: bad.clone(),
                },
            ),
        );
        assert_eq!(response.status, 400, "{bad}");
    }
}

#[test]
fn port_inspect_validates_and_preserves_raw_lsof_output() {
    let raw = b"p4242\x00cnode\x00u501\x00Ldev\x00n127.0.0.1:443\x00";
    let runner = FakeRunner::new(move |program, args, _| {
        assert_eq!(program, "/usr/sbin/lsof");
        assert_eq!(
            args,
            &["-nP", "-a", "-iTCP:443", "-sTCP:LISTEN", "+c0", "-F0pcun"]
        );
        CommandOutput::success(raw.to_vec())
    });
    let mut config = server_config();
    config.runner = runner.clone();
    let server = Server::new(config);

    for bad in [0, -1, 65536, 70000] {
        let response = call(
            &server,
            &post("/v1/ports/inspect", &PortInspectRequest { port: bad }),
        );
        assert_eq!(response.status, 400);
        assert_eq!(error_of(&response), format!("invalid port {bad}"));
    }
    assert!(runner.argv().is_empty());

    let response = call(
        &server,
        &post("/v1/ports/inspect", &PortInspectRequest { port: 443 }),
    );
    assert_eq!(response.status, 200);
    let decoded: super::protocol::PortInspectResponse =
        serde_json::from_slice(&response.body).expect("response");
    assert_eq!(decoded.output, raw);

    // A failure with output is still a successful inspection (lsof exits 1
    // when nothing listens).
    let runner =
        FakeRunner::new(|_, _, _| CommandOutput::failure(b"p1\x00".to_vec(), "exit 1".to_string()));
    let mut config = server_config();
    config.runner = runner;
    let server = Server::new(config);
    let response = call(
        &server,
        &post("/v1/ports/inspect", &PortInspectRequest { port: 65535 }),
    );
    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_slice(), br#"{"output":"cDEA"}"#);

    let runner = FakeRunner::new(|_, _, _| {
        CommandOutput::failure(Vec::new(), "lsof: not found".to_string())
    });
    let mut config = server_config();
    config.runner = runner;
    let server = Server::new(config);
    let response = call(
        &server,
        &post("/v1/ports/inspect", &PortInspectRequest { port: 1 }),
    );
    assert_eq!(response.status, 500);
    assert_eq!(error_of(&response), "lsof: not found");
}

#[test]
fn launchd_control_validation_matrix() {
    let runner = FakeRunner::succeeding();
    let mut config = server_config();
    config.runner = runner.clone();
    let server = Server::new(config);

    let response = call(
        &server,
        &post(
            "/v1/launchd/control",
            &LaunchdControlRequest {
                commands: Vec::new(),
            },
        ),
    );
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response), "no commands specified");

    let invalid: Vec<(Vec<String>, &str)> = vec![
        (vec!["print".to_string()], "requires arguments"),
        (
            vec!["print".to_string(), String::new()],
            "invalid launchctl argument",
        ),
        (
            vec!["print".to_string(), format!("system/{LABEL}")],
            "cannot target privileged helper",
        ),
        (
            vec![
                "bootstrap".to_string(),
                "system".to_string(),
                format!("/Library/LaunchDaemons/{LABEL}.plist"),
            ],
            "cannot target privileged helper",
        ),
        (
            vec!["rm".to_string(), "-rf".to_string(), "/".to_string()],
            "unsupported launchctl subcommand",
        ),
        (
            vec!["frobnicate".to_string(), "system/x".to_string()],
            "unsupported launchctl subcommand",
        ),
        (
            vec!["bootstrap".to_string(), "system".to_string()],
            "bootstrap expects domain and plist path",
        ),
        (
            vec![
                "bootstrap".to_string(),
                "bogus".to_string(),
                "/Library/LaunchDaemons/x.plist".to_string(),
            ],
            "invalid launchd domain",
        ),
        (
            vec![
                "bootstrap".to_string(),
                "system".to_string(),
                "relative.plist".to_string(),
            ],
            "invalid plist path",
        ),
        (
            vec![
                "bootstrap".to_string(),
                "system".to_string(),
                "/Library/LaunchDaemons/../LaunchDaemons/x.plist".to_string(),
            ],
            "invalid plist path",
        ),
        (
            vec!["bootout".to_string(), "nonsense".to_string()],
            "invalid launchd target",
        ),
        (
            vec![
                "kickstart".to_string(),
                "-x".to_string(),
                "system/x".to_string(),
            ],
            "unexpected kickstart arguments",
        ),
        (
            vec![
                "kill".to_string(),
                "SIGUSR1".to_string(),
                "system/x".to_string(),
            ],
            "unsupported signal",
        ),
    ];
    for (command, fragment) in invalid {
        let response = call(
            &server,
            &post(
                "/v1/launchd/control",
                &LaunchdControlRequest {
                    commands: vec![command.clone()],
                },
            ),
        );
        assert_eq!(response.status, 400, "{command:?}");
        assert!(
            error_of(&response).contains(fragment),
            "{command:?} -> {}",
            error_of(&response)
        );
    }
    assert!(runner.argv().is_empty(), "nothing invalid may run");

    // The full allowed argv set runs in order.
    let commands: Vec<Vec<String>> = vec![
        vec![
            "bootstrap".to_string(),
            "system".to_string(),
            "/Library/LaunchDaemons/com.example.plist".to_string(),
        ],
        vec!["bootout".to_string(), "system/com.example".to_string()],
        vec!["print".to_string(), "system/com.example".to_string()],
        vec!["enable".to_string(), "gui/501/com.example".to_string()],
        vec!["disable".to_string(), "user/501/com.example".to_string()],
        vec!["kickstart".to_string(), "system/com.example".to_string()],
        vec![
            "kickstart".to_string(),
            "-k".to_string(),
            "system/com.example".to_string(),
        ],
        vec![
            "kickstart".to_string(),
            "-p".to_string(),
            "system/com.example".to_string(),
        ],
        vec![
            "kill".to_string(),
            "SIGTERM".to_string(),
            "system/com.example".to_string(),
        ],
    ];
    let response = call(
        &server,
        &post(
            "/v1/launchd/control",
            &LaunchdControlRequest {
                commands: commands.clone(),
            },
        ),
    );
    assert_eq!(response.status, 200, "{}", error_of(&response));
    let expected: Vec<Vec<String>> = commands
        .iter()
        .map(|command| {
            let mut argv = vec!["/bin/launchctl".to_string()];
            argv.extend(command.clone());
            argv
        })
        .collect();
    assert_eq!(runner.argv(), expected);
}

#[test]
fn launchd_control_stops_at_the_first_failure() {
    let runner = FakeRunner::new(|_, args, _| {
        if args.first() == Some(&"bootout") {
            CommandOutput::failure(b"  Bootout failed: 36\n".to_vec(), "exit 36".to_string())
        } else {
            CommandOutput::success(Vec::new())
        }
    });
    let mut config = server_config();
    config.runner = runner.clone();
    let server = Server::new(config);
    let response = call(
        &server,
        &post(
            "/v1/launchd/control",
            &LaunchdControlRequest {
                commands: vec![
                    vec!["print".to_string(), "system/a".to_string()],
                    vec!["bootout".to_string(), "system/a".to_string()],
                    vec!["print".to_string(), "system/b".to_string()],
                ],
            },
        ),
    );
    assert_eq!(response.status, 500);
    assert_eq!(error_of(&response), "Bootout failed: 36");
    assert_eq!(runner.calls().len(), 2);
}

fn save_server(dir: &Path, runner: Arc<FakeRunner>) -> (Server, Arc<RecordingChown>) {
    let allowed = dir.to_str().expect("utf8").to_string();
    let chown = Arc::new(RecordingChown::default());
    let mut config = server_config();
    config.managed_launchd_dirs = vec![allowed];
    config.runner = runner;
    config.chown = chown.clone();
    (Server::new(config), chown)
}

fn save_request(dir: &Path, name: &str) -> LaunchdSaveRequest {
    LaunchdSaveRequest {
        plist: plist("com.example.worker"),
        path: dir
            .join(format!("{name}.plist"))
            .to_str()
            .expect("utf8")
            .to_string(),
        domain: "system".to_string(),
        target: "system/com.example.worker".to_string(),
        existing_path: String::new(),
        load: false,
    }
}

#[test]
fn launchd_save_validation_and_self_protection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let runner = FakeRunner::succeeding();
    let (server, _) = save_server(dir.path(), runner.clone());
    let good = save_request(dir.path(), "com.example.worker");

    let cases: Vec<(LaunchdSaveRequest, u16, &str)> = vec![
        (
            LaunchdSaveRequest {
                path: "relative.plist".to_string(),
                ..good.clone()
            },
            400,
            "must be absolute",
        ),
        (
            LaunchdSaveRequest {
                path: dir.path().join("x.txt").to_str().expect("utf8").to_string(),
                ..good.clone()
            },
            400,
            "must end with .plist",
        ),
        (
            LaunchdSaveRequest {
                domain: "bogus".to_string(),
                ..good.clone()
            },
            400,
            "invalid launchd domain",
        ),
        (
            LaunchdSaveRequest {
                target: "nonsense".to_string(),
                ..good.clone()
            },
            400,
            "invalid launchd target",
        ),
        (
            LaunchdSaveRequest {
                plist: Vec::new(),
                ..good.clone()
            },
            400,
            "plist data is empty",
        ),
        (
            LaunchdSaveRequest {
                target: format!("system/{LABEL}"),
                ..good.clone()
            },
            403,
            "cannot modify privileged helper job",
        ),
        (
            LaunchdSaveRequest {
                existing_path: dir
                    .path()
                    .join(format!("{LABEL}.plist"))
                    .to_str()
                    .expect("utf8")
                    .to_string(),
                ..good.clone()
            },
            403,
            "cannot modify privileged helper job",
        ),
        (
            LaunchdSaveRequest {
                existing_path: format!("{HELPER_DIR}/x.plist"),
                ..good.clone()
            },
            403,
            "cannot modify privileged helper job",
        ),
    ];
    for (request, status, fragment) in cases {
        let response = call(&server, &post("/v1/launchd/save", &request));
        assert_eq!(response.status, status, "{request:?}");
        assert!(
            error_of(&response).contains(fragment),
            "{request:?} -> {}",
            error_of(&response)
        );
    }
    assert!(runner.argv().is_empty());
}

#[test]
fn launchd_save_guards_existing_paths_and_destinations() {
    let dir = tempfile::tempdir().expect("temp dir");
    let runner = FakeRunner::succeeding();
    let (server, _) = save_server(dir.path(), runner.clone());

    // A destination that already exists is a conflict.
    let request = save_request(dir.path(), "com.example.worker");
    std::fs::write(&request.path, b"old").expect("existing job");
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 409);
    assert!(error_of(&response).contains("already exists"));
    std::fs::remove_file(&request.path).expect("cleanup");

    // A missing ExistingPath is a validation error.
    let request = LaunchdSaveRequest {
        existing_path: dir
            .path()
            .join("com.example.missing.plist")
            .to_str()
            .expect("utf8")
            .to_string(),
        ..save_request(dir.path(), "com.example.worker")
    };
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 400);
    assert!(error_of(&response).contains("lstat"));

    // An ExistingPath that is not a regular file is rejected.
    let existing_dir = dir.path().join("com.example.dir.plist");
    std::fs::create_dir(&existing_dir).expect("dir");
    let request = LaunchdSaveRequest {
        existing_path: existing_dir.to_str().expect("utf8").to_string(),
        ..save_request(dir.path(), "com.example.worker")
    };
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 400);
    assert!(error_of(&response).contains("is not a regular file"));
}

#[test]
fn launchd_save_writes_atomically_and_loads_in_order() {
    let dir = tempfile::tempdir().expect("temp dir");
    let runner = FakeRunner::succeeding();
    let (server, chown) = save_server(dir.path(), runner.clone());

    // Save without load writes the file and never calls launchctl.
    let request = save_request(dir.path(), "com.example.worker");
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_slice(), br#"{"saved":true}"#);
    assert_eq!(std::fs::read(&request.path).expect("saved"), request.plist);
    let mode = std::fs::metadata(&request.path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644);
    assert!(runner.argv().is_empty());
    // chown is skipped when not root, so nothing is recorded here.
    assert!(chown.calls.lock().expect("chown").is_empty());

    // Load bootstraps the saved path.
    let request = LaunchdSaveRequest {
        load: true,
        ..save_request(dir.path(), "com.example.loaded")
    };
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 200);
    assert_eq!(
        runner.argv(),
        vec![vec![
            "/bin/launchctl".to_string(),
            "bootstrap".to_string(),
            "system".to_string(),
            request.path.clone()
        ]]
    );

    // A bootstrap failure still saves and reports the output.
    let runner = FakeRunner::new(|program, args, _| {
        if args.first() == Some(&"bootstrap") {
            CommandOutput::failure(
                b"Bootstrap failed: 5\n".to_vec(),
                format!("{program}: exit status 5"),
            )
        } else {
            CommandOutput::success(Vec::new())
        }
    });
    let (server, _) = save_server(dir.path(), runner);
    let request = LaunchdSaveRequest {
        load: true,
        ..save_request(dir.path(), "com.example.failed")
    };
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_slice(),
        br#"{"saved":true,"error":"Bootstrap failed: 5"}"#
    );
    assert_eq!(std::fs::read(&request.path).expect("saved"), request.plist);
}

#[test]
fn launchd_save_replaces_and_reloads_an_existing_job() {
    let dir = tempfile::tempdir().expect("temp dir");
    let runner = FakeRunner::succeeding();
    let (server, _) = save_server(dir.path(), runner.clone());

    let old = save_request(dir.path(), "com.example.old");
    std::fs::write(&old.path, b"old").expect("old job");
    let new = LaunchdSaveRequest {
        load: true,
        existing_path: old.path.clone(),
        ..save_request(dir.path(), "com.example.new")
    };
    let response = call(&server, &post("/v1/launchd/save", &new));
    assert_eq!(response.status, 200, "{}", error_of(&response));
    assert!(!Path::new(&old.path).exists(), "old job removed");
    assert!(Path::new(&new.path).exists());
    assert_eq!(
        runner.argv(),
        vec![
            vec![
                "/bin/launchctl".to_string(),
                "print".to_string(),
                "system/com.example.worker".to_string()
            ],
            vec![
                "/bin/launchctl".to_string(),
                "bootout".to_string(),
                "system/com.example.worker".to_string()
            ],
            vec![
                "/bin/launchctl".to_string(),
                "bootstrap".to_string(),
                "system".to_string(),
                new.path.clone()
            ],
        ]
    );
}

#[test]
fn launchd_save_reloads_the_same_path_and_reports_bootout_failure() {
    let dir = tempfile::tempdir().expect("temp dir");
    let request = LaunchdSaveRequest {
        load: true,
        existing_path: save_request(dir.path(), "com.example.worker").path,
        ..save_request(dir.path(), "com.example.worker")
    };
    std::fs::write(&request.path, &request.plist).expect("existing job");
    // print fails: no bootout, then bootstrap.
    let runner = FakeRunner::new(|_, args, _| {
        if args.first() == Some(&"print") {
            CommandOutput::failure(Vec::new(), "not loaded".to_string())
        } else {
            CommandOutput::success(Vec::new())
        }
    });
    let (server, _) = save_server(dir.path(), runner.clone());
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 200);
    assert_eq!(
        runner.argv(),
        vec![
            vec![
                "/bin/launchctl".to_string(),
                "print".to_string(),
                "system/com.example.worker".to_string()
            ],
            vec![
                "/bin/launchctl".to_string(),
                "bootstrap".to_string(),
                "system".to_string(),
                request.path.clone()
            ],
        ]
    );

    // print succeeds and bootout fails: saved with the bootout error, no
    // bootstrap.
    let runner = FakeRunner::new(|_, args, _| match args.first() {
        Some(&"bootout") => {
            CommandOutput::failure(b"Bootout failed: 36\n".to_vec(), "exit 36".to_string())
        }
        _ => CommandOutput::success(Vec::new()),
    });
    let (server, _) = save_server(dir.path(), runner.clone());
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_slice(),
        br#"{"saved":true,"error":"Bootout failed: 36"}"#
    );
    assert_eq!(runner.argv().len(), 2);
    assert!(runner.argv().iter().all(|argv| argv[1] != "bootstrap"));
}

fn remove_server(dir: &Path, runner: Arc<FakeRunner>) -> (Server, Arc<RecordingChown>) {
    let chown = Arc::new(RecordingChown::default());
    let mut config = server_config();
    config.managed_launchd_dirs = vec![dir.to_str().expect("utf8").to_string()];
    config.runner = runner;
    config.chown = chown.clone();
    (Server::new(config), chown)
}

fn remove_request(dir: &Path, trash: &Path) -> LaunchdRemoveRequest {
    LaunchdRemoveRequest {
        path: dir
            .join("com.example.worker.plist")
            .to_str()
            .expect("utf8")
            .to_string(),
        target: "system/com.example.worker".to_string(),
        trash_dir: trash.to_str().expect("utf8").to_string(),
    }
}

#[test]
fn launchd_remove_validation_matrix() {
    let dir = tempfile::tempdir().expect("temp dir");
    let trash = tempfile::tempdir().expect("trash");
    let runner = FakeRunner::succeeding();
    let (server, _) = remove_server(dir.path(), runner.clone());
    let good = remove_request(dir.path(), trash.path());

    let cases: Vec<(LaunchdRemoveRequest, &str)> = vec![
        (
            LaunchdRemoveRequest {
                path: "relative.plist".to_string(),
                ..good.clone()
            },
            "must be absolute",
        ),
        (
            LaunchdRemoveRequest {
                target: format!("system/{LABEL}"),
                ..good.clone()
            },
            "cannot remove privileged helper job",
        ),
        (
            LaunchdRemoveRequest {
                target: "bogus".to_string(),
                ..good.clone()
            },
            "invalid launchd target",
        ),
        (
            LaunchdRemoveRequest {
                trash_dir: "relative".to_string(),
                ..good.clone()
            },
            "trash directory must be absolute",
        ),
        (
            LaunchdRemoveRequest {
                trash_dir: "/".to_string(),
                ..good.clone()
            },
            "invalid trash directory",
        ),
        (
            LaunchdRemoveRequest {
                trash_dir: format!("{}/", trash.path().display()),
                ..good.clone()
            },
            "invalid trash directory",
        ),
        (
            LaunchdRemoveRequest {
                trash_dir: trash
                    .path()
                    .join("missing")
                    .to_str()
                    .expect("utf8")
                    .to_string(),
                ..good.clone()
            },
            "lstat",
        ),
    ];
    for (request, fragment) in cases {
        let response = call(&server, &post("/v1/launchd/remove", &request));
        let expected = if fragment == "cannot remove privileged helper job" {
            403
        } else {
            400
        };
        assert_eq!(response.status, expected, "{request:?}");
        assert!(
            error_of(&response).contains(fragment),
            "{request:?} -> {}",
            error_of(&response)
        );
    }

    // A file instead of a directory is not a trash directory.
    let file = dir.path().join("trash-file");
    std::fs::write(&file, b"x").expect("file");
    let request = LaunchdRemoveRequest {
        trash_dir: file.to_str().expect("utf8").to_string(),
        ..good.clone()
    };
    let response = call(&server, &post("/v1/launchd/remove", &request));
    assert_eq!(response.status, 400);
    assert!(error_of(&response).contains("must be a real directory"));

    // A symlinked trash directory is refused.
    let link = dir.path().join("trash-link");
    std::os::unix::fs::symlink(trash.path(), &link).expect("symlink");
    let request = LaunchdRemoveRequest {
        trash_dir: link.to_str().expect("utf8").to_string(),
        ..good.clone()
    };
    let response = call(&server, &post("/v1/launchd/remove", &request));
    assert_eq!(response.status, 400);
    assert!(error_of(&response).contains("must be a real directory"));
    assert!(runner.argv().is_empty());
}

#[test]
fn launchd_remove_moves_the_file_and_chowns_it_to_the_caller() {
    let dir = tempfile::tempdir().expect("temp dir");
    let trash = tempfile::tempdir().expect("trash");
    let runner = FakeRunner::new(|_, args, _| {
        if args.first() == Some(&"print") {
            CommandOutput::success(b"state = active\n".to_vec())
        } else {
            CommandOutput::success(Vec::new())
        }
    });
    let (server, chown) = remove_server(dir.path(), runner.clone());
    let request = remove_request(dir.path(), trash.path());
    std::fs::write(&request.path, plist("com.example.worker")).expect("job");
    let destination = trash.path().join("com.example.worker.plist");

    let response = call(&server, &post("/v1/launchd/remove", &request));
    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_slice(), br#"{"ok":true}"#);
    assert!(!Path::new(&request.path).exists());
    assert_eq!(
        std::fs::read(&destination).expect("trashed"),
        plist("com.example.worker")
    );
    assert_eq!(
        runner.argv(),
        vec![
            vec![
                "/bin/launchctl".to_string(),
                "print".to_string(),
                "system/com.example.worker".to_string()
            ],
            vec![
                "/bin/launchctl".to_string(),
                "bootout".to_string(),
                "system/com.example.worker".to_string()
            ],
        ]
    );
    let calls = chown.calls.lock().expect("chown");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, destination);
    assert_eq!(calls[0].1, euid());
    assert_eq!(calls[0].2, -1);
}

#[test]
fn launchd_remove_rejects_a_missing_or_non_regular_job_and_an_existing_destination() {
    let dir = tempfile::tempdir().expect("temp dir");
    let trash = tempfile::tempdir().expect("trash");
    let runner = FakeRunner::succeeding();
    let (server, _) = remove_server(dir.path(), runner.clone());
    let request = remove_request(dir.path(), trash.path());

    let response = call(&server, &post("/v1/launchd/remove", &request));
    assert_eq!(response.status, 400);
    assert!(error_of(&response).contains("is not a regular file"));

    std::fs::create_dir(&request.path).expect("dir at job path");
    let response = call(&server, &post("/v1/launchd/remove", &request));
    assert_eq!(response.status, 400);
    std::fs::remove_dir(&request.path).expect("cleanup");

    std::fs::write(&request.path, b"job").expect("job");
    std::fs::write(trash.path().join("com.example.worker.plist"), b"already").expect("destination");
    let response = call(&server, &post("/v1/launchd/remove", &request));
    assert_eq!(response.status, 409);
    assert!(error_of(&response).contains("already exists"));
    assert!(runner.argv().is_empty());
}

#[test]
fn launchd_remove_reports_a_bootout_failure_without_moving() {
    let dir = tempfile::tempdir().expect("temp dir");
    let trash = tempfile::tempdir().expect("trash");
    let runner = FakeRunner::new(|_, args, _| {
        if args.first() == Some(&"bootout") {
            CommandOutput::failure(b"Bootout failed: 36\n".to_vec(), "exit 36".to_string())
        } else {
            CommandOutput::success(Vec::new())
        }
    });
    let (server, _) = remove_server(dir.path(), runner);
    let request = remove_request(dir.path(), trash.path());
    std::fs::write(&request.path, b"job").expect("job");
    let response = call(&server, &post("/v1/launchd/remove", &request));
    assert_eq!(response.status, 500);
    assert_eq!(error_of(&response), "Bootout failed: 36");
    assert!(Path::new(&request.path).exists());
}

#[test]
fn request_decoding_matches_the_go_limits() {
    let (server, _) = direct_server(None);

    let invalid = call(
        &server,
        &Request::with_body("POST", "/v1/ports/inspect", b"{".to_vec()),
    );
    assert_eq!(invalid.status, 400);

    let trailing = call(
        &server,
        &Request::with_body("POST", "/v1/ports/inspect", br#"{"port":443}{}"#.to_vec()),
    );
    assert_eq!(trailing.status, 400);
    assert_eq!(error_of(&trailing), "unexpected trailing JSON data");

    // Trailing whitespace is fine, like encoding/json.
    let whitespace = call(
        &server,
        &Request::with_body("POST", "/v1/ports/inspect", b"{\"port\":443}\n  ".to_vec()),
    );
    assert_eq!(whitespace.status, 200);

    let empty = call(
        &server,
        &Request::with_body("POST", "/v1/ports/inspect", Vec::new()),
    );
    assert_eq!(empty.status, 400);
    assert_eq!(error_of(&empty), "EOF");

    let oversized = vec![b' '; super::MAX_REQUEST_BYTES + 1];
    let too_large = call(
        &server,
        &Request::with_body("POST", "/v1/ports/inspect", oversized),
    );
    assert_eq!(too_large.status, 400);
    assert_eq!(error_of(&too_large), "http: request body too large");

    let unknown = call(
        &server,
        &Request::with_body(
            "POST",
            "/v1/ports/inspect",
            br#"{"port":443,"x":1}"#.to_vec(),
        ),
    );
    assert_eq!(unknown.status, 200);
}

#[test]
fn helper_self_protection_covers_targets_paths_and_control_arguments() {
    let dir = tempfile::tempdir().expect("temp dir");
    let runner = FakeRunner::succeeding();
    let (server, _) = save_server(dir.path(), runner.clone());
    // A save whose path is the helper's own plist basename is refused.
    let request = LaunchdSaveRequest {
        path: dir
            .path()
            .join(format!("{LABEL}.plist"))
            .to_str()
            .expect("utf8")
            .to_string(),
        plist: plist("com.example"),
        domain: "system".to_string(),
        target: "system/com.example".to_string(),
        existing_path: String::new(),
        load: false,
    };
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 403);

    // Every control argument is inspected, not only the target.
    let response = call(
        &server,
        &post(
            "/v1/launchd/control",
            &LaunchdControlRequest {
                commands: vec![vec!["bootout".to_string(), format!("gui/501/{LABEL}")]],
            },
        ),
    );
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response), "cannot target privileged helper");
    assert!(runner.argv().is_empty());
}

#[test]
fn launchd_save_rejects_a_symlinked_existing_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let runner = FakeRunner::succeeding();
    let (server, _) = save_server(dir.path(), runner);
    let real = dir.path().join("com.example.real.plist");
    std::fs::write(&real, b"x").expect("real");
    let link = dir.path().join("com.example.link.plist");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let request = LaunchdSaveRequest {
        existing_path: link.to_str().expect("utf8").to_string(),
        ..save_request(dir.path(), "com.example.worker")
    };
    let response = call(&server, &post("/v1/launchd/save", &request));
    assert_eq!(response.status, 400);
    assert!(error_of(&response).contains("refusing symlink"));
    assert!(!PathBuf::from(&request.path).exists());
}

#[test]
fn command_runner_failure_without_output_uses_the_error() {
    let runner = FakeRunner::new(|_, _, _| {
        CommandOutput::failure(Vec::new(), "launchctl: exit status 1".to_string())
    });
    let mut config = server_config();
    config.runner = runner;
    let server = Server::new(config);
    let response = call(
        &server,
        &post(
            "/v1/launchd/control",
            &LaunchdControlRequest {
                commands: vec![vec!["print".to_string(), "system/x".to_string()]],
            },
        ),
    );
    assert_eq!(response.status, 500);
    assert_eq!(error_of(&response), "launchctl: exit status 1");
}

#[test]
fn server_config_fills_defaults() {
    let server = Server::new(ServerConfig::new(0));
    assert_eq!(
        server.config().idle_timeout,
        super::server::DEFAULT_IDLE_TIMEOUT
    );
    assert_eq!(
        server.config().managed_launchd_dirs,
        vec![
            "/Library/LaunchAgents".to_string(),
            "/Library/LaunchDaemons".to_string()
        ]
    );
    assert_eq!(
        server.config().socket_job_dirs,
        vec!["/Library/LaunchDaemons".to_string()]
    );
}
