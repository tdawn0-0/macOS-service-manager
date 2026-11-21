//! Instance lifecycle, ported from `database.go:60-206`.
//!
//! `Start`/`Stop`/`Restart`/`Remove` all hold the
//! `database instance:<id>` resource lease for the whole operation; a start
//! also holds the shared package lease and re-reads state before touching the
//! process. Removal unregisters the instance and deletes only its generated
//! job plist — never any data directory.

use std::collections::BTreeMap;

use crate::engine::JobSpec;
use crate::health::{ServiceProbe, wait_service};
use crate::model::{Instance, State};
use crate::state::{ResourceKey, Tx};

use super::locations::{config_directory, database_data_directory, verify_data_location};
use super::{Cancelled, DatabaseError, Databases};

/// The action a database row offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceAction {
    Start,
    Stop,
    Restart,
    Remove,
}

impl InstanceAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "start" => Some(InstanceAction::Start),
            "stop" => Some(InstanceAction::Stop),
            "restart" => Some(InstanceAction::Restart),
            "remove" => Some(InstanceAction::Remove),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            InstanceAction::Start => "start",
            InstanceAction::Stop => "stop",
            InstanceAction::Restart => "restart",
            InstanceAction::Remove => "remove",
        }
    }
}

impl Databases {
    /// `InstanceAction`: the Go lock order is instance lease, package shared
    /// lease (start/restart), state re-read, process change, state write.
    pub fn instance_action(
        &self,
        id: &str,
        action: InstanceAction,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        self.init()?;
        let _instance_lease = self
            .store()
            .resource_leases_cancellable(&[ResourceKey::new("database instance", id)], cancelled)?;
        let mut state = self.store().read()?;
        let instance = state
            .instances
            .get(id)
            .cloned()
            .ok_or_else(|| DatabaseError::invalid("instance not found"))?;

        let _package_lease = if matches!(action, InstanceAction::Start | InstanceAction::Restart) {
            let lease =
                self.store()
                    .package_lease_cancellable(&instance.package, false, cancelled)?;
            state = self.store().read()?;
            match state.instances.get(id) {
                Some(current) if current == &instance => {}
                _ => {
                    return Err(DatabaseError::invalid(
                        "database settings changed; retry with the current instance",
                    ));
                }
            }
            Some(lease)
        } else {
            None
        };

        self.check_instance_action(&state, id, action, cancelled)?;

        if action != InstanceAction::Start {
            self.store().emit(&format!("Stopping database {id}"));
            let job = format!("db-{id}");
            self.engine().stop_job(&job, cancelled)?;
            verify_shutdown_target(self.engine(), &self.engine().target(&job), cancelled)?;
        }

        match action {
            InstanceAction::Start | InstanceAction::Restart => {
                self.store().emit(&format!("Starting database {id}"));
                if cancelled() {
                    return Err(DatabaseError::Cancelled);
                }
                let data_dir = database_data_directory(self.store().root(), &instance, None);
                let config_dir = config_directory(self.store().root(), &instance, None);
                let working_dir = self.store().root().join("instances").join(&instance.id);
                self.start_instance_job(
                    &state,
                    &instance,
                    &format!("db-{id}"),
                    &working_dir,
                    &data_dir,
                    &config_dir,
                    cancelled,
                )?;
                Ok(())
            }
            InstanceAction::Remove => {
                self.store()
                    .with_state_transaction_cancellable(cancelled, |latest| {
                        match latest.instances.get(id) {
                            Some(current) if current == &instance => {}
                            _ => {
                                return Err(crate::state::StateError::invalid(
                                    "database settings changed before removal; refresh and retry",
                                ));
                            }
                        }
                        self.check_instance_action(latest, id, action, cancelled)
                            .map_err(crate::state::StateError::from)?;
                        let plist = self.engine().job_path(&format!("db-{id}"));
                        match std::fs::remove_file(&plist) {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => return Err(error.into()),
                        }
                        latest.instances.remove(id);
                        remove_instance_profiles(latest, id);
                        Ok(Tx::new(()))
                    })?;
                Ok(())
            }
            InstanceAction::Stop => Ok(()),
        }
    }

