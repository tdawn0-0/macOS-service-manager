//! The helper's HTTP/1.1 server over the launchd-activated Unix socket.
//!
//! Ported from `server.go`: the route table, the two-layer authorization
//! middleware, every endpoint's validation, the atomic plist writes, the
//! `launchctl`/`lsof` command vectors, and the 60-second idle shutdown.
//!
//! All process execution, peer identity and ownership changes are injected
//! through [`CommandRunner`], [`super::auth::PeerAuth`] and [`Chowner`], so
//! tests never run `launchctl` or `lsof` and never need root.

use std::fmt;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::auth::{PeerAuth, SystemPeerAuth};
use super::http::{self, ReadError, Request, Response};
use super::paths::{clean_path, dir_path, lstat_error, validate_clean_target_file};
use super::protocol::{
    CaInstallRequest, CaRemoveRequest, ErrorResponse, LaunchdControlRequest, LaunchdRemoveRequest,
    LaunchdSaveRequest, LaunchdSaveResponse, LinuxProxySocketInstallRequest,
    LinuxProxySocketStopRequest, OkResponse, PortInspectRequest, PortInspectResponse,
    ProxySocketInstallRequest, ProxySocketKickstartRequest, ProxySocketStopRequest, StatusResponse,
    SystemdControlRequest, SystemdRemoveRequest, SystemdSaveRequest, encode_server_json,
};
use super::validate::{
    path_basename, targets_helper, targets_linux_helper, validate_ca_name, validate_ca_pem,
    validate_domain, validate_launchctl_command, validate_launchd_target,
    validate_linux_proxy_unit_names, validate_proxy_socket_request, validate_proxy_socket_target,
    validate_systemctl_command, validate_systemd_unit_file, validate_systemd_unit_name,
    validate_xml_plist,
};
use super::{MAX_REQUEST_BYTES, PEER_REJECTED_MESSAGE, PROTOCOL_VERSION};

/// The Go server's default idle timeout.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// The Go server's `ReadHeaderTimeout`.
pub const READ_HEADER_TIMEOUT: Duration = Duration::from_secs(5);

/// The directories root-managed launchd jobs live in.
pub const DEFAULT_MANAGED_LAUNCHD_DIRS: [&str; 2] =
    ["/Library/LaunchAgents", "/Library/LaunchDaemons"];

/// The directories launchd socket jobs live in.
pub const DEFAULT_SOCKET_JOB_DIRS: [&str; 1] = ["/Library/LaunchDaemons"];

/// The directories root-managed systemd unit files live in on Linux.
pub const DEFAULT_MANAGED_SYSTEMD_DIRS: [&str; 1] = ["/etc/systemd/system"];

/// Describes a Linux distribution's system CA trust anchor directory and update command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaTrustTarget {
    pub anchor_dir: PathBuf,
    pub extension: String,
    pub update_program: String,
    pub update_args: Vec<String>,
}

/// Detects the active Linux distribution's system CA trust anchor store.
pub fn detect_linux_ca_trust_target() -> Result<CaTrustTarget, String> {
    let candidates = [
        // Arch / CachyOS / p11-kit
        (
            "/etc/ca-certificates/trust-source/anchors",
            "crt",
            "update-ca-trust",
            vec![],
        ),
        // Fedora / RHEL / CentOS / Rocky / Alma
        (
            "/etc/pki/ca-trust/source/anchors",
            "pem",
            "update-ca-trust",
            vec!["extract"],
        ),
        // Debian / Ubuntu
        (
            "/usr/local/share/ca-certificates",
            "crt",
            "update-ca-certificates",
            vec![],
        ),
    ];
    for (dir, ext, prog, args) in candidates {
        if Path::new(dir).is_dir() {
            return Ok(CaTrustTarget {
                anchor_dir: PathBuf::from(dir),
                extension: ext.to_string(),
                update_program: prog.to_string(),
                update_args: args.into_iter().map(String::from).collect(),
            });
        }
    }
    Err("could not locate a supported Linux CA trust anchor directory".to_string())
}

/// The result of running one command: combined output and why it failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutput {
    pub output: Vec<u8>,
    pub error: Option<String>,
}

impl CommandOutput {
    pub fn success(output: Vec<u8>) -> Self {
        CommandOutput {
            output,
            error: None,
        }
    }

    pub fn failure(output: Vec<u8>, message: impl Into<String>) -> Self {
        CommandOutput {
            output,
            error: Some(message.into()),
        }
    }

    pub fn failed(&self) -> bool {
        self.error.is_some()
    }
}

