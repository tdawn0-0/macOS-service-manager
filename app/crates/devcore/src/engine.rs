//! The launchd execution engine, ported from the Go `engine.go`.
//!
//! Every managed process runs as a launch agent in the current user's
//! `gui/<uid>` domain under a label derived from the workspace root. The label
//! binds a job to one development home, so two roots can run the same instance
//! id without colliding. Commands are injected through [`CommandRunner`];
//! production uses [`SystemCommandRunner`] (five-minute deadline), tests use a
//! fake and never run `launchctl`.
//!
//! [`Engine::start_job`] reuses [`crate::launchd::Spec`] to write the property
//! list atomically. [`Engine::stop_job`] boots the job out and then waits until
//! both the launchd record and the owned PID are gone.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, LazyLock, mpsc};
use std::time::{Duration, Instant};

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::fs::{PRIVATE_FILE, atomic_write, clean_path, create_private_dir_all};
use crate::launchd::{CommandError, LaunchdError, ProcessRunner, Spec};
use crate::state::StateError;

/// The deadline the Go `runCommand` gives a command.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long [`Engine::stop_job`] waits for the job to disappear.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// The deadline of a single `launchctl print` observation.
pub const STATE_TIMEOUT: Duration = Duration::from_secs(1);

/// The interval between launchd observations, matching the Go loops.
pub const POLL_INTERVAL: Duration = Duration::from_millis(100);

static JOB_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,199}$").unwrap());

/// Runs one program with a deadline. The command runner is the test seam; the
/// real implementation never involves a shell.
pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<String, CommandError>;

    /// Run with additional environment entries and optional stdin.
    ///
    /// Database clients need a 0600 password file path in the environment and
    /// SQL on stdin; neither may appear in `argv`. Runners that cannot support
    /// this report an error instead of silently dropping the input.
    fn run_with_input(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
        _timeout: Duration,
    ) -> Result<String, CommandError> {
        let _ = (env, stdin);
        Err(CommandError::new(
            program,
            args.iter().map(|arg| (*arg).to_string()).collect(),
            "command runner does not support environment or input",
        ))
    }
}

/// The real runner: stdin closed, stdout and stderr captured separately, and
/// the caller's deadline enforced by killing the child.
#[derive(Debug, Clone, Default)]
pub struct SystemCommandRunner;

impl SystemCommandRunner {
    pub fn new() -> Self {
        SystemCommandRunner
    }
}

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<String, CommandError> {
        self.run_internal(program, args, &[], None, timeout, false)
    }

    fn run_with_input(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<String, CommandError> {
        self.run_internal(program, args, env, stdin, timeout, true)
    }
}

impl SystemCommandRunner {
    #[allow(clippy::too_many_arguments)]
    fn run_internal(
        &self,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
        timeout: Duration,
        clean_database_environment: bool,
    ) -> Result<String, CommandError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if clean_database_environment {
            // Database tools must not inherit an ambient credential environment.
            for (key, _) in std::env::vars_os() {
                let key = key.to_string_lossy();
                if key.starts_with("PG") || key.starts_with("MYSQL_") {
                    command.env_remove(&*key);
                }
            }
        }
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Err(command_error(
                    program,
                    args,
                    String::new(),
                    error.to_string(),
                    None,
                    false,
                ));
            }
        };
        if let Some(bytes) = stdin {
            use std::io::Write;
            if let Some(mut pipe) = child.stdin.take() {
                let bytes = bytes.to_vec();
                std::mem::drop(std::thread::spawn(move || {
                    let _ = pipe.write_all(&bytes);
                }));
            }
        }
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let (out_tx, out_rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel();
        std::mem::drop(std::thread::spawn(move || {
            let _ = out_tx.send(read_all(stdout));
        }));
        std::mem::drop(std::thread::spawn(move || {
            let _ = err_tx.send(read_all(stderr));
        }));

        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        timed_out = true;
                        let _ = child.kill();
                        break child.wait().ok();
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {
                    let _ = child.kill();
                    break child.wait().ok();
                }
            }
        };
        let stdout = out_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default();
        let stderr = err_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default();
        let stdout = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        match status {
            Some(status) if status.success() && !timed_out => Ok(stdout),
            status => Err(command_error(
                program,
                args,
                stdout,
                stderr,
                status.and_then(exit_code),
                timed_out,
            )),
        }
    }
}

fn command_error(
    program: &str,
    args: &[&str],
    stdout: String,
    stderr: String,
    status: Option<i32>,
    timed_out: bool,
) -> CommandError {
    CommandError {
        program: program.to_string(),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        stdout,
        stderr,
        status,
        timed_out,
    }
}

