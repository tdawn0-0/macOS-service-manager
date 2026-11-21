//! Proxy apply/start/stop/restart orchestration.
//!
//! Ported from `startProxy`/`ApplyProxy`/`RestartProxy`/`StopProxy`. Every
//! operating-system boundary is a trait: launchd jobs go through devcore's
//! [`Engine`], ports through [`PortProbe`], TLS verification through
//! [`TlsVerifier`], readiness through [`HealthProbe`], and the privileged
//! socket daemon through [`SocketInstaller`] (Phase 4).

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use devcore::engine::{Engine, EngineError, JobSpec, ProcessRunnerAdapter, label_prefix};
use devcore::helper::systemd_unit::{build_linux_proxy_socket_units, linux_proxy_unit_stem};
use devcore::helper::{
    Client as HelperClient, HelperError, LinuxProxySocketInstallRequest,
    LinuxProxySocketStopRequest,
};
use devcore::launchd::ProcessRunner;
use devcore::state::{ResourceKey, StateError, Store};

use crate::ca::{AuthorityService, CaError};
use crate::tunnel::config::{TunnelConfigError, published_config_exists, write_config_from_state};

use super::rules::{
    ProxyRoute, PublishedProxyConfig, listener_ports, privileged_ports, read_published_config,
    routes_from_state, validate_proxy_routes, write_published_config,
};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const TLS_VERIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// Errors from proxy lifecycle operations.
#[derive(Debug, thiserror::Error)]
pub enum ProxyLifecycleError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Rules(#[from] super::rules::RuleError),
    #[error(transparent)]
    Ca(#[from] CaError),
    #[error(transparent)]
    Tunnel(#[from] TunnelConfigError),
    #[error("{0}")]
    Invalid(String),
    #[error(
        "HTTPS port {port} is unavailable; inspect the listener or choose another local port: {detail}"
    )]
    PortUnavailable { port: i64, detail: String },
    #[error("{0}")]
    Unsupported(String),
}

/// Bindability and liveness checks for listener ports.
pub trait PortProbe: Send + Sync {
    fn available(&self, port: i64) -> bool;
    fn answering(&self, port: i64) -> bool;
}

/// The real probe: bind 127.0.0.1, then try a short TCP connect.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemPortProbe;

impl PortProbe for SystemPortProbe {
    fn available(&self, port: i64) -> bool {
        std::net::TcpListener::bind(("127.0.0.1", port as u16)).is_ok()
    }

    fn answering(&self, port: i64) -> bool {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port as u16));
        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
    }
}

/// Wait for a listener to accept connections.
pub trait HealthProbe: Send + Sync {
    fn wait_tcp(&self, port: i64) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemHealthProbe;

impl HealthProbe for SystemHealthProbe {
    fn wait_tcp(&self, port: i64) -> Result<(), String> {
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port as u16));
        loop {
            if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("HTTPS proxy on port {port} did not become ready"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Verify a trusted TLS handshake using only the local CA.
pub trait TlsVerifier: Send + Sync {
    fn verify(&self, domain: &str, port: i64) -> Result<(), String>;
}

/// Production verifier: one-second attempts over a 20-second budget.
pub struct SystemTlsVerifier {
    ca_der: Vec<u8>,
}

impl SystemTlsVerifier {
    pub fn new(ca_der: Vec<u8>) -> Self {
        SystemTlsVerifier { ca_der }
    }
}

impl TlsVerifier for SystemTlsVerifier {
    fn verify(&self, domain: &str, port: i64) -> Result<(), String> {
        crate::ensure_crypto_provider();
        use rustls::pki_types::{CertificateDer, ServerName};
        let deadline = Instant::now() + TLS_VERIFY_TIMEOUT;
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from(self.ca_der.clone()))
            .map_err(|error| error.to_string())?;
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let config = Arc::new(config);
        loop {
            let attempt = (|| -> Result<(), String> {
                let server_name = ServerName::try_from(domain.to_string())
                    .map_err(|_| "invalid server name".to_string())?;
                let connection = rustls::ClientConnection::new(Arc::clone(&config), server_name)
                    .map_err(|error| error.to_string())?;
                let address = std::net::SocketAddr::from(([127, 0, 0, 1], port as u16));
                let tcp =
                    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500))
                        .map_err(|error| error.to_string())?;
                tcp.set_read_timeout(Some(Duration::from_secs(1)))
                    .map_err(|error| error.to_string())?;
                let mut stream = rustls::StreamOwned::new(connection, tcp);
                use std::io::Write;
                stream
                    .write_all(b"HEAD / HTTP/1.0\r\n\r\n")
                    .and_then(|()| stream.flush())
                    .map_err(|error| error.to_string())
            })();
            if attempt.is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "HTTPS proxy {domain}:{port} did not complete a trusted TLS handshake"
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Verifies that a job/target is gone before switching authorities or modes.
pub trait ShutdownVerifier: Send + Sync {
    fn verify_stopped(&self, target: &str) -> Result<(), String>;
}

/// The real verifier backed by [`Engine::verify_stopped`] (launchd on macOS,
/// systemd user units on Linux).
#[derive(Clone)]
pub struct EngineShutdownVerifier {
    engine: Engine,
}

impl EngineShutdownVerifier {
    pub fn new(engine: Engine) -> Self {
        EngineShutdownVerifier { engine }
    }

    fn resolve_job_id<'a>(&self, target: &'a str) -> &'a str {
        let prefix = format!(
            "{}/xyz.jyeu.devbase.dev.{}.",
            self.engine.domain(),
            label_prefix(self.engine.root())
        );
        target.strip_prefix(&prefix).unwrap_or(target)
    }
}

impl ShutdownVerifier for EngineShutdownVerifier {
    fn verify_stopped(&self, target: &str) -> Result<(), String> {
        let id = self.resolve_job_id(target);
        match self.engine.verify_stopped(id) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("{target} is still running")),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// The privileged socket-daemon boundary (launchd daemon on macOS, systemd
/// socket activation on Linux).
pub trait SocketInstaller: Send + Sync {
    fn loaded(&self) -> bool;
    fn install(&self, sites: &[ProxyRoute], executable: &str) -> Result<(), ProxyLifecycleError>;
    fn stop(&self) -> Result<(), ProxyLifecycleError>;
    fn kickstart(&self) -> Result<(), ProxyLifecycleError>;
}

/// Until the Phase 4 macOS helper lands, privileged ports stop at this seam.
#[derive(Debug, Clone, Copy, Default)]
pub struct Phase4SocketInstaller;

impl SocketInstaller for Phase4SocketInstaller {
    fn loaded(&self) -> bool {
        false
    }

