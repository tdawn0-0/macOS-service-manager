//! The tunnel launchd job and apply/start/stop/restart order.
//!
//! Ported from `tunnel_client.go`'s job half. Synchronization is a separate
//! async step (`TunnelService::sync_tunnels`); `apply` runs it first when the
//! saved target is stale, exactly like `ApplyTunnel`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devcore::engine::{Engine, EngineError, JobSpec, ProcessRunnerAdapter};
use devcore::health::parse_job_state;
use devcore::launchd::ProcessRunner;
use devcore::state::{StateError, Store};

use crate::helpers::helper_executable;
use crate::proxy::lifecycle::{CurrentExecutable, ExecutableSource};
use crate::secrets::SecretError;

use super::cloudflare::DEFAULT_API_BASE;
use super::config::{TunnelConfigError, tunnel_config_from_state};
use super::service::{TunnelError, TunnelJobControl, TunnelService};

const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors from tunnel job control.
#[derive(Debug, thiserror::Error)]
pub enum TunnelLifecycleError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Config(#[from] TunnelConfigError),
    #[error(transparent)]
    Secret(#[from] SecretError),
    #[error(transparent)]
    Tunnel(#[from] TunnelError),
    #[error("{0}")]
    Invalid(String),
}

/// The production tunnel job.
pub struct TunnelJobs {
    root: PathBuf,
    store: Store,
    engine: Engine,
    runner: Arc<dyn ProcessRunner>,
    executable: Arc<dyn ExecutableSource>,
    api_base: String,
}

impl TunnelJobs {
    pub fn new(store: Store, runner: Arc<dyn ProcessRunner>) -> Self {
        let engine = Engine::new(
            store.root(),
            Arc::new(ProcessRunnerAdapter::new(Arc::clone(&runner))),
        );
        TunnelJobs {
            root: store.root().to_path_buf(),
            store,
            engine,
            runner,
            executable: Arc::new(CurrentExecutable),
            api_base: DEFAULT_API_BASE.to_string(),
        }
    }

    pub fn with_executable(mut self, executable: Arc<dyn ExecutableSource>) -> Self {
        self.executable = executable;
        self
    }

    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = api_base.into();
        self
    }

    pub fn service(&self) -> TunnelService {
        TunnelService::new(self.store.clone(), self.api_base.clone()).with_jobs(Arc::new(Self {
            root: self.root.clone(),
            store: self.store.clone(),
            engine: Engine::new(
                self.store.root(),
                Arc::new(ProcessRunnerAdapter::new(Arc::clone(&self.runner))),
            ),
            runner: Arc::clone(&self.runner),
            executable: Arc::clone(&self.executable),
            api_base: self.api_base.clone(),
        }))
    }

    fn secrets_ensure(&self) -> Result<(), TunnelLifecycleError> {
        crate::secrets::SecretStore::new(&self.root).ensure_tunnel_secret()?;
        Ok(())
    }

    fn loaded(&self) -> bool {
        self.runner
            .run("/bin/launchctl", &["print", &self.engine.target("tunnel")])
            .is_ok()
    }

    fn running_pid(&self) -> Option<i32> {
        let output = self
            .runner
            .run("/bin/launchctl", &["print", &self.engine.target("tunnel")])
            .ok()?;
        let state = parse_job_state(&output);
        (state.running && state.pid > 0).then_some(state.pid)
    }

    /// `ApplyTunnel`: sync first when needed, then ensure the job runs.
    pub async fn apply(&self) -> Result<(), TunnelLifecycleError> {
        let service = self.service();
        if service.needs_sync()? && service.has_cloudflare_token() {
            service.sync_tunnels().await?;
        }
        self.start()
    }

    /// Publish the snapshot and start the job, or return if it is running.
    pub fn start(&self) -> Result<(), TunnelLifecycleError> {
        self.store.init()?;
        self.secrets_ensure()?;
        let config = tunnel_config_from_state(&self.store.read()?)?;
        if config.routes.is_empty() {
            return Err(TunnelLifecycleError::Invalid(
                "enable Tunnel on at least one HTTPS rule first".into(),
            ));
        }
        self.store.with_state(|state| {
            crate::tunnel::config::write_config_from_state(&self.root, state)
                .map_err(|error| StateError::Io(std::io::Error::other(error)))?;
            Ok(())
        })?;
        if self.running_pid().is_some() {
            return Ok(());
        }
        let source = self
            .executable
            .executable()
            .map_err(TunnelLifecycleError::Invalid)?;
        let helper = helper_executable(&self.root, "tunnel", &source)?;
        let spec = JobSpec {
            id: "tunnel".into(),
            args: vec![
                helper.to_string_lossy().into_owned(),
                "--dev".into(),
                "tunnel".into(),
                self.root.to_string_lossy().into_owned(),
            ],
            working_dir: self.root.to_string_lossy().into_owned(),
            env: home_env(),
            restart: true,
        };
        let fresh = self.engine.start_job_policy(&spec, 5, &|| false)?;
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if self.running_pid().is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                if fresh {
                    let _ = self.engine.stop_job("tunnel", &|| false);
                }
                return Err(TunnelLifecycleError::Invalid(
                    "the tunnel job did not start".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stop the job and publish `Stopped`.
    pub fn stop(&self) -> Result<(), TunnelLifecycleError> {
        self.engine.stop_job("tunnel", &|| false)?;
        let _ = crate::tunnel::config::write_status(
            &self.root,
            crate::tunnel::config::TunnelStatusSnapshot::default(),
        );
        Ok(())
    }

    /// Restart the job, preserving the published snapshot.
    pub fn restart(&self) -> Result<(), TunnelLifecycleError> {
        self.engine.stop_job("tunnel", &|| false)?;
        self.start()
    }
}

impl TunnelJobControl for TunnelJobs {
    fn stop(&self) -> Result<(), TunnelError> {
        TunnelJobs::stop(self).map_err(|error| TunnelError::Job(error.to_string()))
    }

    fn is_loaded(&self) -> bool {
        self.loaded()
    }
}

fn home_env() -> std::collections::BTreeMap<String, String> {
    let mut env = std::collections::BTreeMap::new();
    if let Some(home) = std::env::var_os("HOME") {
        env.insert("HOME".into(), home.to_string_lossy().into_owned());
    }
    env
}