fn read_all(mut pipe: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer);
    buffer
}

fn exit_code(status: ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.code().or_else(|| status.signal())
}

/// A failed engine action.
#[derive(Debug)]
pub enum EngineError {
    Invalid(String),
    Io(io::Error),
    Launchd(LaunchdError),
    Command(CommandError),
    State(StateError),
    Timeout(String),
    Cancelled,
}

impl EngineError {
    /// The `launchctl` failure this error wraps, if any.
    pub fn command_error(&self) -> Option<&CommandError> {
        match self {
            EngineError::Command(error) => Some(error),
            _ => None,
        }
    }

    fn timed_out(&self) -> bool {
        match self {
            EngineError::Command(error) => error.timed_out,
            EngineError::Timeout(_) => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Invalid(message) => f.write_str(message),
            EngineError::Io(error) => error.fmt(f),
            EngineError::Launchd(error) => error.fmt(f),
            EngineError::Command(error) => error.fmt(f),
            EngineError::State(error) => error.fmt(f),
            EngineError::Timeout(message) => write!(f, "{message}: context deadline exceeded"),
            EngineError::Cancelled => f.write_str("context canceled"),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EngineError::Io(error) => Some(error),
            EngineError::Launchd(error) => Some(error),
            EngineError::Command(error) => Some(error),
            EngineError::State(error) => Some(error),
            EngineError::Invalid(_) | EngineError::Timeout(_) | EngineError::Cancelled => None,
        }
    }
}

impl From<io::Error> for EngineError {
    fn from(error: io::Error) -> Self {
        EngineError::Io(error)
    }
}

impl From<LaunchdError> for EngineError {
    fn from(error: LaunchdError) -> Self {
        EngineError::Launchd(error)
    }
}

impl From<CommandError> for EngineError {
    fn from(error: CommandError) -> Self {
        EngineError::Command(error)
    }
}

impl From<StateError> for EngineError {
    fn from(error: StateError) -> Self {
        EngineError::State(error)
    }
}

impl From<EngineError> for StateError {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::State(error) => error,
            EngineError::Cancelled => StateError::Cancelled,
            EngineError::Io(error) => StateError::Io(error),
            other => StateError::invalid(other.to_string()),
        }
    }
}

/// Called after a job property list is written and before it is bootstrapped.
/// The database slices use this to record job digests.
pub type JobSavedHook = Arc<dyn Fn(&str, &Path) -> Result<(), StateError> + Send + Sync + 'static>;

/// Runtime state observed from the platform job backend (`launchd` or `systemd --user`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobRuntimeState {
    pub loaded: bool,
    pub running: bool,
    pub pid: i32,
    pub runs: i32,
    pub last_exit_status: Option<i32>,
    pub last_signal: String,
    pub exit_reason: bool,
}

impl JobRuntimeState {
    /// Converts this runtime observation into [`crate::health::JobState`].
    pub fn to_job_state(&self) -> crate::health::JobState {
        crate::health::JobState {
            running: self.running,
            pid: self.pid,
            runs: self.runs,
            last_exit_code: self.last_exit_status,
            last_signal: self.last_signal.clone(),
            exit_reason: self.exit_reason,
        }
    }
}

impl From<JobRuntimeState> for crate::health::JobState {
    fn from(value: JobRuntimeState) -> Self {
        value.to_job_state()
    }
}

/// Platform job management backend plugged into [`Engine`].
///
/// macOS uses [`LaunchdBackend`] (`launchctl` + `.plist` files); Linux uses
/// [`SystemdUserBackend`] (`systemctl --user` + `.service`/`.timer` units).
pub trait JobBackend: Send + Sync {
    /// Path to the primary job definition file under `$root/jobs/`.
    fn job_path(&self, engine: &Engine, id: &str) -> PathBuf;