/// Runs one program to completion for the helper, capturing combined output.
pub trait CommandRunner: Send + Sync + fmt::Debug {
    fn run(&self, program: &str, args: &[&str]) -> CommandOutput;
}

/// The production runner: no shell, combined stdout and stderr.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> CommandOutput {
        match std::process::Command::new(program).args(args).output() {
            Ok(output) => {
                let mut combined = output.stdout;
                combined.extend_from_slice(&output.stderr);
                if output.status.success() {
                    CommandOutput::success(combined)
                } else {
                    let code = output
                        .status
                        .code()
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "signal".to_string());
                    CommandOutput::failure(combined, format!("{program}: exit status {code}"))
                }
            }
            Err(error) => CommandOutput::failure(Vec::new(), format!("{program}: {error}")),
        }
    }
}

/// Changes a path's ownership, used only when the helper runs as root.
pub trait Chowner: Send + Sync + fmt::Debug {
    fn chown(&self, path: &Path, uid: u32, gid: i32) -> std::io::Result<()>;
}

/// The production `lchown`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemChown;

impl Chowner for SystemChown {
    fn chown(&self, path: &Path, uid: u32, gid: i32) -> std::io::Result<()> {
        let path = path
            .to_str()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path"))?;
        let c_path = std::ffi::CString::new(path)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path"))?;
        // SAFETY: the C string is valid and lchown does not retain it.
        let rc = unsafe { libc::lchown(c_path.as_ptr(), uid, gid as libc::gid_t) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Configures the helper server. Defaults match `NewServer`.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub owner_uid: i64,
    pub idle_timeout: Duration,
    pub managed_launchd_dirs: Vec<String>,
    pub socket_job_dirs: Vec<String>,
    pub managed_systemd_dirs: Vec<String>,
    pub ca_trust_target: Option<CaTrustTarget>,
    pub runner: Arc<dyn CommandRunner>,
    pub peer_auth: Arc<dyn PeerAuth>,
    pub chown: Arc<dyn Chowner>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            owner_uid: 0,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            managed_launchd_dirs: DEFAULT_MANAGED_LAUNCHD_DIRS
                .iter()
                .map(|dir| (*dir).to_string())
                .collect(),
            socket_job_dirs: DEFAULT_SOCKET_JOB_DIRS
                .iter()
                .map(|dir| (*dir).to_string())
                .collect(),
            managed_systemd_dirs: DEFAULT_MANAGED_SYSTEMD_DIRS
                .iter()
                .map(|dir| (*dir).to_string())
                .collect(),
            ca_trust_target: None,
            runner: Arc::new(SystemCommandRunner),
            peer_auth: Arc::new(SystemPeerAuth),
            chown: Arc::new(SystemChown),
        }
    }
}

impl ServerConfig {
    pub fn new(owner_uid: i64) -> Self {
        ServerConfig {
            owner_uid,
            ..ServerConfig::default()
        }
    }
}

struct IdleState {
    active: usize,
    last_touch: Instant,
}

struct ActiveGuard<'a> {
    server: &'a Server,
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.server.end_activity();
    }
}

struct OwnedActiveGuard {
    server: Arc<Server>,
}

impl Drop for OwnedActiveGuard {
    fn drop(&mut self) {
        self.server.end_activity();
    }
}

/// Serves privileged operations over the Unix socket for `owner_uid`.
pub struct Server {
    cfg: ServerConfig,
    idle: Mutex<IdleState>,
}

impl Server {
    /// Constructs a server with validated defaults.
    pub fn new(mut cfg: ServerConfig) -> Self {
        if cfg.idle_timeout.is_zero() {
            cfg.idle_timeout = DEFAULT_IDLE_TIMEOUT;
        }
        if cfg.managed_launchd_dirs.is_empty() {
            cfg.managed_launchd_dirs = DEFAULT_MANAGED_LAUNCHD_DIRS
                .iter()
                .map(|dir| (*dir).to_string())
                .collect();
        }
        if cfg.socket_job_dirs.is_empty() {
            cfg.socket_job_dirs = DEFAULT_SOCKET_JOB_DIRS
                .iter()
                .map(|dir| (*dir).to_string())
                .collect();
        }
        if cfg.managed_systemd_dirs.is_empty() {
            cfg.managed_systemd_dirs = DEFAULT_MANAGED_SYSTEMD_DIRS
                .iter()
                .map(|dir| (*dir).to_string())
                .collect();
        }
        Server {
            cfg,
            idle: Mutex::new(IdleState {
                active: 0,
                last_touch: Instant::now(),
            }),
        }
    }

    pub fn config(&self) -> &ServerConfig {
        &self.cfg
    }