    fn install(&self, _sites: &[ProxyRoute], _executable: &str) -> Result<(), ProxyLifecycleError> {
        Err(ProxyLifecycleError::Unsupported(
            "privileged HTTPS ports require the Phase 4 socket daemon helper".into(),
        ))
    }

    fn stop(&self) -> Result<(), ProxyLifecycleError> {
        Ok(())
    }

    fn kickstart(&self) -> Result<(), ProxyLifecycleError> {
        Err(ProxyLifecycleError::Unsupported(
            "privileged HTTPS ports require the Phase 4 socket daemon helper".into(),
        ))
    }
}

/// Helper client operations required by [`SystemdSocketInstaller`].
pub trait LinuxProxySocketHelper: Send + Sync {
    fn linux_proxy_socket_install(
        &self,
        request: &LinuxProxySocketInstallRequest,
    ) -> Result<(), HelperError>;
    fn linux_proxy_socket_stop(
        &self,
        request: &LinuxProxySocketStopRequest,
    ) -> Result<(), HelperError>;
    fn systemd_control(&self, commands: &[Vec<String>]) -> Result<(), HelperError>;
}

impl LinuxProxySocketHelper for HelperClient {
    fn linux_proxy_socket_install(
        &self,
        request: &LinuxProxySocketInstallRequest,
    ) -> Result<(), HelperError> {
        HelperClient::linux_proxy_socket_install(self, request)
    }

    fn linux_proxy_socket_stop(
        &self,
        request: &LinuxProxySocketStopRequest,
    ) -> Result<(), HelperError> {
        HelperClient::linux_proxy_socket_stop(self, request)
    }

