//! The privileged helper client, ported from Go's `client.go`.
//!
//! Installing stages the whole app bundle with `ditto` (or a single binary with
//! `install`), writes the LaunchDaemon plist, bootstraps it, and waits for the
//! helper's `/v1/status`. The single administrator prompt comes from
//! [`super::admin`]. Status checks are passive: they read the plist and staged
//! executable without launching the helper.
//!
//! RPC is one HTTP request per operation over a pooled-by-nature Unix socket;
//! connections are retried on `ENOENT`/`ECONNREFUSED` with 15/30/60 ms
//! backoff while launchd starts the helper.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::admin::{AdminRunner, default_admin_runner};
use super::http;
use super::paths::{io_reason, shell_quote_path};
use super::plist_doc::{
    HelperPlistDoc, build_helper_plist, current_binary_info, expected_staged_executable,
    has_owner_arg, parse_helper_plist,
};
use super::protocol::{
    CaInstallRequest, CaRemoveRequest, ErrorResponse, LaunchdControlRequest,
    LinuxProxySocketInstallRequest, LinuxProxySocketStopRequest, PortInspectRequest,
    PortInspectResponse, ProxySocketInstallRequest, ProxySocketKickstartRequest,
    ProxySocketStopRequest, StatusResponse, SystemdControlRequest, SystemdRemoveRequest,
    SystemdSaveRequest, encode_request_json,
};
#[cfg(not(target_os = "macos"))]
use super::systemd_unit::{LINUX_HELPER_DIR, LINUX_HELPER_SERVICE_UNIT, LINUX_HELPER_SOCKET};
use super::systemd_unit::{
    build_helper_service_unit, build_helper_socket_unit, install_linux_helper_script,
    parse_linux_helper_units, uninstall_linux_helper_script,
};
#[cfg(target_os = "macos")]
use super::{HELPER_DIR, PLIST_PATH, SOCKET_PATH};
use super::{
    HelperError, HelperState, HelperStatus, LABEL, MAX_REQUEST_BYTES, PEER_REJECTED_MESSAGE,
};
use crate::launchd::{
    HelperApi, LaunchdError, LaunchdRemoveRequest as HostRemoveRequest,
    LaunchdSaveRequest as HostSaveRequest, LaunchdSaveResponse as HostSaveResponse,
    PrivilegedHelper,
};

/// The total time one RPC may take, matching Go's 30-second HTTP client.
pub const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
/// The default health-check window after install.
pub const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(8);
/// The default delay between health checks.
pub const DEFAULT_HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How long one health-check attempt may take.
pub const HEALTH_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
/// Connect retry backoff, matching `dialUnixWithRetry`.
pub const DIAL_BACKOFFS: [Duration; 3] = [
    Duration::from_millis(15),
    Duration::from_millis(30),
    Duration::from_millis(60),
];

/// Identifies the Devbase artifact the helper expects to match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BinaryInfo {
    pub executable: PathBuf,
    pub bundle_path: Option<PathBuf>,
    pub owner_uid: u32,
}

impl BinaryInfo {
    /// The artifact identity of the running process.
    pub fn current() -> Self {
        current_binary_info()
    }
}

/// Configures a helper RPC client. Defaults match `NewClient`.
#[derive(Clone)]
pub struct ClientConfig {
    pub socket_path: PathBuf,
    pub plist_path: PathBuf,
    pub helper_dir: PathBuf,
    pub expected: BinaryInfo,
    /// Runs install/uninstall as root after one administrator prompt.
    pub admin: Arc<dyn AdminRunner>,
    pub health_timeout: Duration,
    pub health_poll_interval: Duration,
}

fn default_socket_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(SOCKET_PATH)
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from(LINUX_HELPER_SOCKET)
    }
}

fn default_unit_or_plist_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(PLIST_PATH)
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from(LINUX_HELPER_SERVICE_UNIT)
    }
}