    /// Serves until the idle timeout expires or the server is stopped.
    pub fn serve(self: &Arc<Self>, listener: UnixListener) -> std::io::Result<()> {
        self.serve_until(listener, Arc::new(AtomicBool::new(false)))
    }

    /// Serves until `stop` is set, including by the idle loop.
    pub fn serve_until(
        self: &Arc<Self>,
        listener: UnixListener,
        stop: Arc<AtomicBool>,
    ) -> std::io::Result<()> {
        self.touch();
        listener.set_nonblocking(true)?;
        let idle_server = Arc::clone(self);
        let idle_stop = Arc::clone(&stop);
        let idle = std::thread::spawn(move || idle_server.idle_loop(&idle_stop));

        let mut connections = Vec::new();
        let mut result = Ok(());
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(error) = stream.set_nonblocking(false) {
                        result = Err(error);
                        break;
                    }
                    let guard = self.track_owned_activity();
                    let server = Arc::clone(self);
                    connections.push(std::thread::spawn(move || {
                        let _guard = guard;
                        server.serve_connection(stream);
                    }));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => {
                    result = Err(error);
                    stop.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
        drop(listener);
        for connection in connections {
            let _ = connection.join();
        }
        let _ = idle.join();
        result
    }

    fn serve_connection(&self, mut stream: UnixStream) {
        let _ = http::set_timeouts(&stream, READ_HEADER_TIMEOUT);
        let response = match http::read_request(&mut stream, MAX_REQUEST_BYTES) {
            Ok(request) => self.handle(&request, Some(&stream)),
            Err(ReadError::TooLarge) => error_response(400, "http: request body too large"),
            Err(ReadError::BadRequest(message)) => Response::text(400, &format!("{message}\n")),
            Err(ReadError::Io(_)) => return,
        };
        let _ = http::write_response(&mut stream, &response);
    }

    /// Applies authorization and routing to one request. Public within the
    /// crate so endpoint tests never need a socket.
    pub(crate) fn handle(&self, request: &Request, peer: Option<&UnixStream>) -> Response {
        let _guard = self.track_activity();
        match self.authorize(peer) {
            Ok(()) => self.route(request),
            Err(response) => response,
        }
    }

    fn begin_activity(&self) {
        if let Ok(mut state) = self.idle.lock() {
            state.active += 1;
            state.last_touch = Instant::now();
        }
    }

    fn end_activity(&self) {
        if let Ok(mut state) = self.idle.lock() {
            state.active = state.active.saturating_sub(1);
            state.last_touch = Instant::now();
        }
    }

    fn track_activity(&self) -> ActiveGuard<'_> {
        self.begin_activity();
        ActiveGuard { server: self }
    }

    fn track_owned_activity(self: &Arc<Self>) -> OwnedActiveGuard {
        self.begin_activity();
        OwnedActiveGuard {
            server: Arc::clone(self),
        }
    }

    fn touch(&self) {
        if let Ok(mut state) = self.idle.lock() {
            state.last_touch = Instant::now();
        }
    }