    fn systemd_control(&self, commands: &[Vec<String>]) -> Result<(), HelperError> {
        HelperClient::systemd_control(self, commands)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DefaultLinuxProxySocketHelper;

impl LinuxProxySocketHelper for DefaultLinuxProxySocketHelper {
    fn linux_proxy_socket_install(
        &self,
        request: &LinuxProxySocketInstallRequest,
    ) -> Result<(), HelperError> {
        let client = HelperClient::require()?;
        client.linux_proxy_socket_install(request)
    }

    fn linux_proxy_socket_stop(
        &self,
        request: &LinuxProxySocketStopRequest,
    ) -> Result<(), HelperError> {
        let client = HelperClient::require()?;
        client.linux_proxy_socket_stop(request)
    }

    fn systemd_control(&self, commands: &[Vec<String>]) -> Result<(), HelperError> {
        let client = HelperClient::require()?;
        client.systemd_control(commands)
    }
}

/// Linux systemd socket-activation installer for privileged HTTPS ports
/// (`< 1024`), delegating root unit management to
/// [`devcore::helper::Client::linux_proxy_socket_install`] and
/// [`devcore::helper::Client::linux_proxy_socket_stop`].
#[derive(Clone)]
pub struct SystemdSocketInstaller {
    root: PathBuf,
    runner: Arc<dyn ProcessRunner>,
    helper: Arc<dyn LinuxProxySocketHelper>,
    username: Option<String>,
    home: Option<String>,
    unit_dir: Option<PathBuf>,
}

impl SystemdSocketInstaller {
    pub fn new(root: impl Into<PathBuf>, runner: Arc<dyn ProcessRunner>) -> Self {
        SystemdSocketInstaller {
            root: root.into(),
            runner,
            helper: Arc::new(DefaultLinuxProxySocketHelper),
            username: None,
            home: None,
            unit_dir: None,
        }
    }

    pub fn with_helper(mut self, helper: Arc<dyn LinuxProxySocketHelper>) -> Self {
        self.helper = helper;
        self
    }

    pub fn with_username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    pub fn with_home(mut self, home: impl Into<String>) -> Self {
        self.home = Some(home.into());
        self
    }

    pub fn with_unit_dir(mut self, unit_dir: impl Into<PathBuf>) -> Self {
        self.unit_dir = Some(unit_dir.into());
        self
    }

    pub fn socket_unit_name(&self) -> String {
        format!("{}.socket", linux_proxy_unit_stem(&self.root))
    }

    pub fn service_unit_name(&self) -> String {
        format!("{}.service", linux_proxy_unit_stem(&self.root))
    }

    fn effective_username(&self) -> String {
        if let Some(username) = &self.username {
            return username.clone();
        }
        if let Ok(user) = std::env::var("USER")
            && !user.trim().is_empty()
        {
            return user;
        }
        if let Ok(user) = std::env::var("LOGNAME")
            && !user.trim().is_empty()
        {
            return user;
        }
        // SAFETY: `geteuid`/`getpwuid` return a static passwd pointer or null.
        unsafe {
            let passwd = libc::getpwuid(libc::geteuid());
            if !passwd.is_null() && !(*passwd).pw_name.is_null() {
                return std::ffi::CStr::from_ptr((*passwd).pw_name)
                    .to_string_lossy()
                    .into_owned();
            }
        }
        String::new()
    }

    fn effective_home(&self) -> String {
        if let Some(home) = &self.home {
            return home.clone();
        }
        devcore::state::home_from_environment()
            .to_string_lossy()
            .into_owned()
    }
}

impl SocketInstaller for SystemdSocketInstaller {
    fn loaded(&self) -> bool {
        let socket_unit = self.socket_unit_name();
        if self
            .runner
            .run("systemctl", &["is-active", &socket_unit])
            .is_ok()
        {
            return true;
        }
        let service_unit = self.service_unit_name();
        self.runner
            .run("systemctl", &["is-active", &service_unit])
            .is_ok()
    }

    fn install(&self, sites: &[ProxyRoute], executable: &str) -> Result<(), ProxyLifecycleError> {
        let ports = listener_ports(sites);
        let username = self.effective_username();
        let home = self.effective_home();
        let units = build_linux_proxy_socket_units(
            &self.root,
            self.unit_dir.as_deref(),
            Path::new(executable),
            &username,
            &ports,
            &home,
        )
        .map_err(|error| ProxyLifecycleError::Invalid(error.to_string()))?;

        self.helper
            .linux_proxy_socket_install(&LinuxProxySocketInstallRequest {
                socket_unit_name: units.socket_unit_name,
                service_unit_name: units.service_unit_name,
                socket_content: units.socket_content.into_bytes(),
                service_content: units.service_content.into_bytes(),
            })
            .map_err(|error| ProxyLifecycleError::Invalid(error.to_string()))
    }

    fn stop(&self) -> Result<(), ProxyLifecycleError> {
        let request = LinuxProxySocketStopRequest {
            socket_unit_name: self.socket_unit_name(),
            service_unit_name: self.service_unit_name(),
        };
        match self.helper.linux_proxy_socket_stop(&request) {
            Ok(()) => Ok(()),
            Err(HelperError::NotInstalled | HelperError::Disabled) if !self.loaded() => Ok(()),
            Err(error) => Err(ProxyLifecycleError::Invalid(error.to_string())),
        }
    }

    fn kickstart(&self) -> Result<(), ProxyLifecycleError> {
        self.helper
            .systemd_control(&[vec!["restart".into(), self.service_unit_name()]])
            .map_err(|error| ProxyLifecycleError::Invalid(error.to_string()))
    }
}

/// Returns the default platform [`SocketInstaller`].
pub fn default_socket_installer(
    root: &Path,
    runner: Arc<dyn ProcessRunner>,
) -> Arc<dyn SocketInstaller> {
    #[cfg(target_os = "macos")]
    {
        let _ = (root, runner);
        Arc::new(Phase4SocketInstaller)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(SystemdSocketInstaller::new(root, runner))
    }
}

/// The current executable, injected for tests and bundle moves.
pub trait ExecutableSource: Send + Sync {
    fn executable(&self) -> Result<PathBuf, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CurrentExecutable;

impl ExecutableSource for CurrentExecutable {
    fn executable(&self) -> Result<PathBuf, String> {
        std::env::current_exe().map_err(|error| error.to_string())
    }
}

/// Proxy lifecycle facade.
pub struct ProxyService {
    root: PathBuf,
    store: Store,
    engine: Engine,
    authority: Arc<AuthorityService>,
    executable: Arc<dyn ExecutableSource>,
    probe: Arc<dyn PortProbe>,
    health: Arc<dyn HealthProbe>,
    tls: Arc<dyn TlsVerifierFactory>,
    shutdown: Arc<dyn ShutdownVerifier>,
    sockets: Arc<dyn SocketInstaller>,
    event: Arc<dyn Fn(&str) + Send + Sync>,
}

/// Builds a verifier from the current CA DER at apply time.
pub trait TlsVerifierFactory: Send + Sync {
    fn verifier(&self, ca_der: Vec<u8>) -> Arc<dyn TlsVerifier>;
}

pub struct SystemTlsVerifierFactory;

impl TlsVerifierFactory for SystemTlsVerifierFactory {
    fn verifier(&self, ca_der: Vec<u8>) -> Arc<dyn TlsVerifier> {
        Arc::new(SystemTlsVerifier::new(ca_der))
    }
}

impl ProxyService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Store,
        runner: Arc<dyn ProcessRunner>,
        authority: Arc<AuthorityService>,
        executable: Arc<dyn ExecutableSource>,
        probe: Arc<dyn PortProbe>,
        health: Arc<dyn HealthProbe>,
        tls: Arc<dyn TlsVerifierFactory>,
        shutdown: Arc<dyn ShutdownVerifier>,
        sockets: Arc<dyn SocketInstaller>,
    ) -> Self {
        let engine = Engine::new(store.root(), Arc::new(ProcessRunnerAdapter::new(runner)));
        ProxyService {
            root: store.root().to_path_buf(),
            store,
            engine,
            authority,
            executable,
            probe,
            health,
            tls,
            shutdown,
            sockets,
            event: Arc::new(|_| {}),
        }
    }

    /// The production seams (`Phase4SocketInstaller` on macOS,
    /// [`SystemdSocketInstaller`] on Linux).
    pub fn system(store: Store, authority: Arc<AuthorityService>) -> Self {
        let runner: Arc<dyn ProcessRunner> = Arc::new(devcore::launchd::SystemRunner::new());
        let engine = Engine::new(
            store.root(),
            Arc::new(ProcessRunnerAdapter::new(Arc::clone(&runner))),
        );
        let sockets = default_socket_installer(store.root(), Arc::clone(&runner));
        ProxyService::new(
            store,
            runner,
            authority,
            Arc::new(CurrentExecutable),
            Arc::new(SystemPortProbe),
            Arc::new(SystemHealthProbe),
            Arc::new(SystemTlsVerifierFactory),
            Arc::new(EngineShutdownVerifier::new(engine)),
            sockets,
        )
    }

    pub fn with_engine(mut self, engine: Engine) -> Self {
        self.engine = engine;
        self
    }

    pub fn with_event(mut self, event: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.event = Arc::new(event);
        self
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    fn loaded(&self, id: &str) -> bool {
        matches!(self.engine.job_state(id), Ok(Some(state)) if state.loaded)
    }

    /// Apply the saved state: lease ports, validate, publish, start, verify.
    pub fn apply(&self) -> Result<(), ProxyLifecycleError> {
        self.transact(|state| self.apply_locked(state))
    }

    fn apply_locked(&self, state: &mut devcore::model::State) -> Result<(), ProxyLifecycleError> {
        self.start_proxy(state)?;
        let tunnel_loaded = self.loaded("tunnel");
        if published_config_exists(&self.root) || tunnel_loaded {
            match write_config_from_state(&self.root, state) {
                Ok(_) => {}
                Err(error) if tunnel_loaded => {
                    return Err(ProxyLifecycleError::Invalid(error.to_string()));
                }
                Err(_) => {}
            }
        }
        Ok(())
    }

    fn start_proxy(&self, state: &mut devcore::model::State) -> Result<(), ProxyLifecycleError> {
        let sites = routes_from_state(state);
        let ports = listener_ports(&sites);
        let keys: Vec<ResourceKey> = ports
            .iter()
            .map(|port| ResourceKey::new("TCP port", port.to_string()))
            .collect();
        let _leases = self.store.resource_leases(&keys)?;
        validate_proxy_routes(&sites)?;

        if sites.is_empty() {
            self.stop_proxy()?;
            self.shutdown
                .verify_stopped("proxy")
                .map_err(ProxyLifecycleError::Invalid)?;
            if self.sockets.loaded() {
                return Err(ProxyLifecycleError::Invalid(
                    "privileged HTTPS proxy socket service is still active".into(),
                ));
            }
            write_published_config(
                &self.root,
                &PublishedProxyConfig {
                    schema_version: 1,
                    listener_port: state.proxy_listener_port,
                    rules: Vec::new(),
                },
            )?;
            return Ok(());
        }

        let authority = self.authority.load_or_create()?;
        let old = read_published_config(&self.root)
            .map(|config| super::rules::routes_from_config(&config))
            .unwrap_or_default();
        let executable = self
            .executable
            .executable()
            .map_err(ProxyLifecycleError::Invalid)?;
        let executable = executable.to_string_lossy().into_owned();

        if privileged_ports(&sites) {
            return self.start_socket_proxy(&sites, &old, &executable);
        }

        let loaded = self.loaded("proxy");
        self.preflight(&sites, &old, loaded || self.sockets.loaded())?;
        self.sockets.stop()?;
        let helper = crate::helpers::helper_executable(
            &self.root,
            "proxy",
            std::path::Path::new(&executable),
        )?;
        let executable = helper.to_string_lossy().into_owned();
        let before: BTreeSet<i64> = old.iter().map(|site| site.listener_port).collect();
        let after: BTreeSet<i64> = ports.iter().copied().collect();
        let changed = before != after;
        if loaded && changed {
            self.engine.stop_job("proxy", &|| false)?;
        }

        write_published_config(&self.root, &PublishedProxyConfig::from_state(state))?;
        let spec = JobSpec {
            id: "proxy".into(),
            args: vec![
                executable,
                "--dev".into(),
                "serve".into(),
                self.root_string(),
            ],
            working_dir: self.root_string(),
            env: home_env(),
            restart: false,
        };
        let fresh = match self.engine.start_job(&spec, &|| false) {
            Ok(fresh) => fresh,
            Err(error) => {
                let _ =
                    write_published_config(&self.root, &PublishedProxyConfig::from_routes(&old));
                if loaded {
                    let _ = self.engine.start_job(&spec, &|| false);
                }
                return Err(error.into());
            }
        };
        let mut failure = None;
        if fresh {
            for port in &ports {
                if let Err(error) = self.health.wait_tcp(*port) {
                    failure = Some(ProxyLifecycleError::Invalid(error));
                    break;
                }
            }
        }
        if failure.is_none() {
            let verifier = self.tls.verifier(authority.cert_der.clone());
            for site in &sites {
                if let Err(error) = verifier.verify(&site.domain, site.listener_port) {
                    failure = Some(ProxyLifecycleError::Invalid(error));
                    break;
                }
            }
        }
        if let Some(failure) = failure {
            let _ = self.engine.stop_job("proxy", &|| false);
            let _ = write_published_config(&self.root, &PublishedProxyConfig::from_routes(&old));
            if loaded {
                let _ = self.engine.start_job(&spec, &|| false);
            }
            return Err(failure);
        }
        Ok(())
    }

    fn preflight(
        &self,
        sites: &[ProxyRoute],
        old: &[ProxyRoute],
        loaded: bool,
    ) -> Result<(), ProxyLifecycleError> {
        let old_ports: BTreeSet<i64> = old.iter().map(|site| site.listener_port).collect();
        for port in listener_ports(sites) {
            if loaded && old_ports.contains(&port) {
                continue;
            }
            if !self.probe.available(port) && self.probe.answering(port) {
                return Err(ProxyLifecycleError::PortUnavailable {
                    port,
                    detail: "the port is already accepting connections".into(),
                });
            }
        }
        Ok(())
    }

    fn start_socket_proxy(
        &self,
        sites: &[ProxyRoute],
        old: &[ProxyRoute],
        executable: &str,
    ) -> Result<(), ProxyLifecycleError> {
        self.preflight(sites, old, self.loaded("proxy") || self.sockets.loaded())?;
        let _ = self.engine.stop_job("proxy", &|| false);
        write_published_config(&self.root, &PublishedProxyConfig::from_routes(sites))?;
        if let Err(error) = self.sockets.install(sites, executable) {
            let _ = write_published_config(&self.root, &PublishedProxyConfig::from_routes(old));
            return Err(error);
        }
        let authority = self.authority.load_or_create()?;
        let verifier = self.tls.verifier(authority.cert_der.clone());
        for site in sites {
            if let Err(error) = verifier.verify(&site.domain, site.listener_port) {
                let _ = self.sockets.stop();
                let _ = write_published_config(&self.root, &PublishedProxyConfig::from_routes(old));
                return Err(ProxyLifecycleError::Invalid(error));
            }
        }
        Ok(())
    }

    /// Stop the user job and the privileged daemon.
    pub fn stop(&self) -> Result<(), ProxyLifecycleError> {
        self.engine.stop_job("proxy", &|| false)?;
        self.sockets.stop()?;
        Ok(())
    }

    /// Restart the proxy: kickstart the daemon when loaded, else re-apply.
    pub fn restart(&self) -> Result<(), ProxyLifecycleError> {
        self.transact(|state| self.restart_locked(state))
    }

    /// Run a mutating operation under the state transaction while preserving
    /// its typed error for the UI.
    fn transact<F>(&self, operation: F) -> Result<(), ProxyLifecycleError>
    where
        F: FnOnce(&mut devcore::model::State) -> Result<(), ProxyLifecycleError>,
    {
        let mut captured = None;
        let result = self.store.with_state(|state| match operation(state) {
            Ok(()) => Ok(()),
            Err(error) => {
                captured = Some(error);
                Err(StateError::Io(io::Error::other(
                    "proxy operation did not commit",
                )))
            }
        });
        match captured {
            Some(error) => Err(error),
            None => {
                result?;
                Ok(())
            }
        }
    }

    fn restart_locked(&self, state: &mut devcore::model::State) -> Result<(), ProxyLifecycleError> {
        let sites = routes_from_state(state);
        if sites.is_empty() {
            return Err(ProxyLifecycleError::Invalid(
                "no HTTPS proxy rules configured".into(),
            ));
        }
        if self.sockets.loaded() {
            self.sockets.kickstart()?;
            let authority = self.authority.load_or_create()?;
            let verifier = self.tls.verifier(authority.cert_der.clone());
            for site in &sites {
                verifier
                    .verify(&site.domain, site.listener_port)
                    .map_err(ProxyLifecycleError::Invalid)?;
            }
            return Ok(());
        }
        self.engine.stop_job("proxy", &|| false)?;
        self.start_proxy(state)
    }

    /// The publication state (`Unknown`, `Pending`, `Published`).
    pub fn publication_state(&self) -> Result<&'static str, ProxyLifecycleError> {
        Ok(super::rules::publication_state(&self.store)?)
    }

    fn root_string(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    fn stop_proxy(&self) -> Result<(), ProxyLifecycleError> {
        self.engine.stop_job("proxy", &|| false)?;
        self.sockets.stop()?;
        Ok(())
    }
}

fn home_env() -> std::collections::BTreeMap<String, String> {
    let mut env = std::collections::BTreeMap::new();
    if let Some(home) = std::env::var_os("HOME") {
        env.insert("HOME".into(), home.to_string_lossy().into_owned());
    }
    env
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use devcore::launchd::CommandError;
    use tempfile::TempDir;

    use super::*;
    use crate::ca::store::AuthorityStore;
    use crate::ca::{AlwaysStopped, FixedProxyGate, ProxyStopCheck, SecurityTrust};

    #[derive(Default)]
    struct RecordingRunner {
        calls: Mutex<Vec<Vec<String>>>,
        responses: Mutex<VecDeque<Result<String, CommandError>>>,
    }

    impl ProcessRunner for RecordingRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
            let mut call = vec![program.to_string()];
            call.extend(args.iter().map(|arg| arg.to_string()));
            self.calls.lock().unwrap().push(call);
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(CommandError::new(program, Vec::new(), "not found"));
            }
            responses.pop_front().unwrap()
        }
    }