fn default_helper_staging_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(HELPER_DIR)
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from(LINUX_HELPER_DIR)
    }
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            socket_path: default_socket_path(),
            plist_path: default_unit_or_plist_path(),
            helper_dir: default_helper_staging_dir(),
            expected: BinaryInfo::current(),
            admin: default_admin_runner(),
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
            health_poll_interval: DEFAULT_HEALTH_POLL_INTERVAL,
        }
    }
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConfig")
            .field("socket_path", &self.socket_path)
            .field("plist_path", &self.plist_path)
            .field("helper_dir", &self.helper_dir)
            .field("expected", &self.expected)
            .field("admin", &self.admin)
            .field("health_timeout", &self.health_timeout)
            .field("health_poll_interval", &self.health_poll_interval)
            .finish()
    }
}

/// Static override installed by tests; restored on drop.
#[derive(Debug)]
#[must_use]
pub struct DefaultClientGuard {
    previous: Option<Arc<Client>>,
}

impl Drop for DefaultClientGuard {
    fn drop(&mut self) {
        if let Ok(mut slot) = OVERRIDE.lock() {
            *slot = self.previous.take();
        }
    }
}

static OVERRIDE: LazyLock<Mutex<Option<Arc<Client>>>> = LazyLock::new(|| Mutex::new(None));

/// Communicates with the launchd-managed privileged helper.
#[derive(Debug, Clone)]
pub struct Client {
    cfg: ClientConfig,
    disabled: bool,
}

impl Client {
    /// Creates a client, filling the path and timeout defaults.
    pub fn new(mut cfg: ClientConfig) -> Self {
        if cfg.socket_path.as_os_str().is_empty() {
            cfg.socket_path = default_socket_path();
        }
        if cfg.plist_path.as_os_str().is_empty() {
            cfg.plist_path = default_unit_or_plist_path();
        }
        if cfg.helper_dir.as_os_str().is_empty() {
            cfg.helper_dir = default_helper_staging_dir();
        }
        if cfg.health_timeout.is_zero() {
            cfg.health_timeout = DEFAULT_HEALTH_TIMEOUT;
        }
        if cfg.health_poll_interval.is_zero() {
            cfg.health_poll_interval = DEFAULT_HEALTH_POLL_INTERVAL;
        }
        Client {
            cfg,
            disabled: false,
        }
    }

    /// A client that refuses every operation, used in isolated test runs.
    pub fn disabled() -> Self {
        Client::disabled_with(ClientConfig::default())
    }

    /// A disabled client with an explicit configuration, for tests.
    pub fn disabled_with(cfg: ClientConfig) -> Self {
        Client {
            cfg,
            disabled: true,
        }
    }