    fn idle_loop(&self, stop: &AtomicBool) {
        loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let wait = {
                let Ok(mut state) = self.idle.lock() else {
                    return;
                };
                if state.active > 0 {
                    state.last_touch = Instant::now();
                    self.cfg.idle_timeout.min(Duration::from_millis(10))
                } else {
                    let elapsed = state.last_touch.elapsed();
                    if elapsed >= self.cfg.idle_timeout {
                        stop.store(true, Ordering::SeqCst);
                        return;
                    }
                    (self.cfg.idle_timeout - elapsed).min(Duration::from_millis(10))
                }
            };
            std::thread::sleep(wait);
        }
    }

    fn authorize(&self, peer: Option<&UnixStream>) -> Result<(), Response> {
        let Some(peer) = peer else {
            return Err(error_response(403, "missing peer connection"));
        };
        match self.cfg.peer_auth.peer_uid(peer) {
            Ok(uid) => {
                if self.cfg.owner_uid <= 0 || i64::from(uid) != self.cfg.owner_uid {
                    return Err(error_response(
                        403,
                        format!("peer uid {uid} is not authorized"),
                    ));
                }
            }
            Err(_) => {
                return Err(error_response(403, "failed to verify peer uid"));
            }
        }
        if let Err(error) = self.cfg.peer_auth.peer_authorized(peer) {
            eprintln!("privhelper: peer code check failed: {error}");
            return Err(error_response(403, PEER_REJECTED_MESSAGE));
        }
        Ok(())
    }

    fn route(&self, request: &Request) -> Response {
        const ROUTES: [(&str, &str); 15] = [
            ("GET", "/v1/status"),
            ("POST", "/v1/proxy-socket/install"),
            ("POST", "/v1/proxy-socket/stop"),
            ("POST", "/v1/proxy-socket/kickstart"),
            ("POST", "/v1/ports/inspect"),
            ("POST", "/v1/launchd/control"),
            ("POST", "/v1/launchd/save"),
            ("POST", "/v1/launchd/remove"),
            ("POST", "/v1/systemd/control"),
            ("POST", "/v1/systemd/save"),
            ("POST", "/v1/systemd/remove"),
            ("POST", "/v1/ca/install"),
            ("POST", "/v1/ca/remove"),
            ("POST", "/v1/systemd/proxy-socket/install"),
            ("POST", "/v1/systemd/proxy-socket/stop"),
        ];
        let Some((allowed, _)) = ROUTES
            .iter()
            .find(|(_, path)| *path == request.path)
            .copied()
        else {
            return Response::text(404, "404 page not found\n");
        };
        let head_of_get = allowed == "GET" && request.method == "HEAD";
        if request.method != allowed && !head_of_get {
            return Response::text(405, "Method Not Allowed\n").with_header("Allow", allowed);
        }
        match request.path.as_str() {
            "/v1/status" => self.handle_status(),
            "/v1/proxy-socket/install" => self.handle_proxy_socket_install(request),
            "/v1/proxy-socket/stop" => self.handle_proxy_socket_stop(request),
            "/v1/proxy-socket/kickstart" => self.handle_proxy_socket_kickstart(request),
            "/v1/ports/inspect" => self.handle_port_inspect(request),
            "/v1/launchd/control" => self.handle_launchd_control(request),
            "/v1/launchd/save" => self.handle_launchd_save(request),
            "/v1/launchd/remove" => self.handle_launchd_remove(request),
            "/v1/systemd/control" => self.handle_systemd_control(request),
            "/v1/systemd/save" => self.handle_systemd_save(request),
            "/v1/systemd/remove" => self.handle_systemd_remove(request),
            "/v1/ca/install" => self.handle_ca_install(request),
            "/v1/ca/remove" => self.handle_ca_remove(request),
            "/v1/systemd/proxy-socket/install" => self.handle_linux_proxy_socket_install(request),
            "/v1/systemd/proxy-socket/stop" => self.handle_linux_proxy_socket_stop(request),
            _ => Response::text(404, "404 page not found\n"),
        }
    }

    fn handle_status(&self) -> Response {
        json_response(
            200,
            &StatusResponse {
                protocol_version: PROTOCOL_VERSION,
                owner_uid: self.cfg.owner_uid,
            },
        )
    }

    fn handle_proxy_socket_install(&self, request: &Request) -> Response {
        let req: ProxySocketInstallRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        let job_path = match validate_proxy_socket_request(
            &req.job_path,
            &req.target,
            &self.cfg.socket_job_dirs,
        ) {
            Ok(path) => path,
            Err(message) => return error_response(400, message),
        };
        if let Err(message) = validate_xml_plist(&req.plist) {
            return error_response(400, message);
        }
        if let Err(error) = atomic_write_file(Path::new(&job_path), &req.plist, 0o644) {
            return error_response(500, error.to_string());
        }
        self.chown_root(&job_path);
        let _ = self.run("/bin/launchctl", &["bootout", &req.target]);
        let output = self.run("/bin/launchctl", &["bootstrap", "system", &job_path]);
        if output.failed() {
            return error_response(500, cmd_error(&output));
        }
        json_response(200, &OkResponse::success())
    }

    fn handle_proxy_socket_stop(&self, request: &Request) -> Response {
        let req: ProxySocketStopRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        let job_path = match validate_proxy_socket_request(
            &req.job_path,
            &req.target,
            &self.cfg.socket_job_dirs,
        ) {
            Ok(path) => path,
            Err(message) => return error_response(400, message),
        };
        if !self.run("/bin/launchctl", &["print", &req.target]).failed() {
            let output = self.run("/bin/launchctl", &["bootout", &req.target]);
            if output.failed() {
                return error_response(500, cmd_error(&output));
            }
        }
        match std::fs::remove_file(&job_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return error_response(500, error.to_string()),
        }
        json_response(200, &OkResponse::success())
    }

    fn handle_proxy_socket_kickstart(&self, request: &Request) -> Response {
        let req: ProxySocketKickstartRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        if let Err(message) = validate_proxy_socket_target(&req.target) {
            return error_response(400, message);
        }
        let output = self.run("/bin/launchctl", &["kickstart", "-k", &req.target]);
        if output.failed() {
            return error_response(500, cmd_error(&output));
        }
        json_response(200, &OkResponse::success())
    }

    fn handle_port_inspect(&self, request: &Request) -> Response {
        let req: PortInspectRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        if req.port <= 0 || req.port > 65535 {
            return error_response(400, format!("invalid port {}", req.port));
        }
        let port = format!("-iTCP:{}", req.port);
        let output = self.run(
            "/usr/sbin/lsof",
            &["-nP", "-a", &port, "-sTCP:LISTEN", "+c0", "-F0pcun"],
        );
        if output.failed() && output.output.is_empty() {
            return error_response(500, cmd_error(&output));
        }
        json_response(
            200,
            &PortInspectResponse {
                output: output.output,
            },
        )
    }

    fn handle_launchd_control(&self, request: &Request) -> Response {
        let req: LaunchdControlRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        if req.commands.is_empty() {
            return error_response(400, "no commands specified");
        }
        for command in &req.commands {
            if let Err(message) = validate_launchctl_command(command) {
                return error_response(400, message);
            }
        }
        for command in &req.commands {
            let args: Vec<&str> = command.iter().map(String::as_str).collect();
            let output = self.run("/bin/launchctl", &args);
            if output.failed() {
                return error_response(500, cmd_error(&output));
            }
        }
        json_response(200, &OkResponse::success())
    }

    fn handle_launchd_save(&self, request: &Request) -> Response {
        let mut req: LaunchdSaveRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        let path =
            match validate_clean_target_file(&req.path, &self.cfg.managed_launchd_dirs, ".plist") {
                Ok(path) => path,
                Err(message) => return error_response(400, message),
            };
        if targets_helper(&req.target, &path) || targets_helper("", &req.existing_path) {
            return error_response(403, "cannot modify privileged helper job");
        }
        if !req.existing_path.is_empty() {
            match validate_clean_target_file(
                &req.existing_path,
                &self.cfg.managed_launchd_dirs,
                ".plist",
            ) {
                Ok(existing) => req.existing_path = existing,
                Err(message) => return error_response(400, message),
            }
        }
        if let Err(message) = validate_domain(&req.domain) {
            return error_response(400, message);
        }
        if let Err(message) = validate_launchd_target(&req.target) {
            return error_response(400, message);
        }
        if let Err(message) = validate_xml_plist(&req.plist) {
            return error_response(400, message);
        }

        if req.existing_path.is_empty() {
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    return error_response(409, format!("job {path} already exists"));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return error_response(500, error.to_string()),
            }
        } else {
            let existing = match std::fs::symlink_metadata(&req.existing_path) {
                Ok(metadata) => metadata,
                Err(error) => return error_response(400, lstat_error(&req.existing_path, &error)),
            };
            if !existing.is_file() {
                return error_response(
                    400,
                    format!("existing job {} is not a regular file", req.existing_path),
                );
            }
        }

        if let Err(error) = std::fs::create_dir_all(dir_path(&path)) {
            return error_response(500, error.to_string());
        }
        if let Err(error) = atomic_write_file(Path::new(&path), &req.plist, 0o644) {
            return error_response(500, error.to_string());
        }
        self.chown_root(&path);

        if !req.existing_path.is_empty() && req.existing_path != path {
            if !self.run("/bin/launchctl", &["print", &req.target]).failed() {
                let output = self.run("/bin/launchctl", &["bootout", &req.target]);
                if output.failed() {
                    return error_response(500, cmd_error(&output));
                }
            }
            match std::fs::remove_file(&req.existing_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return error_response(500, error.to_string()),
            }
        }

        if req.load {
            if req.existing_path == path
                && !self.run("/bin/launchctl", &["print", &req.target]).failed()
            {
                let output = self.run("/bin/launchctl", &["bootout", &req.target]);
                if output.failed() {
                    return json_response(
                        200,
                        &LaunchdSaveResponse {
                            saved: true,
                            error: cmd_error(&output),
                        },
                    );
                }
            }
            let output = self.run("/bin/launchctl", &["bootstrap", &req.domain, &path]);
            if output.failed() {
                return json_response(
                    200,
                    &LaunchdSaveResponse {
                        saved: true,
                        error: cmd_error(&output),
                    },
                );
            }
        }
        json_response(
            200,
            &LaunchdSaveResponse {
                saved: true,
                error: String::new(),
            },
        )
    }

    fn handle_launchd_remove(&self, request: &Request) -> Response {
        let req: LaunchdRemoveRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        let path =
            match validate_clean_target_file(&req.path, &self.cfg.managed_launchd_dirs, ".plist") {
                Ok(path) => path,
                Err(message) => return error_response(400, message),
            };
        if targets_helper(&req.target, &path) {
            return error_response(403, "cannot remove privileged helper job");
        }
        if let Err(message) = validate_launchd_target(&req.target) {
            return error_response(400, message);
        }
        let trash_dir = match self.validate_trash_dir(&req.trash_dir) {
            Ok(dir) => dir,
            Err(message) => return error_response(400, message),
        };

        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            _ => {
                return error_response(400, format!("job {path} is not a regular file"));
            }
        }
        let destination = Path::new(&trash_dir).join(path_basename(&path));
        if !matches!(
            std::fs::symlink_metadata(&destination),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ) {
            return error_response(
                409,
                format!("destination {} already exists", destination.display()),
            );
        }

        if !self.run("/bin/launchctl", &["print", &req.target]).failed() {
            let output = self.run("/bin/launchctl", &["bootout", &req.target]);
            if output.failed() {
                return error_response(500, cmd_error(&output));
            }
        }
        if let Err(error) = move_file(Path::new(&path), &destination) {
            return error_response(500, error.to_string());
        }
        let _ = self
            .cfg
            .chown
            .chown(&destination, self.cfg.owner_uid.max(0) as u32, -1);
        json_response(200, &OkResponse::success())
    }

    fn handle_systemd_control(&self, request: &Request) -> Response {
        let req: SystemdControlRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        if req.commands.is_empty() {
            return error_response(400, "no commands specified");
        }
        for command in &req.commands {
            if let Err(message) = validate_systemctl_command(command) {
                return error_response(400, message);
            }
        }
        for command in &req.commands {
            let args: Vec<&str> = command.iter().map(String::as_str).collect();
            let output = self.run("systemctl", &args);
            if output.failed() {
                return error_response(500, cmd_error(&output));
            }
        }
        json_response(200, &OkResponse::success())
    }

    fn handle_systemd_save(&self, request: &Request) -> Response {
        let req: SystemdSaveRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        if req.unit_content.is_empty() {
            return error_response(400, "unit content is empty");
        }
        if let Err(message) = validate_systemd_unit_name(&req.unit_name) {
            return error_response(400, message);
        }
        let path = match validate_systemd_unit_file(&req.path, &self.cfg.managed_systemd_dirs) {
            Ok(path) => path,
            Err(message) => return error_response(400, message),
        };
        if targets_linux_helper(&req.unit_name) || targets_linux_helper(&path) {
            return error_response(403, "cannot modify privileged helper unit");
        }
        if path_basename(&path) != req.unit_name {
            return error_response(400, "unit path basename does not match unit name");
        }
        if let Err(error) = std::fs::create_dir_all(dir_path(&path)) {
            return error_response(500, error.to_string());
        }
        if let Err(error) = atomic_write_file(Path::new(&path), &req.unit_content, 0o644) {
            return error_response(500, error.to_string());
        }
        self.chown_root(&path);

        if req.reload {
            let output = self.run("systemctl", &["daemon-reload"]);
            if output.failed() {
                return error_response(500, cmd_error(&output));
            }
        }
        if req.enable {
            let output = self.run("systemctl", &["enable", &req.unit_name]);
            if output.failed() {
                return error_response(500, cmd_error(&output));
            }
        }
        if req.start {
            let output = self.run("systemctl", &["start", &req.unit_name]);
            if output.failed() {
                return error_response(500, cmd_error(&output));
            }
        }
        json_response(200, &OkResponse::success())
    }

    fn handle_systemd_remove(&self, request: &Request) -> Response {
        let req: SystemdRemoveRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        if let Err(message) = validate_systemd_unit_name(&req.unit_name) {
            return error_response(400, message);
        }
        let path = match validate_systemd_unit_file(&req.path, &self.cfg.managed_systemd_dirs) {
            Ok(path) => path,
            Err(message) => return error_response(400, message),
        };
        if targets_linux_helper(&req.unit_name) || targets_linux_helper(&path) {
            return error_response(403, "cannot remove privileged helper unit");
        }
        let trash_dir = match self.validate_trash_dir(&req.trash_dir) {
            Ok(dir) => dir,
            Err(message) => return error_response(400, message),
        };
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            _ => {
                return error_response(400, format!("unit {path} is not a regular file"));
            }
        }
        let destination = Path::new(&trash_dir).join(path_basename(&path));
        if !matches!(
            std::fs::symlink_metadata(&destination),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ) {
            return error_response(
                409,
                format!("destination {} already exists", destination.display()),
            );
        }
        let _ = self.run("systemctl", &["stop", &req.unit_name]);
        let _ = self.run("systemctl", &["disable", &req.unit_name]);
        if let Err(error) = move_file(Path::new(&path), &destination) {
            return error_response(500, error.to_string());
        }
        let _ = self
            .cfg
            .chown
            .chown(&destination, self.cfg.owner_uid.max(0) as u32, -1);
        let _ = self.run("systemctl", &["daemon-reload"]);
        json_response(200, &OkResponse::success())
    }

    fn ca_trust_target(&self) -> Result<CaTrustTarget, String> {
        match &self.cfg.ca_trust_target {
            Some(target) => Ok(target.clone()),
            None => detect_linux_ca_trust_target(),
        }
    }

    fn handle_ca_install(&self, request: &Request) -> Response {
        let req: CaInstallRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        let name = match validate_ca_name(&req.name) {
            Ok(name) => name,
            Err(message) => return error_response(400, message),
        };
        if let Err(message) = validate_ca_pem(&req.cert_pem) {
            return error_response(400, message);
        }
        let target = match self.ca_trust_target() {
            Ok(target) => target,
            Err(message) => return error_response(500, message),
        };
        let filename = if name.ends_with(&format!(".{}", target.extension)) {
            name
        } else {
            format!("{name}.{}", target.extension)
        };
        let anchor_path = target.anchor_dir.join(&filename);
        if let Err(error) = atomic_write_file(&anchor_path, &req.cert_pem, 0o644) {
            return error_response(500, error.to_string());
        }
        self.chown_root(&anchor_path.to_string_lossy());
        let args: Vec<&str> = target.update_args.iter().map(String::as_str).collect();
        let output = self.run(&target.update_program, &args);
        if output.failed() {
            return error_response(500, cmd_error(&output));
        }
        json_response(200, &OkResponse::success())
    }

    fn handle_ca_remove(&self, request: &Request) -> Response {
        let req: CaRemoveRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        let name = match validate_ca_name(&req.name) {
            Ok(name) => name,
            Err(message) => return error_response(400, message),
        };
        let target = match self.ca_trust_target() {
            Ok(target) => target,
            Err(message) => return error_response(500, message),
        };
        let filename = if name.ends_with(&format!(".{}", target.extension)) {
            name
        } else {
            format!("{name}.{}", target.extension)
        };
        let anchor_path = target.anchor_dir.join(&filename);
        match std::fs::remove_file(&anchor_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return error_response(500, error.to_string()),
        }
        let args: Vec<&str> = target.update_args.iter().map(String::as_str).collect();
        let output = self.run(&target.update_program, &args);
        if output.failed() {
            return error_response(500, cmd_error(&output));
        }
        json_response(200, &OkResponse::success())
    }

    fn handle_linux_proxy_socket_install(&self, request: &Request) -> Response {
        let req: LinuxProxySocketInstallRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        if let Err(message) =
            validate_linux_proxy_unit_names(&req.socket_unit_name, &req.service_unit_name)
        {
            return error_response(400, message);
        }
        if req.socket_content.is_empty() || req.service_content.is_empty() {
            return error_response(400, "proxy socket and service unit content are required");
        }
        let dir = &self.cfg.managed_systemd_dirs[0];
        let socket_path = Path::new(dir).join(&req.socket_unit_name);
        let service_path = Path::new(dir).join(&req.service_unit_name);
        let socket_str = socket_path.to_string_lossy().into_owned();
        let service_str = service_path.to_string_lossy().into_owned();
        if let Err(message) =
            validate_systemd_unit_file(&socket_str, &self.cfg.managed_systemd_dirs)
        {
            return error_response(400, message);
        }
        if let Err(message) =
            validate_systemd_unit_file(&service_str, &self.cfg.managed_systemd_dirs)
        {
            return error_response(400, message);
        }
        if let Err(error) = std::fs::create_dir_all(dir) {
            return error_response(500, error.to_string());
        }
        if let Err(error) = atomic_write_file(&socket_path, &req.socket_content, 0o644) {
            return error_response(500, error.to_string());
        }
        if let Err(error) = atomic_write_file(&service_path, &req.service_content, 0o644) {
            return error_response(500, error.to_string());
        }
        self.chown_root(&socket_str);
        self.chown_root(&service_str);

        let reload = self.run("systemctl", &["daemon-reload"]);
        if reload.failed() {
            return error_response(500, cmd_error(&reload));
        }
        let _ = self.run("systemctl", &["stop", &req.service_unit_name]);
        let _ = self.run("systemctl", &["stop", &req.socket_unit_name]);
        let enable = self.run("systemctl", &["enable", "--now", &req.socket_unit_name]);
        if enable.failed() {
            return error_response(500, cmd_error(&enable));
        }
        json_response(200, &OkResponse::success())
    }

    fn handle_linux_proxy_socket_stop(&self, request: &Request) -> Response {
        let req: LinuxProxySocketStopRequest = match decode_json(request) {
            Ok(req) => req,
            Err(response) => return response,
        };
        if let Err(message) =
            validate_linux_proxy_unit_names(&req.socket_unit_name, &req.service_unit_name)
        {
            return error_response(400, message);
        }
        let dir = &self.cfg.managed_systemd_dirs[0];
        let socket_path = Path::new(dir).join(&req.socket_unit_name);
        let service_path = Path::new(dir).join(&req.service_unit_name);
        let _ = self.run("systemctl", &["disable", "--now", &req.socket_unit_name]);
        let _ = self.run("systemctl", &["stop", &req.service_unit_name]);
        for path in [&socket_path, &service_path] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return error_response(500, error.to_string()),
            }
        }
        let _ = self.run("systemctl", &["daemon-reload"]);
        json_response(200, &OkResponse::success())
    }

    fn validate_trash_dir(&self, dir: &str) -> Result<String, String> {
        if !Path::new(dir).is_absolute() {
            return Err("trash directory must be absolute".to_string());
        }
        let clean = clean_path(dir);
        if clean != dir || clean == "/" {
            return Err(format!("invalid trash directory {dir:?}"));
        }
        let metadata =
            std::fs::symlink_metadata(&clean).map_err(|error| lstat_error(&clean, &error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "trash directory {clean:?} must be a real directory"
            ));
        }
        // SAFETY: geteuid has no preconditions.
        let euid = unsafe { libc::geteuid() };
        if euid == 0 && metadata.uid() != self.cfg.owner_uid.max(0) as u32 {
            return Err(format!("trash directory {clean:?} is not owned by caller"));
        }
        Ok(clean)
    }

    fn run(&self, program: &str, args: &[&str]) -> CommandOutput {
        self.cfg.runner.run(program, args)
    }

    fn chown_root(&self, path: &str) {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            return;
        }
        let _ = self.cfg.chown.chown(Path::new(path), 0, 0);
    }
}