    struct FakeProbe {
        available: bool,
        answering: bool,
    }

    impl PortProbe for FakeProbe {
        fn available(&self, _port: i64) -> bool {
            self.available
        }
        fn answering(&self, _port: i64) -> bool {
            self.answering
        }
    }

    struct OkHealth;
    impl HealthProbe for OkHealth {
        fn wait_tcp(&self, _port: i64) -> Result<(), String> {
            Ok(())
        }
    }

    struct OkTls;
    impl TlsVerifier for OkTls {
        fn verify(&self, _domain: &str, _port: i64) -> Result<(), String> {
            Ok(())
        }
    }

    struct OkTlsFactory;
    impl TlsVerifierFactory for OkTlsFactory {
        fn verifier(&self, _ca_der: Vec<u8>) -> Arc<dyn TlsVerifier> {
            Arc::new(OkTls)
        }
    }

    struct OkShutdown;
    impl ShutdownVerifier for OkShutdown {
        fn verify_stopped(&self, _target: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn service(
        state: devcore::model::State,
        probe: Arc<dyn PortProbe>,
    ) -> (TempDir, Store, Arc<ProxyService>, Arc<RecordingRunner>) {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        store.write_state(&state).unwrap();
        let runner = Arc::new(RecordingRunner::default());
        let authority = Arc::new(AuthorityService::new(
            AuthorityStore::new(dir.path()),
            Arc::new(SecurityTrust::new(Arc::new(RecordingRunner::default()))),
            Arc::new(AlwaysStopped),
        ));
        let service = Arc::new(ProxyService::new(
            store.clone(),
            runner.clone(),
            authority,
            Arc::new(CurrentExecutable),
            probe,
            Arc::new(OkHealth),
            Arc::new(OkTlsFactory),
            Arc::new(OkShutdown),
            Arc::new(Phase4SocketInstaller),
        ));
        (dir, store, service, runner)
    }

    fn rule(domain: &str, port: i64) -> devcore::model::ProxyRule {
        devcore::model::ProxyRule {
            domain: domain.into(),
            local_port: port,
            ..devcore::model::ProxyRule::default()
        }
    }

    #[test]
    fn no_rules_writes_an_empty_snapshot_and_stops_the_job() {
        let state = devcore::model::State {
            proxy_listener_port: 443,
            ..devcore::model::State::default()
        };
        let (_dir, store, service, runner) = service(
            state,
            Arc::new(FakeProbe {
                available: true,
                answering: false,
            }),
        );
        service.apply().unwrap();
        let config = read_published_config(store.root()).unwrap();
        assert!(config.rules.is_empty());
        assert!(
            runner.calls.lock().unwrap().iter().any(|call| {
                call.contains(&"print".to_string()) || call.contains(&"show".to_string())
            }),
            "the proxy job was inspected before being treated as absent"
        );
    }

    #[test]
    fn occupied_ports_fail_preflight() {
        let state = devcore::model::State {
            proxy_listener_port: 8443,
            proxy_rules: vec![rule("app.devbase.localhost", 3000)],
            ..devcore::model::State::default()
        };
        let (_dir, _store, service, _runner) = service(
            state,
            Arc::new(FakeProbe {
                available: false,
                answering: true,
            }),
        );
        let error = service.apply().unwrap_err();
        assert!(matches!(
            error,
            ProxyLifecycleError::PortUnavailable { port: 8443, .. }
        ));
    }

    #[test]
    fn privileged_ports_stop_at_the_phase4_seam() {
        let state = devcore::model::State {
            proxy_listener_port: 443,
            proxy_rules: vec![rule("app.devbase.localhost", 3000)],
            ..devcore::model::State::default()
        };
        let (_dir, _store, service, _runner) = service(
            state,
            Arc::new(FakeProbe {
                available: true,
                answering: false,
            }),
        );
        let error = service.apply().unwrap_err();
        assert!(matches!(error, ProxyLifecycleError::Unsupported(_)));
        assert!(error.to_string().contains("Phase 4 socket daemon helper"));
    }

    #[test]
    fn fixed_gate_reports_running_proxies() {
        assert!(FixedProxyGate { running: true }.verify_stopped().is_err());
        assert!(FixedProxyGate { running: false }.verify_stopped().is_ok());
    }

    struct FakeLinuxSocketHelper {
        installs: Mutex<Vec<LinuxProxySocketInstallRequest>>,
        stops: Mutex<Vec<LinuxProxySocketStopRequest>>,
        controls: Mutex<Vec<Vec<Vec<String>>>>,
        install_result: Mutex<Result<(), HelperError>>,
        stop_result: Mutex<Result<(), HelperError>>,
        control_result: Mutex<Result<(), HelperError>>,
    }

    impl FakeLinuxSocketHelper {
        fn succeeding() -> Arc<Self> {
            Arc::new(FakeLinuxSocketHelper {
                installs: Mutex::new(Vec::new()),
                stops: Mutex::new(Vec::new()),
                controls: Mutex::new(Vec::new()),
                install_result: Mutex::new(Ok(())),
                stop_result: Mutex::new(Ok(())),
                control_result: Mutex::new(Ok(())),
            })
        }

        fn not_installed() -> Arc<Self> {
            Arc::new(FakeLinuxSocketHelper {
                installs: Mutex::new(Vec::new()),
                stops: Mutex::new(Vec::new()),
                controls: Mutex::new(Vec::new()),
                install_result: Mutex::new(Err(HelperError::NotInstalled)),
                stop_result: Mutex::new(Err(HelperError::NotInstalled)),
                control_result: Mutex::new(Err(HelperError::NotInstalled)),
            })
        }
    }

    impl LinuxProxySocketHelper for FakeLinuxSocketHelper {
        fn linux_proxy_socket_install(
            &self,
            request: &LinuxProxySocketInstallRequest,
        ) -> Result<(), HelperError> {
            self.installs.lock().unwrap().push(request.clone());
            self.install_result.lock().unwrap().clone()
        }

        fn linux_proxy_socket_stop(
            &self,
            request: &LinuxProxySocketStopRequest,
        ) -> Result<(), HelperError> {
            self.stops.lock().unwrap().push(request.clone());
            self.stop_result.lock().unwrap().clone()
        }

        fn systemd_control(&self, commands: &[Vec<String>]) -> Result<(), HelperError> {
            self.controls.lock().unwrap().push(commands.to_vec());
            self.control_result.lock().unwrap().clone()
        }
    }

    struct FixedExecutable(PathBuf);
    impl ExecutableSource for FixedExecutable {
        fn executable(&self) -> Result<PathBuf, String> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn engine_shutdown_verifier_uses_launchd_and_systemd_backends() {
        let dir = TempDir::new().unwrap();

        // Launchd backend: absent job -> Ok, running job -> Err, full target is unwrapped.
        let launchd_runner = Arc::new(RecordingRunner {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(
                vec![
                    Err(CommandError::new(
                        "launchctl",
                        vec![],
                        "Could not find service",
                    )),
                    Ok("state = running\npid = 4242\n".into()),
                    Err(CommandError::new(
                        "launchctl",
                        vec![],
                        "Could not find specified service",
                    )),
                ]
                .into(),
            ),
        });
        let launchd_engine = Engine::new(
            dir.path(),
            Arc::new(ProcessRunnerAdapter::new(launchd_runner.clone())),
        )
        .with_backend(Arc::new(devcore::engine::LaunchdBackend));
        let verifier = EngineShutdownVerifier::new(launchd_engine.clone());
        assert!(verifier.verify_stopped("proxy").is_ok());
        assert!(verifier.verify_stopped("proxy").is_err());
        assert!(
            verifier
                .verify_stopped(&launchd_engine.target("proxy"))
                .is_ok()
        );

        // Systemd user backend: cleanly inactive -> Ok, active/running -> Err.
        let systemd_runner = Arc::new(RecordingRunner {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(
                vec![
                    Ok("LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n".into()),
                    Ok("LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=9001\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n".into()),
                ]
                .into(),
            ),
        });
        let systemd_engine = Engine::new(
            dir.path(),
            Arc::new(ProcessRunnerAdapter::new(systemd_runner.clone())),
        )
        .with_backend(Arc::new(
            devcore::engine::SystemdUserBackend::with_unit_dir(dir.path().join("systemd-user")),
        ));
        let verifier = EngineShutdownVerifier::new(systemd_engine);
        assert!(verifier.verify_stopped("proxy").is_ok());
        assert!(verifier.verify_stopped("proxy").is_err());
        let calls = systemd_runner.calls.lock().unwrap();
        assert_eq!(calls[0][0], "systemctl");
        assert_eq!(calls[0][1], "--user");
        assert_eq!(calls[0][2], "show");
    }

    #[test]
    fn proxy_service_loaded_uses_engine_backend() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let runner = Arc::new(RecordingRunner {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(
                vec![
                    Ok("LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=1234\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n".into()),
                    Ok("LoadState=not-found\nActiveState=inactive\nSubState=dead\nMainPID=0\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n".into()),
                ]
                .into(),
            ),
        });
        let engine = Engine::new(
            dir.path(),
            Arc::new(ProcessRunnerAdapter::new(runner.clone())),
        )
        .with_backend(Arc::new(
            devcore::engine::SystemdUserBackend::with_unit_dir(dir.path().join("systemd-user")),
        ));
        let authority = Arc::new(AuthorityService::new(
            AuthorityStore::new(dir.path()),
            Arc::new(SecurityTrust::new(Arc::new(RecordingRunner::default()))),
            Arc::new(AlwaysStopped),
        ));
        let svc = ProxyService::new(
            store,
            runner.clone(),
            authority,
            Arc::new(CurrentExecutable),
            Arc::new(FakeProbe {
                available: true,
                answering: false,
            }),
            Arc::new(OkHealth),
            Arc::new(OkTlsFactory),
            Arc::new(OkShutdown),
            Arc::new(Phase4SocketInstaller),
        )
        .with_engine(engine);

        assert!(svc.loaded("proxy"));
        assert!(!svc.loaded("tunnel"));
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0][0], "systemctl");
        assert_eq!(calls[1][0], "systemctl");
    }