    pub fn config(&self) -> &ClientConfig {
        &self.cfg
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// The default client for this process.
    ///
    /// Tests and isolated `MSM_DEV_HOME` runs without an explicit socket get a
    /// disabled client, so they never touch the host's installed helper.
    pub fn default_client() -> Arc<Client> {
        if let Ok(slot) = OVERRIDE.lock()
            && let Some(client) = slot.as_ref()
        {
            return Arc::clone(client);
        }
        if cfg!(test) || dev_home_without_socket() {
            return Arc::new(Client::disabled());
        }
        let socket = std::env::var_os("MSM_PRIVHELPER_SOCKET")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(default_socket_path);
        Arc::new(Client::new(ClientConfig {
            socket_path: socket,
            ..ClientConfig::default()
        }))
    }

    /// Overrides [`Client::default_client`] for a test, restoring on drop.
    ///
    /// Tests that use this must not run concurrently with another default
    /// client test; the crate's helper tests keep this to one test.
    pub fn set_default_for_test(client: Arc<Client>) -> DefaultClientGuard {
        let mut slot = OVERRIDE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = slot.replace(client);
        DefaultClientGuard { previous }
    }

    fn uses_systemd_units(&self) -> bool {
        self.cfg.plist_path.extension() == Some(std::ffi::OsStr::new("service"))
    }

    fn linux_socket_unit_path(&self) -> PathBuf {
        self.cfg.plist_path.with_extension("socket")
    }

    fn status_systemd(&self) -> HelperStatus {
        let service_raw = match std::fs::read_to_string(&self.cfg.plist_path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return HelperStatus {
                    state: HelperState::NotInstalled,
                    message: "Not installed. Root operations need it; installing asks for your administrator password once."
                        .to_string(),
                };
            }
            Err(error) => {
                return HelperStatus {
                    state: HelperState::NotInstalled,
                    message: format!(
                        "Could not inspect helper configuration: open {}: {}",
                        self.cfg.plist_path.display(),
                        io_reason(&error)
                    ),
                };
            }
        };
        let socket_unit = self.linux_socket_unit_path();
        let socket_raw = match std::fs::read_to_string(&socket_unit) {
            Ok(raw) => raw,
            Err(_) => {
                return HelperStatus {
                    state: HelperState::NotInstalled,
                    message: "Installed helper socket unit is missing and needs to be reinstalled."
                        .to_string(),
                };
            }
        };
        let doc = parse_linux_helper_units(&service_raw, &socket_raw);
        let staged = expected_staged_executable(&self.cfg.helper_dir, &self.cfg.expected);
        if doc.exec_start.len() < 4 || doc.exec_start[0] != path_text(&staged) {
            return HelperStatus {
                state: HelperState::NotInstalled,
                message:
                    "Installed helper points to a different location and needs to be reinstalled."
                        .to_string(),
            };
        }
        if !has_owner_arg(&doc.exec_start, self.cfg.expected.owner_uid) {
            return HelperStatus {
                state: HelperState::NotInstalled,
                message: "Installed helper is configured for a different user and needs to be reinstalled."
                    .to_string(),
            };
        }
        if doc.listen_stream != path_text(&self.cfg.socket_path) {
            return HelperStatus {
                state: HelperState::NotInstalled,
                message:
                    "Installed helper socket configuration is invalid and needs to be reinstalled."
                        .to_string(),
            };
        }
        if std::fs::metadata(&staged).is_err() {
            return HelperStatus {
                state: HelperState::NotInstalled,
                message: "Installed helper binary is missing and needs to be reinstalled."
                    .to_string(),
            };
        }
        HelperStatus {
            state: HelperState::Installed,
            message: "Installed. Root operations run through it without password prompts."
                .to_string(),
        }
    }

