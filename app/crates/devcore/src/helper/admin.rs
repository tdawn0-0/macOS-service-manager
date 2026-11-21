//! The single administrator-password prompt.
//!
//! Ported from `admin.go` and the `system.RunFor`/`ShellQuote`/
//! `AppleScriptString` helpers it calls. Only installing and removing the
//! helper itself runs a command as root this way; everything else asks the
//! already-installed helper, so no other module can reach a password prompt.

use std::fmt;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// A command run as root waits for the user to authenticate.
pub const ADMIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How Go prints a five-minute `time.Duration`.
pub const ADMIN_TIMEOUT_LABEL: &str = "5m0s";

/// How long `RunFor` waits for inherited pipes after the program exits.
const WAIT_DELAY: Duration = Duration::from_secs(1);

/// Why an administrator-authorized command failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminError {
    /// macOS reported that the user dismissed the authentication dialog.
    Canceled,
    /// Anything else, already shaped for display.
    Message(String),
}

impl fmt::Display for AdminError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdminError::Canceled => f.write_str("authorization canceled"),
            AdminError::Message(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for AdminError {}

/// Runs a command as root after macOS asks the user to authenticate.
///
/// The return value mirrors Go's `runAdmin`: even a failure carries whatever
/// output the command produced, so install/uninstall can include it.
pub trait AdminRunner: Send + Sync + fmt::Debug {
    fn run_admin(&self, program: &str, args: &[&str]) -> (String, Option<AdminError>);
}

/// The macOS production runner: `osascript -e <do shell script …>` with a five-minute
/// timeout.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsascriptAdmin;

impl AdminRunner for OsascriptAdmin {
    fn run_admin(&self, program: &str, args: &[&str]) -> (String, Option<AdminError>) {
        run_admin(program, args)
    }
}

/// The Linux production runner: `pkexec <program> <args…>` via PolicyKit with a
/// five-minute timeout. Maps exit code 126 to [`AdminError::Canceled`] and exit
/// code 127 to a clear polkit unavailability/not-authorized error.
#[derive(Debug, Clone, Copy, Default)]
pub struct PkexecAdmin;

impl AdminRunner for PkexecAdmin {
    fn run_admin(&self, program: &str, args: &[&str]) -> (String, Option<AdminError>) {
        run_pkexec_admin(program, args)
    }
}

/// Returns the default platform [`AdminRunner`]: [`OsascriptAdmin`] on macOS,
/// [`PkexecAdmin`] on Linux.
pub fn default_admin_runner() -> Arc<dyn AdminRunner> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(OsascriptAdmin)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(PkexecAdmin)
    }
}

/// Runs `program args…` as root through `pkexec`.
pub fn run_pkexec_admin(program: &str, args: &[&str]) -> (String, Option<AdminError>) {
    let mut full = Vec::with_capacity(args.len() + 1);
    full.push(program);
    full.extend_from_slice(args);
    match run_for_status(ADMIN_TIMEOUT, "pkexec", &full, ADMIN_TIMEOUT_LABEL) {
        Ok(stdout) => (stdout, None),
        Err(RunForError::Timeout(message)) => (String::new(), Some(AdminError::Message(message))),
        Err(RunForError::Spawn(message)) => (
            String::new(),
            Some(AdminError::Message(format!(
                "pkexec is unavailable ({message}); install polkit to authorize root actions"
            ))),
        ),
        Err(RunForError::Exit {
            code,
            stdout,
            stderr,
        }) => {
            let err = translate_pkexec_exit(code, &stderr, &stdout);
            (stdout, Some(err))
        }
    }
}

/// Translates `pkexec` exit codes into [`AdminError`].
///
/// Per `pkexec(1)`:
/// - `126`: user dismissed the authentication dialog -> [`AdminError::Canceled`]
/// - `127`: authorization failed or no authentication agent available
pub fn translate_pkexec_exit(code: Option<i32>, stderr: &str, stdout: &str) -> AdminError {
    match code {
        Some(126) => AdminError::Canceled,
        Some(127) => {
            let detail = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else if !stdout.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                "pkexec authorization failed or polkit authentication agent is unavailable"
                    .to_string()
            };
            AdminError::Message(detail)
        }
        other => {
            let trimmed_err = stderr.trim();
            if !trimmed_err.is_empty() {
                return AdminError::Message(trimmed_err.to_string());
            }
            let trimmed_out = stdout.trim();
            if !trimmed_out.is_empty() {
                return AdminError::Message(trimmed_out.to_string());
            }
            let code_label = other
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            AdminError::Message(format!("pkexec: exit status {code_label}"))
        }
    }
}