    #[test]
    fn systemd_socket_installer_installs_stops_kickstarts_and_checks_loaded() {
        let dir = TempDir::new().unwrap();
        let runner = Arc::new(RecordingRunner {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(
                vec![
                    Ok("active\n".into()),
                    Err(CommandError::new("systemctl", vec![], "inactive")),
                    Err(CommandError::new("systemctl", vec![], "inactive")),
                ]
                .into(),
            ),
        });
        let helper = FakeLinuxSocketHelper::succeeding();
        let installer = SystemdSocketInstaller::new(dir.path(), runner.clone())
            .with_helper(helper.clone())
            .with_username("alice")
            .with_home("/home/alice");

        assert!(installer.loaded());
        assert!(!installer.loaded());

        let sites = vec![ProxyRoute {
            domain: "app.devbase.localhost".into(),
            listener_port: 443,
            local_port: 3000,
            upstream_host: "127.0.0.1".into(),
        }];
        installer.install(&sites, "/usr/bin/devbase").unwrap();
        {
            let installs = helper.installs.lock().unwrap();
            assert_eq!(installs.len(), 1);
            assert_eq!(installs[0].socket_unit_name, installer.socket_unit_name());
            assert_eq!(installs[0].service_unit_name, installer.service_unit_name());
            let socket_text = String::from_utf8(installs[0].socket_content.clone()).unwrap();
            let service_text = String::from_utf8(installs[0].service_content.clone()).unwrap();
            assert!(socket_text.contains("ListenStream=127.0.0.1:443"));
            assert!(service_text.contains("User=alice"));
            assert!(service_text.contains("MSM_DEV_SOCKET_ACTIVATION=1"));
            assert!(service_text.contains("/usr/bin/devbase --dev serve"));
        }

        installer.kickstart().unwrap();
        assert_eq!(
            helper.controls.lock().unwrap()[0],
            vec![vec!["restart".to_string(), installer.service_unit_name()]]
        );

        installer.stop().unwrap();
        {
            let stops = helper.stops.lock().unwrap();
            assert_eq!(stops.len(), 1);
            assert_eq!(stops[0].socket_unit_name, installer.socket_unit_name());
            assert_eq!(stops[0].service_unit_name, installer.service_unit_name());
        }
    }

