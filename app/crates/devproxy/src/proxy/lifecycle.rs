//! Proxy apply/start/stop/restart orchestration.
//!
//! Ported from `startProxy`/`ApplyProxy`/`RestartProxy`/`StopProxy`. Every
//! operating-system boundary is a trait: launchd jobs go through devcore's
//! [`Engine`], ports through [`PortProbe`], TLS verification through
//! [`TlsVerifier`], readiness through [`HealthProbe`], and the privileged
//! socket daemon through [`SocketInstaller`] (Phase 4).

use std::collections::BTreeSet;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devcore::engine::{Engine, EngineError, JobSpec, ProcessRunnerAdapter};
use devcore::health::parse_job_state;
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

/// The real verifier: a running launchd job with a PID is not stopped.
#[derive(Clone)]
pub struct EngineShutdownVerifier {
    runner: Arc<dyn ProcessRunner>,
}

impl EngineShutdownVerifier {
    pub fn new(runner: Arc<dyn ProcessRunner>) -> Self {
        EngineShutdownVerifier { runner }
    }
}

impl ShutdownVerifier for EngineShutdownVerifier {
    fn verify_stopped(&self, target: &str) -> Result<(), String> {
        match self.runner.run("/bin/launchctl", &["print", target]) {
            Ok(output) => {
                let state = parse_job_state(&output);
                if state.running && state.pid > 0 {
                    Err(format!("{target} is still running"))
                } else {
                    Ok(())
                }
            }
            Err(_) => Ok(()),
        }
    }
}

/// The Phase 4 privileged socket-daemon boundary.
pub trait SocketInstaller: Send + Sync {
    fn loaded(&self) -> bool;
    fn install(&self, sites: &[ProxyRoute], executable: &str) -> Result<(), ProxyLifecycleError>;
    fn stop(&self) -> Result<(), ProxyLifecycleError>;
    fn kickstart(&self) -> Result<(), ProxyLifecycleError>;
}

/// Until the Phase 4 helper lands, privileged ports stop at this seam.
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
    runner: Arc<dyn ProcessRunner>,
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
        let engine = Engine::new(
            store.root(),
            Arc::new(ProcessRunnerAdapter::new(Arc::clone(&runner))),
        );
        ProxyService {
            root: store.root().to_path_buf(),
            store,
            engine,
            runner,
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

    /// The production seams (socket install remains Phase 4).
    pub fn system(store: Store, authority: Arc<AuthorityService>) -> Self {
        let runner: Arc<dyn ProcessRunner> = Arc::new(devcore::launchd::SystemRunner::new());
        ProxyService::new(
            store,
            Arc::clone(&runner),
            authority,
            Arc::new(CurrentExecutable),
            Arc::new(SystemPortProbe),
            Arc::new(SystemHealthProbe),
            Arc::new(SystemTlsVerifierFactory),
            Arc::new(EngineShutdownVerifier::new(runner)),
            Arc::new(Phase4SocketInstaller),
        )
    }

    pub fn with_event(mut self, event: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.event = Arc::new(event);
        self
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    fn loaded(&self, id: &str) -> bool {
        self.runner
            .run("/bin/launchctl", &["print", &self.engine.target(id)])
            .is_ok()
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
                .verify_stopped(&self.engine.target("proxy"))
                .map_err(ProxyLifecycleError::Invalid)?;
            self.shutdown
                .verify_stopped("system/proxy-sockets")
                .map_err(ProxyLifecycleError::Invalid)?;
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
            runner
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.contains(&"print".to_string())),
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
}
