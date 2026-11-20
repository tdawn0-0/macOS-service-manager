//! Injected operating-system interaction.
//!
//! The Go package swapped the package-level `system.Run` in tests. This port
//! carries the runner (and the host and privileged helper) in
//! [`super::Context`] instead, so tests never spawn a real process and never
//! touch the real LaunchAgents directories or Trash.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::error::LaunchdError;

/// The two-minute deadline `internal/system.Run` gives every command.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// The text `system.Run` puts in a timeout message.
pub const RUN_TIMEOUT_LABEL: &str = "2m0s";

/// How long `system.Run` waits for inherited pipes after the program exits.
pub const WAIT_DELAY: Duration = Duration::from_secs(1);

/// A failed process run, shaped after `system.Run`'s error reporting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub program: String,
    pub args: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub status: Option<i32>,
    pub timed_out: bool,
}

impl CommandError {
    /// Build an error for a fake runner without stdout or stderr.
    pub fn new(program: impl Into<String>, args: Vec<String>, message: impl Into<String>) -> Self {
        CommandError {
            program: program.into(),
            args,
            stdout: String::new(),
            stderr: message.into(),
            status: None,
            timed_out: false,
        }
    }

    /// The message `system.Run` returns: the timeout notice, then trimmed
    /// stderr, then trimmed stdout, then the program and its status.
    pub fn message(&self) -> String {
        if self.timed_out {
            return format!("{} did not answer in {RUN_TIMEOUT_LABEL}", self.program);
        }
        let stderr = self.stderr.trim();
        if !stderr.is_empty() {
            return stderr.to_string();
        }
        let stdout = self.stdout.trim();
        if !stdout.is_empty() {
            return stdout.to_string();
        }
        match self.status {
            Some(code) => format!("{}: exit status {code}", self.program),
            None => format!("{}: exited with an error", self.program),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for CommandError {}

/// Runs a program with the given arguments.
pub trait ProcessRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError>;
}

/// The real runner: no shell, stdin closed, stdout and stderr captured
/// separately, and the two-minute deadline of `internal/system.Run`.
#[derive(Debug, Clone, Default)]
pub struct SystemRunner;

impl SystemRunner {
    pub fn new() -> Self {
        SystemRunner
    }
}

impl ProcessRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PATH", child_path());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Err(CommandError {
                    program: program.to_string(),
                    args: args.iter().map(|a| (*a).to_string()).collect(),
                    stdout: String::new(),
                    stderr: error.to_string(),
                    status: None,
                    timed_out: false,
                });
            }
        };
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

        let deadline = Instant::now() + RUN_TIMEOUT;
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
        // A child of the program may hold the pipes open; the Go runner waits
        // one second after exit and then stops waiting.
        let stdout = out_rx.recv_timeout(WAIT_DELAY).unwrap_or_default();
        let stderr = err_rx.recv_timeout(WAIT_DELAY).unwrap_or_default();
        let command_error = CommandError {
            program: program.to_string(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            status: status.and_then(exit_code),
            timed_out,
        };
        match status {
            Some(status) if status.success() && !timed_out => {
                Ok(String::from_utf8_lossy(&stdout).into_owned())
            }
            _ => Err(command_error),
        }
    }
}

fn read_all(mut pipe: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer);
    buffer
}

fn exit_code(status: ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.code().or_else(|| status.signal())
    }
    #[cfg(not(unix))]
    {
        status.code()
    }
}

fn child_path() -> OsString {
    let mut paths = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(existing) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&existing) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).unwrap_or_default()
}

/// Everything the launchd port needs from the machine besides running
/// commands: who the user is, where their home and temp directories are, the
/// running executable, and the Trash.
pub trait Host: Send + Sync {
    fn uid(&self) -> u32;
    fn home_dir(&self) -> Result<PathBuf, LaunchdError>;
    fn temp_dir(&self) -> PathBuf;
    fn current_exe(&self) -> Result<PathBuf, LaunchdError>;
    fn eval_symlinks(&self, path: &Path) -> Option<PathBuf>;
    /// Moves a file to the user's Trash, including collision renaming. Tests
    /// use a fake.
    fn trash(&self, path: &Path) -> Result<(), LaunchdError>;
}

/// The real host.
#[derive(Debug, Clone, Default)]
pub struct SystemHost;

impl SystemHost {
    pub fn new() -> Self {
        SystemHost
    }
}

impl Host for SystemHost {
    fn uid(&self) -> u32 {
        #[cfg(unix)]
        {
            // SAFETY: getuid has no preconditions and cannot fail.
            unsafe { libc::getuid() }
        }
        #[cfg(not(unix))]
        {
            0
        }
    }