    /// Passively inspects the installed helper configuration and staged artifact.
    pub fn status(&self) -> HelperStatus {
        if self.disabled {
            return HelperStatus {
                state: HelperState::NotInstalled,
                message: "Privileged helper is disabled in this environment.".to_string(),
            };
        }
        if self.uses_systemd_units() {
            return self.status_systemd();
        }
        let raw = match std::fs::read(&self.cfg.plist_path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return HelperStatus {
                    state: HelperState::NotInstalled,
                    message: "Not installed. Root operations need it; installing asks for your administrator password once."
                        .to_string(),
                };
            }
            Err(error) => {
                return HelperStatus {
                    state: HelperState::NotInstalled,
                    message: format!(
                        "Could not inspect helper configuration: open {}: {}",
                        self.cfg.plist_path.display(),
                        io_reason(&error)
                    ),
                };
            }
        };
        let doc: HelperPlistDoc = match parse_helper_plist(&raw) {
            Ok(doc) => doc,
            Err(_) => {
                return HelperStatus {
                    state: HelperState::NotInstalled,
                    message:
                        "Installed helper configuration is invalid and needs to be reinstalled."
                            .to_string(),
                };
            }
        };
        let staged = expected_staged_executable(&self.cfg.helper_dir, &self.cfg.expected);
        if doc.label != LABEL
            || doc.program_arguments.len() < 4
            || doc.program_arguments[0] != path_text(&staged)
        {
            return HelperStatus {
                state: HelperState::NotInstalled,
                message:
                    "Installed helper points to a different location and needs to be reinstalled."
                        .to_string(),
            };
        }
        if !has_owner_arg(&doc.program_arguments, self.cfg.expected.owner_uid) {
            return HelperStatus {
                state: HelperState::NotInstalled,
                message: "Installed helper is configured for a different user and needs to be reinstalled."
                    .to_string(),
            };
        }
        match doc.sockets.get(super::SOCKET_NAME) {
            Some(socket) if socket.sock_path_name == path_text(&self.cfg.socket_path) => {}
            _ => {
                return HelperStatus {
                    state: HelperState::NotInstalled,
                    message:
                        "Installed helper socket configuration is invalid and needs to be reinstalled."
                            .to_string(),
                };
            }
        }
        if std::fs::metadata(&staged).is_err() {
            return HelperStatus {
                state: HelperState::NotInstalled,
                message: "Installed helper binary is missing and needs to be reinstalled."
                    .to_string(),
            };
        }
        HelperStatus {
            state: HelperState::Installed,
            message: "Installed. Root operations run through it without password prompts."
                .to_string(),
        }
    }

    /// Whether the helper is installed and matches this app.
    pub fn ready(&self) -> bool {
        !self.disabled && self.status().state == HelperState::Installed
    }

    /// The default client when it is ready, or [`HelperError::NotInstalled`].
    pub fn require() -> Result<Arc<Client>, HelperError> {
        let client = Client::default_client();
        if !client.ready() {
            return Err(HelperError::NotInstalled);
        }
        Ok(client)
    }

    /// Stages the helper, installs its launchd job through one administrator
    /// prompt, and verifies connectivity.
    pub fn install(&self) -> Result<(), HelperError> {
        if self.disabled {
            return Err(HelperError::Message(
                "privileged helper is disabled".to_string(),
            ));
        }
        let expected = &self.cfg.expected;
        if expected.executable.as_os_str().is_empty() {
            return Err(HelperError::Message(
                "could not locate running Devbase executable".to_string(),
            ));
        }
        let staged = expected_staged_executable(&self.cfg.helper_dir, expected);
        if self.uses_systemd_units() {
            let service_unit = build_helper_service_unit(&staged, expected.owner_uid)
                .map_err(HelperError::Message)?;
            let socket_unit = build_helper_socket_unit(&path_text(&self.cfg.socket_path));
            let socket_unit_path = self.linux_socket_unit_path();
            let tmp_dir = install_temp_dir();
            let result = (|| {
                let tmp_socket = tmp_dir.join("devbase-helper.socket");
                let tmp_service = tmp_dir.join("devbase-helper.service");
                std::fs::write(&tmp_socket, socket_unit.as_bytes()).map_err(io_helper_error)?;
                std::fs::write(&tmp_service, service_unit.as_bytes()).map_err(io_helper_error)?;
                let script = install_linux_helper_script(
                    &expected.executable,
                    &self.cfg.helper_dir,
                    &staged,
                    &socket_unit_path,
                    &self.cfg.plist_path,
                    &tmp_socket,
                    &tmp_service,
                    &self.cfg.socket_path,
                );
                let (output, error) = self.cfg.admin.run_admin("/bin/sh", &["-c", &script]);
                if let Some(error) = error {
                    let message = output.trim();
                    if !message.is_empty() {
                        return Err(HelperError::Message(format!(
                            "helper install failed: {message}: {error}"
                        )));
                    }
                    return Err(HelperError::Message(error.to_string()));
                }
                self.wait_for_healthy()
            })();
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return result;
        }
        let plist = build_helper_plist(
            &staged,
            expected.owner_uid,
            &path_text(&self.cfg.socket_path),
        )
        .map_err(HelperError::Message)?;
        let tmp_dir = install_temp_dir();
        let result = (|| {
            let tmp_plist = tmp_dir.join(format!("{LABEL}.plist"));
            std::fs::write(&tmp_plist, &plist).map_err(io_helper_error)?;
            let script = install_helper_script(
                expected,
                &self.cfg.helper_dir,
                &self.cfg.plist_path,
                &self.cfg.socket_path,
                &tmp_plist,
                &staged,
            );
            let (output, error) = self.cfg.admin.run_admin("/bin/sh", &["-c", &script]);
            if let Some(error) = error {
                let message = output.trim();
                if !message.is_empty() {
                    return Err(HelperError::Message(format!(
                        "helper install failed: {message}: {error}"
                    )));
                }
                return Err(HelperError::Message(error.to_string()));
            }
            self.wait_for_healthy()
        })();
        let _ = std::fs::remove_dir_all(&tmp_dir);
        result
    }

    fn wait_for_healthy(&self) -> Result<(), HelperError> {
        let deadline = Instant::now() + self.cfg.health_timeout;
        let mut last_error: Option<HelperError> = None;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let attempt_deadline = (now + HEALTH_ATTEMPT_TIMEOUT).min(deadline);
            match self.ping_until(attempt_deadline) {
                Ok(response) => {
                    if response.owner_uid != i64::from(self.cfg.expected.owner_uid) {
                        return Err(HelperError::Message(format!(
                            "helper owner UID mismatch: got {}, want {}",
                            response.owner_uid, self.cfg.expected.owner_uid
                        )));
                    }
                    return Ok(());
                }
                Err(error) => {
                    let text = error.to_string();
                    if text.contains("not authorized") || text.contains("reinstall the helper") {
                        return Err(HelperError::Message(format!(
                            "helper installed, but health check failed: {error}"
                        )));
                    }
                    last_error = Some(error);
                }
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            std::thread::sleep(self.cfg.health_poll_interval.min(deadline - now));
        }
        let last = last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "context deadline exceeded".to_string());
        Err(HelperError::Message(format!(
            "helper installed, but health check failed: {last}"
        )))
    }

    /// Stops and removes the helper launchd job, plist, staged files and socket.
    pub fn uninstall(&self) -> Result<(), HelperError> {
        if self.disabled {
            return Err(HelperError::Message(
                "privileged helper is disabled".to_string(),
            ));
        }
        let script = if self.uses_systemd_units() {
            uninstall_linux_helper_script(
                &self.linux_socket_unit_path(),
                &self.cfg.plist_path,
                &self.cfg.helper_dir,
                &self.cfg.socket_path,
            )
        } else {
            uninstall_helper_script(
                &self.cfg.plist_path,
                &self.cfg.helper_dir,
                &self.cfg.socket_path,
            )
        };
        let (output, error) = self.cfg.admin.run_admin("/bin/sh", &["-c", &script]);
        if let Some(error) = error {
            let message = output.trim();
            if !message.is_empty() {
                return Err(HelperError::Message(format!(
                    "helper uninstall failed: {message}: {error}"
                )));
            }
            return Err(HelperError::Message(error.to_string()));
        }
        Ok(())
    }

    /// `GET /v1/status`.
    pub fn ping(&self) -> Result<StatusResponse, HelperError> {
        self.ping_until(Instant::now() + OPERATION_TIMEOUT)
    }

    fn ping_until(&self, deadline: Instant) -> Result<StatusResponse, HelperError> {
        let body = self.request("GET", "/v1/status", None, deadline)?;
        serde_json::from_slice(&body).map_err(|error| HelperError::Message(error.to_string()))
    }

    /// Writes the proxy socket daemon plist and bootstraps it.
    pub fn install_proxy_socket(
        &self,
        request: &ProxySocketInstallRequest,
    ) -> Result<(), HelperError> {
        self.post_ok("/v1/proxy-socket/install", request)
    }

    /// Boots out and removes the proxy socket daemon plist.
    pub fn stop_proxy_socket(&self, request: &ProxySocketStopRequest) -> Result<(), HelperError> {
        self.post_ok("/v1/proxy-socket/stop", request)
    }

    /// Restarts the proxy socket daemon.
    pub fn kickstart_proxy_socket(&self, target: &str) -> Result<(), HelperError> {
        self.post_ok(
            "/v1/proxy-socket/kickstart",
            &ProxySocketKickstartRequest {
                target: target.to_string(),
            },
        )
    }

    /// Runs `lsof` as root for one TCP listening port.
    pub fn inspect_port(&self, port: i64) -> Result<Vec<u8>, HelperError> {
        let body = self.post("/v1/ports/inspect", &PortInspectRequest { port })?;
        let response: PortInspectResponse = serde_json::from_slice(&body)
            .map_err(|error| HelperError::Message(error.to_string()))?;
        Ok(response.output)
    }

    /// Runs validated `launchctl` commands as root.
    pub fn launchd_control(&self, commands: &[Vec<String>]) -> Result<(), HelperError> {
        self.post_ok(
            "/v1/launchd/control",
            &LaunchdControlRequest {
                commands: commands.to_vec(),
            },
        )
    }

    /// Saves and optionally bootstraps a system plist.
    pub fn launchd_save(
        &self,
        request: &super::protocol::LaunchdSaveRequest,
    ) -> Result<super::protocol::LaunchdSaveResponse, HelperError> {
        let body = self.post("/v1/launchd/save", request)?;
        serde_json::from_slice(&body).map_err(|error| HelperError::Message(error.to_string()))
    }

    /// Boots out a system job and moves its plist into `trash_dir`.
    pub fn launchd_remove(
        &self,
        request: &super::protocol::LaunchdRemoveRequest,
    ) -> Result<(), HelperError> {
        self.post_ok("/v1/launchd/remove", request)
    }

    /// Runs validated `systemctl` commands as root on Linux.
    pub fn systemd_control(&self, commands: &[Vec<String>]) -> Result<(), HelperError> {
        self.post_ok(
            "/v1/systemd/control",
            &SystemdControlRequest {
                commands: commands.to_vec(),
            },
        )
    }

    /// Writes a system unit file and optionally reloads/enables/starts it on Linux.
    pub fn systemd_save(&self, request: &SystemdSaveRequest) -> Result<(), HelperError> {
        self.post_ok("/v1/systemd/save", request)
    }

    /// Stops/disables a system unit and moves its file into `trash_dir` on Linux.
    pub fn systemd_remove(&self, request: &SystemdRemoveRequest) -> Result<(), HelperError> {
        self.post_ok("/v1/systemd/remove", request)
    }

    /// Installs a PEM root CA anchor into the Linux system trust store.
    pub fn ca_install(&self, request: &CaInstallRequest) -> Result<(), HelperError> {
        self.post_ok("/v1/ca/install", request)
    }

    /// Removes a root CA anchor from the Linux system trust store.
    pub fn ca_remove(&self, request: &CaRemoveRequest) -> Result<(), HelperError> {
        self.post_ok("/v1/ca/remove", request)
    }

    /// Installs and starts Linux systemd proxy socket-activation units for ports `< 1024`.
    pub fn linux_proxy_socket_install(
        &self,
        request: &LinuxProxySocketInstallRequest,
    ) -> Result<(), HelperError> {
        self.post_ok("/v1/systemd/proxy-socket/install", request)
    }

    /// Stops and removes Linux systemd proxy socket-activation units.
    pub fn linux_proxy_socket_stop(
        &self,
        request: &LinuxProxySocketStopRequest,
    ) -> Result<(), HelperError> {
        self.post_ok("/v1/systemd/proxy-socket/stop", request)
    }

    fn post<Request: Serialize>(
        &self,
        path: &str,
        request: &Request,
    ) -> Result<Vec<u8>, HelperError> {
        let body = encode_request_json(request).map_err(HelperError::Message)?;
        self.request(
            "POST",
            path,
            Some(&body),
            Instant::now() + OPERATION_TIMEOUT,
        )
    }

    fn post_ok<Request: Serialize>(
        &self,
        path: &str,
        request: &Request,
    ) -> Result<(), HelperError> {
        self.post(path, request).map(|_| ())
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        deadline: Instant,
    ) -> Result<Vec<u8>, HelperError> {
        if self.disabled {
            return Err(HelperError::Disabled);
        }
        let mut stream = self.connect(deadline)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(HelperError::Message(format!(
                "helper {method} {path} timed out"
            )));
        }
        http::set_timeouts(&stream, remaining).map_err(io_helper_error)?;
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: unix\r\nConnection: close\r\n");
        if let Some(body) = body {
            head.push_str("Content-Type: application/json\r\n");
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).map_err(io_helper_error)?;
        if let Some(body) = body {
            stream.write_all(body).map_err(io_helper_error)?;
        }
        stream.flush().map_err(io_helper_error)?;

        let raw = http::read_response(&mut stream, MAX_REQUEST_BYTES).map_err(io_helper_error)?;
        let parsed = http::parse_response(&raw).map_err(HelperError::Message)?;
        if parsed.status >= 400 {
            if let Ok(error) = serde_json::from_slice::<ErrorResponse>(&parsed.body)
                && !error.error.is_empty()
            {
                if error.error == PEER_REJECTED_MESSAGE {
                    return Err(HelperError::PeerRejected);
                }
                return Err(HelperError::Message(error.error));
            }
            return Err(HelperError::Message(format!(
                "helper {method} {path} failed with HTTP {}",
                parsed.status
            )));
        }
        Ok(parsed.body)
    }

    fn connect(&self, deadline: Instant) -> Result<std::os::unix::net::UnixStream, HelperError> {
        let mut attempt = 0;
        loop {
            match std::os::unix::net::UnixStream::connect(&self.cfg.socket_path) {
                Ok(stream) => return Ok(stream),
                Err(error) => {
                    let message = format!(
                        "dial unix {}: connect: {}",
                        self.cfg.socket_path.display(),
                        io_reason(&error)
                    );
                    let retryable = matches!(
                        error.raw_os_error(),
                        Some(libc::ENOENT) | Some(libc::ECONNREFUSED)
                    );
                    if !retryable || attempt >= DIAL_BACKOFFS.len() || Instant::now() >= deadline {
                        return Err(HelperError::Message(message));
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(HelperError::Message(message));
                    }
                    std::thread::sleep(DIAL_BACKOFFS[attempt].min(remaining));
                    attempt += 1;
                }
            }
        }
    }
}