    #[test]
    fn systemd_socket_installer_stop_is_noop_when_helper_missing_and_unit_inactive() {
        let dir = TempDir::new().unwrap();
        let inactive_runner = Arc::new(RecordingRunner::default());
        let helper = FakeLinuxSocketHelper::not_installed();
        let installer = SystemdSocketInstaller::new(dir.path(), inactive_runner)
            .with_helper(helper.clone())
            .with_username("alice")
            .with_home("/home/alice");
        installer.stop().unwrap();

        // If the unit IS still active while helper is missing, stop must report an error.
        let active_runner = Arc::new(RecordingRunner {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(vec![Ok("active\n".into())].into()),
        });
        let installer = SystemdSocketInstaller::new(dir.path(), active_runner)
            .with_helper(helper)
            .with_username("alice")
            .with_home("/home/alice");
        assert!(installer.stop().is_err());
    }

    #[test]
    fn proxy_service_applies_privileged_port_443_with_systemd_socket_installer() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        store
            .write_state(&devcore::model::State {
                proxy_listener_port: 443,
                proxy_rules: vec![rule("app.devbase.localhost", 3000)],
                ..devcore::model::State::default()
            })
            .unwrap();

        let runner = Arc::new(RecordingRunner::default());
        let helper = FakeLinuxSocketHelper::succeeding();
        let sockets = Arc::new(
            SystemdSocketInstaller::new(dir.path(), Arc::new(RecordingRunner::default()))
                .with_helper(helper.clone())
                .with_username("alice")
                .with_home("/home/alice"),
        );
        let engine = Engine::new(
            dir.path(),
            Arc::new(ProcessRunnerAdapter::new(runner.clone())),
        )
        .with_backend(Arc::new(
            devcore::engine::SystemdUserBackend::with_unit_dir(dir.path().join("systemd-user")),
        ));
        let authority = Arc::new(AuthorityService::new(
            AuthorityStore::new(dir.path()),
            Arc::new(SecurityTrust::new(Arc::new(RecordingRunner::default()))),
            Arc::new(AlwaysStopped),
        ));
        let svc = ProxyService::new(
            store.clone(),
            runner,
            authority,
            Arc::new(FixedExecutable(PathBuf::from("/usr/bin/devbase"))),
            Arc::new(FakeProbe {
                available: true,
                answering: false,
            }),
            Arc::new(OkHealth),
            Arc::new(OkTlsFactory),
            Arc::new(OkShutdown),
            sockets,
        )
        .with_engine(engine);

        svc.apply().unwrap();
        assert_eq!(helper.installs.lock().unwrap().len(), 1);
        let published = read_published_config(store.root()).unwrap();
        assert_eq!(published.listener_port, 443);
        assert_eq!(published.rules.len(), 1);
    }
}
