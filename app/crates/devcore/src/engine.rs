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

/// One development home's launchd job engine.
#[derive(Clone)]
pub struct Engine {
    root: PathBuf,
    runner: Arc<dyn CommandRunner>,
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
            uid: unsafe { libc::getuid() },
            command_timeout: COMMAND_TIMEOUT,
            stop_timeout: STOP_TIMEOUT,
            state_timeout: STATE_TIMEOUT,
            poll: POLL_INTERVAL,
            job_saved: None,
        }
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
        self.jobs_directory()
            .join(format!("{}.plist", self.label(id)))
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

    /// [`Engine::start_job`] with a custom `ThrottleInterval`.
    pub fn start_job_policy(
        &self,
        job: &JobSpec,
        throttle: i32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, EngineError> {
        if !JOB_ID_RE.is_match(&job.id) {
            return Err(EngineError::Invalid("invalid job identifier".into()));
        }
        let target = self.target(&job.id);
        if let Ok(output) = self.run_command("launchctl", &["print", &target], cancelled) {
            if output.contains("state = running") {
                return Ok(false);
            }
            self.run_command("launchctl", &["kickstart", &target], cancelled)?;
            return Ok(true);
        }
        if cancelled() {
            return Err(EngineError::Cancelled);
        }

        create_private_dir_all(&self.jobs_directory())?;
        let path = self.job_path(&job.id);
        let existing = if std::fs::metadata(&path).is_ok() {
            Some(std::fs::read(&path)?)
        } else {
            None
        };
        let mut spec = Spec {
            label: self.label(&job.id),
            args: job.args.clone(),
            working_dir: job.working_dir.clone(),
            run_at_load: true,
            keep_alive: job.restart,
            env: job.env.clone(),
            stdout: self
                .root
                .join("logs")
                .join(format!("{}.log", job.id))
                .to_string_lossy()
                .into_owned(),
            stderr: self
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
        if let Some(hook) = &self.job_saved {
            hook(&job.id, &path)?;
        }
        self.run_command(
            "launchctl",
            &["bootstrap", &self.domain(), path_str(&path)],
            cancelled,
        )?;
        Ok(true)
    }

    /// Stop a managed job and wait until its launchd record and owned process
    /// are both gone. An absent job is success.
    pub fn stop_job(&self, id: &str, cancelled: &dyn Fn() -> bool) -> Result<(), EngineError> {
        if cancelled() {
            return Err(EngineError::Cancelled);
        }
        let deadline = Instant::now() + self.stop_timeout;
        let target = self.target(id);
        let output = match self.run_deadline("launchctl", &["print", &target], deadline) {
            Ok(output) => output,
            Err(error) if error.timed_out() => {
                return Err(EngineError::Timeout(format!("waiting for {id} to stop")));
            }
            Err(EngineError::Command(_)) => return Ok(()),
            Err(other) => return Err(other),
        };
        let pid = crate::health::parse_job_state(&output).pid;
        self.run_deadline("launchctl", &["bootout", &target], deadline)?;
        loop {
            if cancelled() {
                return Err(EngineError::Cancelled);
            }
            let print = self.run_deadline("launchctl", &["print", &target], deadline);
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
            std::thread::sleep(self.poll.min(remaining));
        }
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