fn dev_home_without_socket() -> bool {
    std::env::var_os("MSM_DEV_HOME").is_some_and(|value| !value.is_empty())
        && std::env::var_os("MSM_PRIVHELPER_SOCKET").is_none_or(|value| value.is_empty())
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn io_helper_error(error: std::io::Error) -> HelperError {
    HelperError::Message(error.to_string())
}

fn install_temp_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    for _ in 0..100 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = base.join(format!("devbase-helper-install-{}-{n}", std::process::id()));
        match std::fs::create_dir(&candidate) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &candidate,
                        std::fs::Permissions::from_mode(0o700),
                    );
                }
                return candidate;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return candidate,
        }
    }
    base.join(format!("devbase-helper-install-{}", std::process::id()))
}

/// The single shell script install runs as root, `; `-joined exactly as Go
/// builds it.
pub fn install_helper_script(
    info: &BinaryInfo,
    helper_dir: &Path,
    plist_path: &Path,
    socket_path: &Path,
    tmp_plist: &Path,
    staged_exe: &Path,
) -> String {
    let parent = helper_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let quoted_socket = shell_quote_path(socket_path);
    let mut lines = vec![
        "set -eu".to_string(),
        format!(
            "/bin/launchctl bootout {} >/dev/null 2>&1 || true",
            shell_quote_path(Path::new(&format!("system/{LABEL}")))
        ),
        format!("/bin/rm -f {quoted_socket}"),
        format!("/bin/mkdir -p {}", shell_quote_path(parent)),
        format!("/usr/sbin/chown root:wheel {}", shell_quote_path(parent)),
        format!("/bin/chmod 0755 {}", shell_quote_path(parent)),
        format!("/bin/rm -rf {}", shell_quote_path(helper_dir)),
        format!("/bin/mkdir -p {}", shell_quote_path(helper_dir)),
    ];
    if let Some(bundle) = &info.bundle_path {
        let destination = helper_dir.join("bundle");
        lines.push(format!(
            "/usr/bin/ditto --noqtn {} {}",
            shell_quote_path(bundle),
            shell_quote_path(&destination)
        ));
        lines.push(format!(
            "/usr/bin/codesign --verify --deep --strict {}",
            shell_quote_path(&destination)
        ));
    } else {
        lines.push(format!(
            "/usr/bin/install -m 0755 {} {}",
            shell_quote_path(&info.executable),
            shell_quote_path(staged_exe)
        ));
    }
    lines.push(format!(
        "/usr/sbin/chown -R root:wheel {}",
        shell_quote_path(helper_dir)
    ));
    lines.push(format!("/bin/chmod 0755 {}", shell_quote_path(helper_dir)));
    lines.push(format!(
        "/usr/bin/install -m 0644 {} {}",
        shell_quote_path(tmp_plist),
        shell_quote_path(plist_path)
    ));
    lines.push(format!(
        "/usr/sbin/chown root:wheel {}",
        shell_quote_path(plist_path)
    ));
    lines.push(format!(
        "/bin/launchctl bootstrap system {}",
        shell_quote_path(plist_path)
    ));
    lines.push(format!(
        "i=0; while [ ! -S {quoted_socket} ] && [ \"$i\" -lt 50 ]; do /bin/sleep 0.1; i=$((i + 1)); done"
    ));
    lines.join("; ")
}