    /// Start (or kickstart) a managed job. Returns `Ok(false)` when already running.
    fn start_job_policy(
        &self,
        engine: &Engine,
        job: &JobSpec,
        throttle: i32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, EngineError>;

    /// Stop a managed job and wait until its record and owned PID are gone.
    fn stop_job(
        &self,
        engine: &Engine,
        id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), EngineError>;

    /// Observe runtime state. `Ok(None)` means the job is definitively absent/stopped.
    fn job_state(&self, engine: &Engine, id: &str) -> Result<Option<JobRuntimeState>, EngineError>;

    /// Verify a job is definitively stopped/absent.
    fn verify_stopped(&self, engine: &Engine, id: &str) -> Result<bool, EngineError> {
        match self.job_state(engine, id)? {
            None => Ok(true),
            Some(state) => Ok(!state.loaded && !state.running),
        }
    }

    /// Write and register a periodic background job (launchd `StartInterval` or
    /// systemd `.service` + `.timer`).
    fn ensure_periodic_job(
        &self,
        engine: &Engine,
        id: &str,
        program_args: &[String],
        interval_secs: u64,
        log_path: &Path,
    ) -> Result<(), EngineError>;

    /// Stop a periodic job and remove its generated job/timer files.
    fn stop_periodic_job(
        &self,
        engine: &Engine,
        id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), EngineError> {
        self.stop_job(engine, id, cancelled)?;
        let path = self.job_path(engine, id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// macOS `launchd` backend: writes `$root/jobs/<label>.plist` and drives `launchctl`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LaunchdBackend;

impl JobBackend for LaunchdBackend {
    fn job_path(&self, engine: &Engine, id: &str) -> PathBuf {
        engine
            .jobs_directory()
            .join(format!("{}.plist", engine.label(id)))
    }

    fn start_job_policy(
        &self,
        engine: &Engine,
        job: &JobSpec,
        throttle: i32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, EngineError> {
        if !JOB_ID_RE.is_match(&job.id) {
            return Err(EngineError::Invalid("invalid job identifier".into()));
        }
        let target = engine.target(&job.id);
        if let Ok(output) = engine.run_command("launchctl", &["print", &target], cancelled) {
            if output.contains("state = running") {
                return Ok(false);
            }
            engine.run_command("launchctl", &["kickstart", &target], cancelled)?;
            return Ok(true);
        }
        if cancelled() {
            return Err(EngineError::Cancelled);
        }

        create_private_dir_all(&engine.jobs_directory())?;
        let path = self.job_path(engine, &job.id);
        let existing = if std::fs::metadata(&path).is_ok() {
            Some(std::fs::read(&path)?)
        } else {
            None
        };
        let mut spec = Spec {
            label: engine.label(&job.id),
            args: job.args.clone(),
            working_dir: job.working_dir.clone(),
            run_at_load: true,
            keep_alive: job.restart,
            env: job.env.clone(),
            stdout: engine
                .root
                .join("logs")
                .join(format!("{}.log", job.id))
                .to_string_lossy()
                .into_owned(),
            stderr: engine
                .root
                .join("logs")
                .join(format!("{}.err.log", job.id))
                .to_string_lossy()
                .into_owned(),
            ..Spec::default()
        };
        if throttle > 0 {
            spec.throttle_interval = Some(throttle);
        }
        spec.validate()?;
        let data = spec.marshal(existing.as_deref())?;
        atomic_write(&path, &data, PRIVATE_FILE)?;
        if let Some(hook) = &engine.job_saved {
            hook(&job.id, &path)?;
        }
        engine.run_command(
            "launchctl",
            &["bootstrap", &engine.domain(), path_str(&path)],
            cancelled,
        )?;
        Ok(true)
    }

    fn stop_job(
        &self,
        engine: &Engine,
        id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), EngineError> {
        if cancelled() {
            return Err(EngineError::Cancelled);
        }
        let deadline = Instant::now() + engine.stop_timeout;
        let target = engine.target(id);
        let output = match engine.run_deadline("launchctl", &["print", &target], deadline) {
            Ok(output) => output,
            Err(error) if error.timed_out() => {
                return Err(EngineError::Timeout(format!("waiting for {id} to stop")));
            }
            Err(EngineError::Command(_)) => return Ok(()),
            Err(other) => return Err(other),
        };
        let pid = crate::health::parse_job_state(&output).pid;
        engine.run_deadline("launchctl", &["bootout", &target], deadline)?;
        loop {
            if cancelled() {
                return Err(EngineError::Cancelled);
            }
            let print = engine.run_deadline("launchctl", &["print", &target], deadline);
            let process_gone = pid == 0 || !process_alive(pid);
            match print {
                Err(EngineError::Command(_)) if process_gone => return Ok(()),
                Err(EngineError::Timeout(_)) => {
                    return Err(EngineError::Timeout(format!("waiting for {id} to stop")));
                }
                _ => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EngineError::Timeout(format!("waiting for {id} to stop")));
            }
            std::thread::sleep(engine.poll.min(remaining));
        }
    }

    fn job_state(&self, engine: &Engine, id: &str) -> Result<Option<JobRuntimeState>, EngineError> {
        let target = engine.target(id);
        match engine.run_with_timeout("launchctl", &["print", &target], engine.state_timeout) {
            Ok(output) => {
                let parsed = crate::health::parse_job_state(&output);
                Ok(Some(JobRuntimeState {
                    loaded: true,
                    running: parsed.running,
                    pid: parsed.pid,
                    runs: parsed.runs,
                    last_exit_status: parsed.last_exit_code,
                    last_signal: parsed.last_signal,
                    exit_reason: parsed.exit_reason,
                }))
            }
            Err(EngineError::Command(error)) if is_launchd_not_found(&error.message()) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn ensure_periodic_job(
        &self,
        engine: &Engine,
        id: &str,
        program_args: &[String],
        interval_secs: u64,
        log_path: &Path,
    ) -> Result<(), EngineError> {
        if !JOB_ID_RE.is_match(id) {
            return Err(EngineError::Invalid("invalid job identifier".into()));
        }
        create_private_dir_all(&engine.jobs_directory())?;
        let path = self.job_path(engine, id);
        let existing = if path.exists() {
            Some(std::fs::read(&path)?)
        } else {
            None
        };
        let log_str = if log_path.as_os_str().is_empty() {
            String::new()
        } else {
            log_path.to_string_lossy().into_owned()
        };
        let spec = Spec {
            label: engine.label(id),
            args: program_args.to_vec(),
            working_dir: engine.root.to_string_lossy().into_owned(),
            start_interval: i32::try_from(interval_secs.max(1)).unwrap_or(i32::MAX),
            run_at_load: true,
            stdout: log_str.clone(),
            stderr: log_str,
            ..Spec::default()
        };
        let data = spec.marshal(existing.as_deref())?;
        atomic_write(&path, &data, PRIVATE_FILE)?;
        if matches!(self.job_state(engine, id), Ok(Some(_))) {
            return Ok(());
        }
        engine.run_command(
            "launchctl",
            &["bootstrap", &engine.domain(), path_str(&path)],
            &|| false,
        )?;
        Ok(())
    }
}

fn is_launchd_not_found(message: &str) -> bool {
    message.contains("Could not find service")
        || message.contains("Could not find specified service")
}

/// Linux `systemd --user` backend: writes `$root/jobs/devbase-<prefix>-<id>.service`
/// (plus `.timer` for periodic jobs), links into the user unit directory, and drives
/// `systemctl --user`.
#[derive(Debug, Clone, Default)]
pub struct SystemdUserBackend {
    /// Override directory where unit symlinks are placed (isolated in tests).
    unit_dir: Option<PathBuf>,
}

impl SystemdUserBackend {
    pub fn new() -> Self {
        SystemdUserBackend { unit_dir: None }
    }

    /// Configures an explicit user systemd unit directory (used in tests to
    /// guarantee isolation from `~/.config/systemd/user`).
    pub fn with_unit_dir(unit_dir: impl Into<PathBuf>) -> Self {
        SystemdUserBackend {
            unit_dir: Some(unit_dir.into()),
        }
    }

    /// The systemd service unit name for `id` in `engine`.
    pub fn unit_name(&self, engine: &Engine, id: &str) -> String {
        format!("devbase-{}-{id}.service", label_prefix(&engine.root))
    }

    /// The systemd timer unit name for `id` in `engine`.
    pub fn timer_name(&self, engine: &Engine, id: &str) -> String {
        format!("devbase-{}-{id}.timer", label_prefix(&engine.root))
    }

    fn effective_unit_dir(&self, engine: &Engine) -> PathBuf {
        if let Some(dir) = &self.unit_dir {
            return dir.clone();
        }
        if cfg!(test) {
            return engine.root.join("systemd-user");
        }
        let home = crate::state::home_from_environment();
        crate::platform::linux::paths::systemd_user_unit_dir(&home)
    }

    fn ensure_unit_link(&self, engine: &Engine, unit_name: &str, target: &Path) -> io::Result<()> {
        let dir = self.effective_unit_dir(engine);
        std::fs::create_dir_all(&dir)?;
        let link_path = dir.join(unit_name);
        if std::fs::symlink_metadata(&link_path).is_ok() {
            let _ = std::fs::remove_file(&link_path);
        }
        std::os::unix::fs::symlink(target, &link_path)
    }

    fn remove_unit_link(&self, engine: &Engine, unit_name: &str) {
        let link_path = self.effective_unit_dir(engine).join(unit_name);
        let _ = std::fs::remove_file(&link_path);
    }

    fn show_unit(
        &self,
        engine: &Engine,
        unit: &str,
        timeout: Duration,
    ) -> Result<Option<JobRuntimeState>, EngineError> {
        let args = [
            "--user",
            "show",
            "--no-pager",
            "-p",
            "LoadState,ActiveState,SubState,MainPID,ExecMainCode,ExecMainStatus,NRestarts",
            unit,
        ];
        match engine.run_with_timeout("systemctl", &args, timeout) {
            Ok(output) => {
                let show = crate::systemd::parse_show(&output);
                if show.load_state == "not-found" {
                    return Ok(None);
                }
                let parsed = crate::systemd::show_to_job_state(&show);
                let cleanly_inactive = show.active_state == "inactive"
                    && show.sub_state == "dead"
                    && show.main_pid == 0
                    && parsed.last_exit_code.is_none()
                    && parsed.last_signal.is_empty()
                    && !parsed.exit_reason;
                if cleanly_inactive {
                    return Ok(None);
                }
                Ok(Some(JobRuntimeState {
                    loaded: true,
                    running: parsed.running,
                    pid: parsed.pid,
                    runs: parsed.runs,
                    last_exit_status: parsed.last_exit_code,
                    last_signal: parsed.last_signal,
                    exit_reason: parsed.exit_reason,
                }))
            }
            Err(EngineError::Command(error)) if is_systemd_not_found(&error.message()) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn is_systemd_not_found(message: &str) -> bool {
    message.contains("could not be found")
        || message.contains("not-found")
        || message.contains("No such unit")
}

impl JobBackend for SystemdUserBackend {
    fn job_path(&self, engine: &Engine, id: &str) -> PathBuf {
        engine.jobs_directory().join(self.unit_name(engine, id))
    }

    fn start_job_policy(
        &self,
        engine: &Engine,
        job: &JobSpec,
        throttle: i32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, EngineError> {
        if !JOB_ID_RE.is_match(&job.id) {
            return Err(EngineError::Invalid("invalid job identifier".into()));
        }
        if cancelled() {
            return Err(EngineError::Cancelled);
        }
        if let Ok(Some(state)) = self.job_state(engine, &job.id)
            && state.running
        {
            return Ok(false);
        }

        create_private_dir_all(&engine.jobs_directory())?;
        create_private_dir_all(&engine.root.join("logs"))?;
        let unit_name = self.unit_name(engine, &job.id);
        let path = self.job_path(engine, &job.id);
        let spec = crate::systemd::UnitSpec {
            name: unit_name.clone(),
            description: format!("Devbase managed job {}", job.id),
            exec_start: job.args.clone(),
            working_directory: job.working_dir.clone(),
            environment: job.env.clone(),
            restart: if job.restart {
                "on-failure".to_string()
            } else {
                "no".to_string()
            },
            restart_sec: if throttle > 0 {
                Some(throttle as u32)
            } else if job.restart {
                Some(2)
            } else {
                None
            },
            wanted_by: Vec::new(),
            user: String::new(),
            group: String::new(),
            stdout_path: engine
                .root
                .join("logs")
                .join(format!("{}.log", job.id))
                .to_string_lossy()
                .into_owned(),
            stderr_path: engine
                .root
                .join("logs")
                .join(format!("{}.err.log", job.id))
                .to_string_lossy()
                .into_owned(),
            on_calendar: String::new(),
            on_unit_active_sec: None,
        };
        spec.validate()
            .map_err(|error| EngineError::Invalid(error.to_string()))?;
        let content = spec.render_unit();
        atomic_write(&path, content.as_bytes(), PRIVATE_FILE)?;
        if let Some(hook) = &engine.job_saved {
            hook(&job.id, &path)?;
        }
        self.ensure_unit_link(engine, &unit_name, &path)?;
        engine.run_command("systemctl", &["--user", "daemon-reload"], cancelled)?;
        engine.run_command("systemctl", &["--user", "start", &unit_name], cancelled)?;
        Ok(true)
    }

    fn stop_job(
        &self,
        engine: &Engine,
        id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), EngineError> {
        if cancelled() {
            return Err(EngineError::Cancelled);
        }
        let deadline = Instant::now() + engine.stop_timeout;
        let unit_name = self.unit_name(engine, id);
        let timer_name = self.timer_name(engine, id);
        let timer_path = engine.jobs_directory().join(&timer_name);

        if timer_path.exists() {
            let _ = engine.run_deadline("systemctl", &["--user", "stop", &timer_name], deadline);
            self.remove_unit_link(engine, &timer_name);
        }

        let initial = match self.show_unit(
            engine,
            &unit_name,
            deadline.saturating_duration_since(Instant::now()),
        ) {
            Ok(Some(state)) => state,
            Ok(None) => {
                self.remove_unit_link(engine, &unit_name);
                return Ok(());
            }
            Err(error) if error.timed_out() => {
                return Err(EngineError::Timeout(format!("waiting for {id} to stop")));
            }
            Err(EngineError::Command(_)) => {
                self.remove_unit_link(engine, &unit_name);
                return Ok(());
            }
            Err(other) => return Err(other),
        };
        let pid = initial.pid;
        engine.run_deadline("systemctl", &["--user", "stop", &unit_name], deadline)?;
        self.remove_unit_link(engine, &unit_name);
        let _ = engine.run_deadline(
            "systemctl",
            &["--user", "reset-failed", &unit_name],
            deadline,
        );
        let _ = engine.run_deadline("systemctl", &["--user", "daemon-reload"], deadline);

        loop {
            if cancelled() {
                return Err(EngineError::Cancelled);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EngineError::Timeout(format!("waiting for {id} to stop")));
            }
            let observed = self.show_unit(engine, &unit_name, remaining);
            let process_gone = pid == 0 || !process_alive(pid);
            match observed {
                Ok(None) if process_gone => return Ok(()),
                Ok(Some(state)) if !state.running && process_gone => return Ok(()),
                Err(EngineError::Timeout(_)) => {
                    return Err(EngineError::Timeout(format!("waiting for {id} to stop")));
                }
                _ => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EngineError::Timeout(format!("waiting for {id} to stop")));
            }
            std::thread::sleep(engine.poll.min(remaining));
        }
    }

    fn job_state(&self, engine: &Engine, id: &str) -> Result<Option<JobRuntimeState>, EngineError> {
        let unit_name = self.unit_name(engine, id);
        self.show_unit(engine, &unit_name, engine.state_timeout)
    }

    fn ensure_periodic_job(
        &self,
        engine: &Engine,
        id: &str,
        program_args: &[String],
        interval_secs: u64,
        log_path: &Path,
    ) -> Result<(), EngineError> {
        if !JOB_ID_RE.is_match(id) {
            return Err(EngineError::Invalid("invalid job identifier".into()));
        }
        create_private_dir_all(&engine.jobs_directory())?;
        let unit_name = self.unit_name(engine, id);
        let timer_name = self.timer_name(engine, id);
        let service_path = self.job_path(engine, id);
        let timer_path = engine.jobs_directory().join(&timer_name);
        let log_str = if log_path.as_os_str().is_empty() {
            String::new()
        } else {
            log_path.to_string_lossy().into_owned()
        };
        let spec = crate::systemd::UnitSpec {
            name: unit_name.clone(),
            description: format!("Devbase periodic job {id}"),
            exec_start: program_args.to_vec(),
            working_directory: engine.root.to_string_lossy().into_owned(),
            stdout_path: log_str.clone(),
            stderr_path: log_str,
            on_unit_active_sec: Some(interval_secs.max(1)),
            ..crate::systemd::UnitSpec::default()
        };
        spec.validate()
            .map_err(|error| EngineError::Invalid(error.to_string()))?;
        atomic_write(&service_path, spec.render_unit().as_bytes(), PRIVATE_FILE)?;
        if let Some(timer_content) = spec.render_timer() {
            atomic_write(&timer_path, timer_content.as_bytes(), PRIVATE_FILE)?;
            self.ensure_unit_link(engine, &timer_name, &timer_path)?;
        }
        self.ensure_unit_link(engine, &unit_name, &service_path)?;
        engine.run_command("systemctl", &["--user", "daemon-reload"], &|| false)?;
        if !matches!(
            self.show_unit(engine, &timer_name, engine.state_timeout),
            Ok(Some(state)) if state.running
        ) {
            engine.run_command("systemctl", &["--user", "start", &timer_name], &|| false)?;
        }
        Ok(())
    }

    fn stop_periodic_job(
        &self,
        engine: &Engine,
        id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), EngineError> {
        self.stop_job(engine, id, cancelled)?;
        for path in [
            self.job_path(engine, id),
            engine.jobs_directory().join(self.timer_name(engine, id)),
        ] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

/// Returns the default platform [`JobBackend`]: [`LaunchdBackend`] on macOS,
/// [`SystemdUserBackend`] on Linux.
pub fn default_job_backend() -> Arc<dyn JobBackend> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(LaunchdBackend)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(SystemdUserBackend::new())
    }
}

/// One development home's managed job engine.
#[derive(Clone)]
pub struct Engine {
    root: PathBuf,
    runner: Arc<dyn CommandRunner>,
    backend: Arc<dyn JobBackend>,
    uid: u32,
    command_timeout: Duration,
    stop_timeout: Duration,
    state_timeout: Duration,
    poll: Duration,
    job_saved: Option<JobSavedHook>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("root", &self.root)
            .field("uid", &self.uid)
            .finish()
    }
}

/// A managed job's arguments and environment, before launchd sees them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JobSpec {
    pub id: String,
    pub args: Vec<String>,
    pub working_dir: String,
    pub env: BTreeMap<String, String>,
    pub restart: bool,
}

impl Engine {
    pub fn new(root: impl Into<PathBuf>, runner: Arc<dyn CommandRunner>) -> Self {
        Engine {
            root: root.into(),
            runner,
            backend: default_job_backend(),
            uid: unsafe { libc::getuid() },
            command_timeout: COMMAND_TIMEOUT,
            stop_timeout: STOP_TIMEOUT,
            state_timeout: STATE_TIMEOUT,
            poll: POLL_INTERVAL,
            job_saved: None,
        }
    }

    /// Replaces the job backend (e.g. to test [`SystemdUserBackend`] or
    /// [`LaunchdBackend`] explicitly).
    pub fn with_backend(mut self, backend: Arc<dyn JobBackend>) -> Self {
        self.backend = backend;
        self
    }

    pub fn with_uid(mut self, uid: u32) -> Self {
        self.uid = uid;
        self
    }

    pub fn with_command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    pub fn with_stop_timeout(mut self, timeout: Duration) -> Self {
        self.stop_timeout = timeout;
        self
    }

    pub fn with_state_timeout(mut self, timeout: Duration) -> Self {
        self.state_timeout = timeout;
        self
    }

    pub fn with_poll_interval(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    pub fn with_job_saved_hook(mut self, hook: JobSavedHook) -> Self {
        self.job_saved = Some(hook);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn uid(&self) -> u32 {
        self.uid
    }

    pub fn command_timeout(&self) -> Duration {
        self.command_timeout
    }

    pub fn stop_timeout(&self) -> Duration {
        self.stop_timeout
    }

    pub fn state_timeout(&self) -> Duration {
        self.state_timeout
    }

    pub fn poll_interval(&self) -> Duration {
        self.poll
    }

    /// The launchd domain managed jobs live in: `gui/<uid>`.
    pub fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    /// The label of a managed job. The first four bytes of the workspace
    /// root's SHA-256 keep jobs from different roots apart.
    pub fn label(&self, id: &str) -> String {
        let digest = Sha256::digest(clean_path(&self.root).as_os_str().as_bytes());
        let prefix = crate::fs::encode_hex(&digest[..4]);
        format!("xyz.jyeu.devbase.dev.{prefix}.{id}")
    }

    /// The `domain/label` target `launchctl` uses.
    pub fn target(&self, id: &str) -> String {
        format!("{}/{}", self.domain(), self.label(id))
    }

    pub fn jobs_directory(&self) -> PathBuf {
        self.root.join("jobs")
    }

    pub fn job_path(&self, id: &str) -> PathBuf {
        self.backend.job_path(self, id)
    }

    /// Observe the runtime state of a managed job.
    ///
    /// Returns `Ok(None)` when the job is definitively absent/stopped,
    /// `Ok(Some(state))` when loaded/running/failed, and `Err(_)` if the job
    /// manager could not be queried.
    pub fn job_state(&self, id: &str) -> Result<Option<JobRuntimeState>, EngineError> {
        self.backend.job_state(self, id)
    }

    /// Verify that a managed job is definitively stopped/absent.
    ///
    /// Returns `Err(_)` when the platform job manager cannot be queried (an
    /// observation failure is never proof that a job has stopped).
    pub fn verify_stopped(&self, id: &str) -> Result<bool, EngineError> {
        self.backend.verify_stopped(self, id)
    }

    /// Write and register a periodic background job (launchd `StartInterval`
    /// on macOS; systemd `.service` + `.timer` on Linux).
    pub fn ensure_periodic_job(
        &self,
        id: &str,
        program_args: &[String],
        interval_secs: u64,
        log_path: &Path,
    ) -> Result<(), EngineError> {
        self.backend
            .ensure_periodic_job(self, id, program_args, interval_secs, log_path)
    }

    /// Stop a periodic background job and remove its generated files.
    pub fn stop_periodic_job(
        &self,
        id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), EngineError> {
        self.backend.stop_periodic_job(self, id, cancelled)
    }

    /// Lists managed job IDs in `$root/jobs/` whose definition files reference
    /// `path` (supports both `.plist` and `.service` definitions).
    pub fn jobs_referencing(&self, path: &Path) -> Result<Vec<String>, EngineError> {
        let dir = self.jobs_directory();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let target_raw = path.to_string_lossy();
        let target_clean = clean_path(path);
        let target_clean_str = target_clean.to_string_lossy();
        let prefix = label_prefix(&self.root);
        let launchd_prefix = format!("xyz.jyeu.devbase.dev.{prefix}.");
        let systemd_prefix = format!("devbase-{prefix}-");

        let mut matched = Vec::new();
        for entry in entries.flatten() {
            let file_path = entry.path();
            if !file_path.is_file() {
                continue;
            }
            let Some(name) = file_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let id = if let Some(rest) = name.strip_prefix(&launchd_prefix)
                && let Some(id) = rest.strip_suffix(".plist")
            {
                id.to_string()
            } else if let Some(rest) = name.strip_prefix(&systemd_prefix)
                && let Some(id) = rest.strip_suffix(".service")
            {
                id.to_string()
            } else {
                continue;
            };
            let Ok(bytes) = std::fs::read(&file_path) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            if (!target_raw.is_empty() && text.contains(target_raw.as_ref()))
                || (!target_clean_str.is_empty() && text.contains(target_clean_str.as_ref()))
            {
                matched.push(id);
            }
        }
        matched.sort();
        matched.dedup();
        Ok(matched)
    }

    /// Run `program` with the five-minute default deadline.
    pub fn run_command(
        &self,
        program: &str,
        args: &[&str],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String, EngineError> {
        if cancelled() {
            return Err(EngineError::Cancelled);
        }
        let result = self
            .runner
            .run(program, args, self.command_timeout)
            .map_err(EngineError::Command);
        if cancelled() {
            return Err(EngineError::Cancelled);
        }
        result
    }

    /// Run a command with an explicit deadline. This is the seam the health
    /// observations and stop loop use.
    pub fn run_with_timeout(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<String, EngineError> {
        match self.runner.run(program, args, timeout) {
            Ok(output) => Ok(output),
            Err(error) if error.timed_out => Err(EngineError::Timeout(format!(
                "{} {}",
                error.program,
                error.args.join(" ")
            ))),
            Err(error) => Err(EngineError::Command(error)),
        }
    }

    /// Start a managed job, or restart an already loaded but stopped one.
    ///
    /// `Ok(false)` means the job was already running and nothing changed.
    pub fn start_job(
        &self,
        spec: &JobSpec,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, EngineError> {
        self.start_job_policy(spec, 0, cancelled)
    }

    /// [`Engine::start_job`] with a custom throttle / restart delay in seconds.
    pub fn start_job_policy(
        &self,
        job: &JobSpec,
        throttle: i32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, EngineError> {
        self.backend
            .start_job_policy(self, job, throttle, cancelled)
    }

    /// Stop a managed job and wait until its job record and owned process
    /// are both gone. An absent job is success.
    pub fn stop_job(&self, id: &str, cancelled: &dyn Fn() -> bool) -> Result<(), EngineError> {
        self.backend.stop_job(self, id, cancelled)
    }

    fn run_deadline(
        &self,
        program: &str,
        args: &[&str],
        deadline: Instant,
    ) -> Result<String, EngineError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(EngineError::Timeout(format!("{} command", program)));
        }
        self.run_with_timeout(program, args, remaining)
    }
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // `kill(pid, 0)` returning ESRCH is the Go check: the process is gone.
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Adapter for the launchd provider seam, so a [`ProcessRunner`] fake can
/// drive the engine. The supplied deadline is enforced by the runner itself.
pub struct ProcessRunnerAdapter {
    runner: Arc<dyn ProcessRunner>,
}

impl ProcessRunnerAdapter {
    pub fn new(runner: Arc<dyn ProcessRunner>) -> Self {
        ProcessRunnerAdapter { runner }
    }
}

impl CommandRunner for ProcessRunnerAdapter {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<String, CommandError> {
        self.runner.run(program, args)
    }
}

/// Whether a path's name is a valid managed job id.
pub fn valid_job_id(id: &str) -> bool {
    JOB_ID_RE.is_match(id)
}

/// The current user's domain without a workspace, used by callers that only
/// need to quote a target.
pub fn current_domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

/// `sha256` prefix used in labels, exposed for tests and diagnostics.
pub fn label_prefix(root: &Path) -> String {
    let digest = Sha256::digest(clean_path(root).as_os_str().as_bytes());
    crate::fs::encode_hex(&digest[..4])
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