    fn home_dir(&self) -> Result<PathBuf, LaunchdError> {
        std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| LaunchdError::io("the home directory is not known"))
    }

    fn temp_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }

    fn current_exe(&self) -> Result<PathBuf, LaunchdError> {
        std::env::current_exe().map_err(|error| LaunchdError::io(error.to_string()))
    }

    fn eval_symlinks(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path)
            .ok()
            .filter(|p| !p.as_os_str().is_empty())
    }

    fn trash(&self, path: &Path) -> Result<(), LaunchdError> {
        let home = self.home_dir()?;
        let trash = home.join(".Trash");
        let name = path
            .file_name()
            .ok_or_else(|| LaunchdError::io("the file has no name"))?;
        let mut destination = trash.join(name);
        if std::fs::symlink_metadata(&destination).is_ok() {
            destination = with_time_suffix(&destination, &local_hms());
        }
        std::fs::rename(path, &destination).map_err(|error| LaunchdError::io(error.to_string()))
    }
}

/// Inserts `" HH.MM.SS"` before the extension, as `system.Trash` does.
fn with_time_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy();
    let (stem, extension) = match name.rfind('.') {
        Some(index) => name.split_at(index),
        None => (name.as_ref(), ""),
    };
    let file = format!("{stem} {suffix}{extension}");
    match path.parent() {
        Some(parent) => parent.join(file),
        None => PathBuf::from(file),
    }
}

#[cfg(unix)]
fn local_hms() -> String {
    // SAFETY: `t` is a valid time value and `tm` is a properly sized buffer.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "00.00.00".to_string();
        }
        format!("{:02}.{:02}.{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

#[cfg(not(unix))]
fn local_hms() -> String {
    "00.00.00".to_string()
}

/// A save request: the property list, where the helper writes it, and whether
/// launchd loads it afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchdSaveRequest {
    pub plist: Vec<u8>,
    pub path: PathBuf,
    pub domain: String,
    pub target: String,
    pub existing_path: Option<PathBuf>,
    pub load: bool,
}

/// The helper's answer: the file is in place, and loading may have failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchdSaveResponse {
    pub saved: bool,
    pub error: Option<String>,
}

/// A root removal: unload the job if launchd has it, then move its file into
/// `trash_dir` for the user to trash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchdRemoveRequest {
    pub path: PathBuf,
    pub target: String,
    pub trash_dir: PathBuf,
}

/// The operations this crate needs from the privileged helper.
pub trait HelperApi: Send + Sync {
    /// Runs each launchctl command in one request, stopping at the first
    /// failure. Each command is the launchctl arguments, without the program.
    fn launchd_control(&self, commands: &[Vec<String>]) -> Result<(), LaunchdError>;
    fn launchd_save(
        &self,
        request: LaunchdSaveRequest,
    ) -> Result<LaunchdSaveResponse, LaunchdError>;
    fn launchd_remove(&self, request: LaunchdRemoveRequest) -> Result<(), LaunchdError>;
}

/// Reaches the privileged helper. `require` returns
/// [`LaunchdError::helper_not_installed`] when it is absent: there is no
/// password-prompt fallback.
pub trait PrivilegedHelper: Send + Sync {
    fn require(&self) -> Result<Arc<dyn HelperApi>, LaunchdError>;
}

/// The state before the helper is installed. The real client is a later
/// phase; this crate only defines the contract.
#[derive(Debug, Clone, Default)]
pub struct UnavailableHelper;

impl PrivilegedHelper for UnavailableHelper {
    fn require(&self) -> Result<Arc<dyn HelperApi>, LaunchdError> {
        Err(LaunchdError::helper_not_installed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_error_message_follows_system_run() {
        let mut error = CommandError {
            program: "launchctl".into(),
            args: vec!["bootout".into()],
            stdout: "  \n".into(),
            stderr: "  Bootout failed: 36\n".into(),
            status: Some(36),
            timed_out: false,
        };
        assert_eq!(error.message(), "Bootout failed: 36");
        error.stderr.clear();
        error.stdout = "  printed reason\n".into();
        assert_eq!(error.message(), "printed reason");
        error.stdout.clear();
        assert_eq!(error.message(), "launchctl: exit status 36");
        error.timed_out = true;
        assert_eq!(error.message(), "launchctl did not answer in 2m0s");
    }

    #[test]
    fn trash_suffix_goes_before_the_extension() {
        assert_eq!(
            with_time_suffix(Path::new("/home/u/.Trash/x.plist"), "12.34.56"),
            PathBuf::from("/home/u/.Trash/x 12.34.56.plist")
        );
        assert_eq!(
            with_time_suffix(Path::new("/home/u/.Trash/x"), "12.34.56"),
            PathBuf::from("/home/u/.Trash/x 12.34.56")
        );
    }
}