    /// Change the loopback port of a stopped database instance.
    pub fn update_instance_port(
        &self,
        id: &str,
        new_port: i64,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        if !(1024..=65535).contains(&new_port) {
            return Err(DatabaseError::invalid(
                "locked database port must be 1024–65535",
            ));
        }
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        self.init()?;
        let _instance_lease = self
            .store()
            .resource_leases_cancellable(&[ResourceKey::new("database instance", id)], cancelled)?;
        let job_id = format!("db-{id}");
        self.store()
            .with_state_transaction_cancellable(cancelled, |state| {
                let Some(instance) = state.instances.get(id).cloned() else {
                    return Err(crate::state::StateError::invalid("instance not found"));
                };
                if instance.port == new_port {
                    return Ok(Tx::new(()));
                }
                verify_shutdown_target(self.engine(), &self.engine().target(&job_id), cancelled)
                    .map_err(|error| {
                        crate::state::StateError::invalid(format!(
                            "stop the database before changing its port: {error}"
                        ))
                    })?;
                let mut reserved = super::ports::AvailablePorts::from_state(state).reserved;
                reserved.remove(&instance.port);
                if reserved.contains(&new_port) {
                    return Err(crate::state::StateError::invalid(format!(
                        "port {new_port} is already assigned to another Devbase service"
                    )));
                }
                super::ports::free_database_port(self.binder(), new_port)
                    .map_err(crate::state::StateError::from)?;
                let mut updated = instance.clone();
                updated.port = new_port;
                let config_dir = config_directory(self.store().root(), &updated, None);
                let kind = updated.kind.clone();
                let job_path = self.engine().job_path(&job_id);
                state.instances.insert(id.to_string(), updated);
                Ok(Tx::new(()).with_commit(move || {
                    let _ = update_engine_config_port(&kind, &config_dir, new_port);
                    let _ = std::fs::remove_file(&job_path);
                }))
            })?;
        Ok(())
    }

    fn check_instance_action(
        &self,
        state: &State,
        id: &str,
        action: InstanceAction,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if matches!(action, InstanceAction::Start | InstanceAction::Restart) {
            let instance = state
                .instances
                .get(id)
                .ok_or_else(|| DatabaseError::invalid("instance not found"))?;
            verify_data_location(self.store(), instance)?;
        }
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        Ok(())
    }

    /// Start (or restart) one managed job and wait for its TCP readiness.
    /// `working_dir`/`data_dir`/`config_dir` differ during initialization,
    /// where the job runs against the staging directory.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_instance_job(
        &self,
        state: &State,
        instance: &Instance,
        job: &str,
        working_dir: &std::path::Path,
        data_dir: &std::path::Path,
        config_dir: &std::path::Path,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        verify_data_location(self.store(), instance)?;
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let already_running = matches!(
            self.engine().job_state(job),
            Ok(Some(state)) if state.running
        );
        if !already_running {
            super::ports::free_database_port(self.binder(), instance.port)?;
        }
        let args = self.instance_start_arguments(state, instance, data_dir, config_dir)?;
        let mut env = BTreeMap::new();
        if instance.kind == "postgresql" {
            env.insert("LANG".to_string(), "C".to_string());
            env.insert("LC_ALL".to_string(), "C".to_string());
        }
        let spec = JobSpec {
            id: job.to_string(),
            args,
            working_dir: working_dir.display().to_string(),
            env,
            restart: false,
        };
        let fresh = self.engine().start_job(&spec, cancelled)?;
        let probe = ServiceProbe::new(
            instance.id.clone(),
            format!("tcp://127.0.0.1:{}", instance.port),
        );
        if let Err(error) = wait_service(self.engine(), self.probe(), job, &probe, cancelled) {
            if fresh {
                let _ = self.engine().stop_job(job, &|| false);
            }
            return Err(DatabaseError::Engine(error));
        }
        Ok(())
    }

    fn instance_start_arguments(
        &self,
        state: &State,
        instance: &Instance,
        data_dir: &std::path::Path,
        config_dir: &std::path::Path,
    ) -> Result<Vec<String>, DatabaseError> {
        let port = instance.port.to_string();
        match instance.kind.as_str() {
            "postgresql" => {
                let binary = self
                    .software()
                    .binary(state, &instance.package, "postgres")?;
                Ok(vec![
                    binary.display().to_string(),
                    "-D".to_string(),
                    data_dir.display().to_string(),
                    "-h".to_string(),
                    "127.0.0.1".to_string(),
                    "-p".to_string(),
                    port,
                    "-k".to_string(),
                    String::new(),
                ])
            }
            "mysql" => {
                let binary = self.software().binary(state, &instance.package, "mysqld")?;
                let package = state
                    .packages
                    .get(&instance.package)
                    .ok_or_else(|| DatabaseError::invalid("database software is not installed"))?;
                let basedir = self
                    .software()
                    .store()
                    .package_directory(package)
                    .display()
                    .to_string();
                Ok(vec![
                    binary.display().to_string(),
                    "--no-defaults".to_string(),
                    format!("--basedir={basedir}"),
                    format!("--datadir={}", data_dir.display()),
                    "--bind-address=127.0.0.1".to_string(),
                    format!("--port={port}"),
                    "--mysqlx=OFF".to_string(),
                    format!(
                        "--socket={}",
                        std::path::Path::new("/tmp")
                            .join(format!(
                                "msm-{}-{}.sock",
                                instance.id,
                                crate::software::random_id()
                            ))
                            .display()
                    ),
                ])
            }
            "mongodb" => {
                let binary = self.software().binary(state, &instance.package, "mongod")?;
                Ok(vec![
                    binary.display().to_string(),
                    "--dbpath".to_string(),
                    data_dir.display().to_string(),
                    "--bind_ip".to_string(),
                    "127.0.0.1".to_string(),
                    "--port".to_string(),
                    port,
                    "--auth".to_string(),
                ])
            }
            "redis" => {
                let binary = self
                    .software()
                    .binary(state, &instance.package, "redis-server")?;
                Ok(vec![
                    binary.display().to_string(),
                    config_dir.join("valkey.conf").display().to_string(),
                ])
            }
            _ => Err(DatabaseError::invalid("unsupported database engine")),
        }
    }
}