/// The uninstall script: boot out the job, then remove plist, staged files and
/// socket. There is no health check afterwards.
pub fn uninstall_helper_script(plist_path: &Path, helper_dir: &Path, socket_path: &Path) -> String {
    [
        "set -eu".to_string(),
        format!(
            "/bin/launchctl bootout {} >/dev/null 2>&1 || true",
            shell_quote_path(Path::new(&format!("system/{LABEL}")))
        ),
        format!("/bin/rm -f {}", shell_quote_path(plist_path)),
        format!("/bin/rm -rf {}", shell_quote_path(helper_dir)),
        format!("/bin/rm -f {}", shell_quote_path(socket_path)),
    ]
    .join("; ")
}

fn to_launchd_error(error: HelperError) -> LaunchdError {
    match error {
        HelperError::NotInstalled => LaunchdError::helper_not_installed(),
        HelperError::PeerRejected => LaunchdError::helper(PEER_REJECTED_MESSAGE),
        other => LaunchdError::helper(other.to_string()),
    }
}

fn host_path(path: &Path) -> Result<String, HelperError> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| HelperError::Message(format!("path {} is not valid UTF-8", path.display())))
}

/// The launchd seam: every root launchd operation goes through this client.
impl HelperApi for Client {
    fn launchd_control(&self, commands: &[Vec<String>]) -> Result<(), LaunchdError> {
        Client::launchd_control(self, commands).map_err(to_launchd_error)
    }