/// Runs `program args…` as root through one `osascript` administrator prompt.
pub fn run_admin(program: &str, args: &[&str]) -> (String, Option<AdminError>) {
    let script = admin_script(program, args);
    match run_osascript(&script) {
        Ok(output) => (output, None),
        Err(message) => (String::new(), Some(translate_admin_error(message))),
    }
}

/// Turns `osascript`'s cancellation wording into [`AdminError::Canceled`].
pub fn translate_admin_error(message: String) -> AdminError {
    if message.contains("User canceled") {
        AdminError::Canceled
    } else {
        AdminError::Message(message)
    }
}

/// The AppleScript that runs one shell command with administrator privileges.
pub fn admin_script(program: &str, args: &[&str]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(args.len() + 1);
    parts.push(shell_quote(program));
    for argument in args {
        parts.push(shell_quote(argument));
    }
    format!(
        "do shell script {} with administrator privileges",
        apple_script_string(&parts.join(" "))
    )
}

/// Quotes one shell word with single quotes, as `system.ShellQuote` does.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Quotes one AppleScript string literal: backslashes then quotes.
pub fn apple_script_string(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Runs `osascript -e <script>` with the five-minute administrator timeout.
pub fn run_osascript(script: &str) -> Result<String, String> {
    run_for(
        ADMIN_TIMEOUT,
        "osascript",
        &["-e", script],
        ADMIN_TIMEOUT_LABEL,
    )
}

enum RunForError {
    Spawn(String),
    Timeout(String),
    Exit {
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
}

fn run_for_status(
    timeout: Duration,
    program: &str,
    args: &[&str],
    timeout_label: &str,
) -> Result<String, RunForError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| RunForError::Spawn(format!("{program}: {error}")))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut pipe = stdout;
        let _ = pipe.read_to_end(&mut out);
        let _ = out_tx.send(out);
    });
    std::thread::spawn(move || {
        let mut err = Vec::new();
        let mut pipe = stderr;
        let _ = pipe.read_to_end(&mut err);
        let _ = err_tx.send(err);
    });

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(RunForError::Spawn(format!("{program}: {error}"))),
        }
    };
    let out = out_rx.recv_timeout(WAIT_DELAY).unwrap_or_default();
    let err = err_rx.recv_timeout(WAIT_DELAY).unwrap_or_default();
    if timed_out {
        return Err(RunForError::Timeout(format!(
            "{program} did not answer in {timeout_label}"
        )));
    }
    let stdout = String::from_utf8_lossy(&out).into_owned();
    let stderr = String::from_utf8_lossy(&err).into_owned();
    match status {
        Some(status) if status.success() => Ok(stdout),
        Some(status) => Err(RunForError::Exit {
            code: status.code(),
            stdout,
            stderr,
        }),
        None => unreachable!("the timeout path returned already"),
    }
}

/// A faithful port of `system.RunFor`: stdout on success, the trimmed stderr
/// (then stdout) on failure, and a fixed message on timeout.
pub fn run_for(
    timeout: Duration,
    program: &str,
    args: &[&str],
    timeout_label: &str,
) -> Result<String, String> {
    match run_for_status(timeout, program, args, timeout_label) {
        Ok(stdout) => Ok(stdout),
        Err(RunForError::Spawn(message) | RunForError::Timeout(message)) => Err(message),
        Err(RunForError::Exit {
            code,
            stdout,
            stderr,
        }) => {
            let stderr = stderr.trim();
            if !stderr.is_empty() {
                return Err(stderr.to_string());
            }
            let stdout = stdout.trim();
            if !stdout.is_empty() {
                return Err(stdout.to_string());
            }
            let code = code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string());
            Err(format!("{program}: exit status {code}"))
        }
    }
}