/// Remove every connection profile that belongs to `instance_id`.
pub(crate) fn remove_instance_profiles(state: &mut State, instance_id: &str) {
    state
        .connection_profiles
        .retain(|_, profile| profile.instance_id != instance_id);
}

fn update_engine_config_port(
    kind: &str,
    config_dir: &std::path::Path,
    new_port: i64,
) -> std::io::Result<()> {
    match kind {
        "redis" => {
            let path = config_dir.join("valkey.conf");
            if let Ok(contents) = std::fs::read_to_string(&path) {
                let mut replaced = false;
                let mut lines: Vec<String> = contents
                    .lines()
                    .map(|line| {
                        if line.trim_start().starts_with("port ") {
                            replaced = true;
                            format!("port {new_port}")
                        } else {
                            line.to_string()
                        }
                    })
                    .collect();
                if !replaced {
                    lines.push(format!("port {new_port}"));
                }
                let updated = format!("{}\n", lines.join("\n"));
                crate::fs::atomic_write(&path, updated.as_bytes(), crate::fs::PRIVATE_FILE)?;
            }
        }
        "mysql" => {
            let path = config_dir.join("client.cnf");
            if let Ok(contents) = std::fs::read_to_string(&path) {
                let mut replaced = false;
                let mut lines: Vec<String> = contents
                    .lines()
                    .map(|line| {
                        if line.trim_start().starts_with("port=") {
                            replaced = true;
                            format!("port={new_port}")
                        } else {
                            line.to_string()
                        }
                    })
                    .collect();
                if !replaced {
                    lines.push(format!("port={new_port}"));
                }
                let updated = format!("{}\n", lines.join("\n"));
                crate::fs::atomic_write(&path, updated.as_bytes(), crate::fs::PRIVATE_FILE)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Verify that managed job `id` is definitively stopped/absent through the
/// active platform [`crate::engine::JobBackend`].
///
/// An observation failure is never proof that a job has stopped.
pub fn verify_shutdown_job(
    engine: &crate::engine::Engine,
    id: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), DatabaseError> {
    if cancelled() {
        return Err(DatabaseError::Cancelled);
    }
    let target = engine.target(id);
    match engine.verify_stopped(id) {
        Ok(true) => Ok(()),
        Ok(false) => Err(DatabaseError::invalid(format!(
            "managed service is still registered: {target}"
        ))),
        Err(crate::engine::EngineError::Timeout(_)) => Err(DatabaseError::invalid(format!(
            "could not verify stopped service: {target}"
        ))),
        Err(error) => Err(DatabaseError::invalid(format!(
            "could not verify stopped service {target}: {error}"
        ))),
    }
}

/// Verify that a managed service target (`<domain>/<label>` or bare job ID) is
/// definitively stopped/absent.
///
/// Accepts the legacy `engine.target(id)` string as well as a bare job ID and
/// delegates through [`Engine::verify_stopped`] so both macOS `launchd` and
/// Linux `systemd --user` work uniformly.
pub fn verify_shutdown_target(
    engine: &crate::engine::Engine,
    target: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), DatabaseError> {
    let prefix = format!("{}/{}", engine.domain(), engine.label(""));
    let job_id = target.strip_prefix(&prefix).unwrap_or(target);
    verify_shutdown_job(engine, job_id, cancelled)
}

#[cfg(test)]
mod systemd_lifecycle_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::engine::{CommandRunner, Engine, SystemdUserBackend};
    use crate::launchd::CommandError;

    struct FakeSystemctl {
        active: Arc<AtomicBool>,
    }

    impl CommandRunner for FakeSystemctl {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _timeout: Duration,
        ) -> Result<String, CommandError> {
            assert_eq!(program, "systemctl");
            assert!(args.starts_with(&["--user", "show"]));
            if self.active.load(Ordering::SeqCst) {
                Ok(
                    "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=4242\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n"
                        .into(),
                )
            } else {
                Ok(
                    "LoadState=not-found\nActiveState=inactive\nSubState=dead\nMainPID=0\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n"
                        .into(),
                )
            }
        }
    }

    #[test]
    fn verify_shutdown_target_works_with_systemd_user_backend() {
        let dir = TempDir::new().unwrap();
        let active = Arc::new(AtomicBool::new(true));
        let engine = Engine::new(
            dir.path(),
            Arc::new(FakeSystemctl {
                active: active.clone(),
            }),
        )
        .with_backend(Arc::new(SystemdUserBackend::with_unit_dir(
            dir.path().join("systemd-user"),
        )));

        let target = engine.target("db-pg1");
        assert!(verify_shutdown_target(&engine, &target, &|| false).is_err());

        active.store(false, Ordering::SeqCst);
        assert!(verify_shutdown_target(&engine, &target, &|| false).is_ok());
    }
}