    fn launchd_save(&self, request: HostSaveRequest) -> Result<HostSaveResponse, LaunchdError> {
        let protocol = super::protocol::LaunchdSaveRequest {
            plist: request.plist,
            path: host_path(&request.path).map_err(to_launchd_error)?,
            domain: request.domain,
            target: request.target,
            existing_path: request
                .existing_path
                .as_deref()
                .map(host_path)
                .transpose()
                .map_err(to_launchd_error)?
                .unwrap_or_default(),
            load: request.load,
        };
        let response = Client::launchd_save(self, &protocol).map_err(to_launchd_error)?;
        Ok(HostSaveResponse {
            saved: response.saved,
            error: (!response.error.is_empty()).then_some(response.error),
        })
    }

    fn launchd_remove(&self, request: HostRemoveRequest) -> Result<(), LaunchdError> {
        let protocol = super::protocol::LaunchdRemoveRequest {
            path: host_path(&request.path).map_err(to_launchd_error)?,
            target: request.target,
            trash_dir: host_path(&request.trash_dir).map_err(to_launchd_error)?,
        };
        Client::launchd_remove(self, &protocol).map_err(to_launchd_error)
    }
}

/// The app's handle on the installed helper.
impl PrivilegedHelper for Client {
    fn require(&self) -> Result<Arc<dyn HelperApi>, LaunchdError> {
        if !self.ready() {
            return Err(LaunchdError::helper_not_installed());
        }
        Ok(Arc::new(self.clone()))
    }
}

/// A [`PrivilegedHelper`] over [`Client::default_client`].
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultHelper;

impl PrivilegedHelper for DefaultHelper {
    fn require(&self) -> Result<Arc<dyn HelperApi>, LaunchdError> {
        Client::default_client().require()
    }
}