/// The trimmed combined output, or the runner's error message when there is
/// none. This is Go's `cmdError`.
pub fn cmd_error(output: &CommandOutput) -> String {
    let text = String::from_utf8_lossy(&output.output).trim().to_string();
    if !text.is_empty() {
        return text;
    }
    output.error.clone().unwrap_or_default()
}

fn error_response(status: u16, message: impl Into<String>) -> Response {
    json_response(status, &ErrorResponse::new(message))
}

fn json_response<T: Serialize>(status: u16, value: &T) -> Response {
    match encode_server_json(value) {
        Ok(body) => Response::json(status, body),
        Err(error) => Response::text(500, &format!("{error}\n")),
    }
}

fn decode_json<T: DeserializeOwned>(request: &Request) -> Result<T, Response> {
    if request.body.len() > MAX_REQUEST_BYTES {
        return Err(error_response(400, "http: request body too large"));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&request.body);
    match T::deserialize(&mut deserializer) {
        Ok(value) => match deserializer.end() {
            Ok(()) => Ok(value),
            Err(_) => Err(error_response(400, "unexpected trailing JSON data")),
        },
        Err(error) => {
            if error.is_eof() {
                Err(error_response(400, "EOF"))
            } else {
                Err(error_response(400, error.to_string()))
            }
        }
    }
}

/// Writes `data` to a fresh `.devbase-tmp-*` file in the destination directory,
/// then renames it into place. This is Go's `atomicWriteFile`.
pub fn atomic_write_file(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut file = None;
    let mut temporary = PathBuf::new();
    for _ in 0..100 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        temporary = dir.join(format!(".devbase-tmp-{}-{n}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(created) => {
                file = Some(created);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let mut file = file.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a temporary file",
        )
    })?;
    let written = (|| {
        file.write_all(data)?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        Ok(())
    })();
    drop(file);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

/// Renames, falling back to a copy when source and destination are on different
/// filesystems. This is Go's `moveFile`.
pub fn move_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    match std::fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) => {
            if error.raw_os_error() != Some(libc::EXDEV) {
                return Err(error);
            }
            let data = std::fs::read(source)?;
            let mode = std::fs::symlink_metadata(source)
                .map(|metadata| metadata.permissions().mode() & 0o7777)
                .unwrap_or(0o644);
            atomic_write_file(destination, &data, mode)?;
            std::fs::remove_file(source)
        }
    }
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("owner_uid", &self.cfg.owner_uid)
            .field("idle_timeout", &self.cfg.idle_timeout)
            .finish_non_exhaustive()
    }
}
