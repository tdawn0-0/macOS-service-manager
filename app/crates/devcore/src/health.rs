//! Job-state parsing, readiness probes and the service wait loop, ported from
//! the Go `health.go`.
//!
//! Launch registration is not readiness. [`wait_service`] requires a stable
//! main process for 200 ms plus a passing readiness check, and fails fast when
//! launchd reports an exit code or signal. Probes are injected through
//! [`HealthProbe`]; [`NetworkProbe`] implements the ordinary TCP and HTTP
//! checks, and tests use a fake.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use url::Url;

use crate::engine::{Engine, EngineError};

/// How long [`wait_health`] retries a failing probe.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// One bounded probe attempt.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// The interval between readiness attempts.
pub const HEALTH_POLL: Duration = Duration::from_millis(200);

/// The process must keep the same PID for this long before a health check
/// counts, so a fast-exiting command cannot look ready by accident.
pub const STABLE_PID: Duration = Duration::from_millis(200);

/// The interval between launchd state observations in [`wait_service`].
pub const READINESS_POLL: Duration = Duration::from_millis(100);

/// The default readiness timeout when the service names none.
pub const DEFAULT_READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// A readiness check, as a managed service declares it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceProbe {
    pub id: String,
    /// Empty or `process`, `tcp://host:port`, or `http(s)://host/path`.
    pub health: String,
    /// Zero means the default 30 seconds.
    pub readiness_timeout_seconds: i64,
}

impl ServiceProbe {
    pub fn new(id: impl Into<String>, health: impl Into<String>) -> Self {
        ServiceProbe {
            id: id.into(),
            health: health.into(),
            readiness_timeout_seconds: 0,
        }
    }

    pub fn readiness_timeout(&self) -> Duration {
        if self.readiness_timeout_seconds <= 0 {
            DEFAULT_READINESS_TIMEOUT
        } else {
            Duration::from_secs(self.readiness_timeout_seconds as u64)
        }
    }
}

/// The injected readiness probe.
pub trait HealthProbe: Send + Sync {
    fn probe(&self, address: &str, timeout: Duration) -> Result<(), String>;
}

/// The real probe: a TCP connect for `tcp://`, a plain HTTP request for
/// `http://`, and the system `curl` for `https://` (no TLS stack is linked).
#[derive(Debug, Clone, Default)]
pub struct NetworkProbe;

impl HealthProbe for NetworkProbe {
    fn probe(&self, address: &str, timeout: Duration) -> Result<(), String> {
        if let Some(rest) = address.strip_prefix("tcp://") {
            return tcp_probe(rest, timeout);
        }
        let url = Url::parse(address).map_err(|_| "invalid readiness request".to_string())?;
        match url.scheme() {
            "http" => http_probe(&url, timeout),
            "https" => https_probe(address, timeout),
            _ => Err(
                "readiness check must be process, tcp://host:port or http(s)://host/path"
                    .to_string(),
            ),
        }
    }
}

fn tcp_probe(address: &str, timeout: Duration) -> Result<(), String> {
    let mut addresses = address
        .to_socket_addrs()
        .map_err(|_| "readiness request did not complete".to_string())?;
    let mut last_error = "readiness request did not complete".to_string();
    for address in addresses.by_ref() {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(_) => return Ok(()),
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(last_error)
}

fn http_probe(url: &Url, timeout: Duration) -> Result<(), String> {
    let host = url
        .host_str()
        .ok_or_else(|| "readiness request did not complete".to_string())?;
    let port = url.port_or_known_default().unwrap_or(80);
    let mut addresses = (host, port)
        .to_socket_addrs()
        .map_err(|_| "readiness request did not complete".to_string())?;
    let address = addresses
        .next()
        .ok_or_else(|| "readiness request did not complete".to_string())?;
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|_| "HTTP readiness request did not complete".to_string())?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let mut path = url.path().to_string();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let host_header = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|_| "HTTP readiness request did not complete".to_string())?;
    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&buffer[..n]);
                if response.len() >= 4096 || response.windows(4).any(|window| window == b"\r\n\r\n")
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let status = String::from_utf8_lossy(&response)
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "HTTP readiness request did not complete".to_string())?;
    if status >= 400 {
        return Err(format!("HTTP {status}"));
    }
    Ok(())
}

fn https_probe(address: &str, timeout: Duration) -> Result<(), String> {
    // Go uses net/http with the system trust store. This crate links no TLS
    // client, so the system curl performs the same bounded request. Its output
    // is never surfaced: the error text is fixed, like the HTTP probe.
    let curl = crate::platform::resolve_curl();
    let result = std::process::Command::new(curl)
        .args([
            "-fsS",
            "--max-time",
            &format_max_time(timeout),
            "-o",
            "/dev/null",
            address,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => Ok(()),
        _ => Err("HTTP readiness request did not complete".to_string()),
    }
}

fn format_max_time(timeout: Duration) -> String {
    format!("{:.3}", timeout.as_secs_f64())
}

/// Validate a readiness address with the same accepted shapes and messages as
/// the Go validator.
pub fn validate_health(address: &str) -> Result<(), EngineError> {
    if address.is_empty() || address == "process" {
        return Ok(());
    }
    let parse_failed = || {
        EngineError::Invalid(
            "readiness check must be process, tcp://host:port or http(s)://host/path".into(),
        )
    };
    let Some(rest) = address.split_once("://") else {
        return Err(parse_failed());
    };
    let authority = rest.1.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(parse_failed());
    }
    let url = Url::parse(address).map_err(|_| parse_failed())?;
    if url.host_str().unwrap_or_default().is_empty()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(parse_failed());
    }
    match url.scheme() {
        "tcp" => {
            let port = url.port().unwrap_or(0);
            if port < 1
                || !url.path().is_empty()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(EngineError::Invalid(
                    "TCP readiness requires a host and port from 1–65535".into(),
                ));
            }
            Ok(())
        }
        "http" | "https" => {
            if let Some(port) = url.port()
                && !(1..=65535).contains(&port)
            {
                return Err(EngineError::Invalid("invalid HTTP readiness port".into()));
            }
            Ok(())
        }
        _ => Err(parse_failed()),
    }
}

/// One bounded readiness attempt. It does not imply that the process which
/// should own the address is alive; service callers also check launchd.
pub fn probe_health(probe: &dyn HealthProbe, address: &str) -> Result<(), EngineError> {
    validate_health(address)?;
    if address.is_empty() || address == "process" {
        return Ok(());
    }
    probe
        .probe(address, PROBE_TIMEOUT)
        .map_err(EngineError::Invalid)
}

/// Retry a readiness probe for up to 30 seconds.
pub fn wait_health(
    probe: &dyn HealthProbe,
    address: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), EngineError> {
    wait_health_timeout(probe, address, HEALTH_TIMEOUT, cancelled)
}

/// [`wait_health`] with an explicit deadline.
pub fn wait_health_timeout(
    probe: &dyn HealthProbe,
    address: &str,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), EngineError> {
    validate_health(address)?;
    let deadline = Instant::now() + timeout;
    loop {
        if cancelled() {
            return Err(EngineError::Invalid(
                "readiness check did not pass: context canceled".into(),
            ));
        }
        if probe_health(probe, address).is_ok() {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(EngineError::Invalid(
                "readiness check did not pass: context deadline exceeded".into(),
            ));
        }
        std::thread::sleep(HEALTH_POLL.min(remaining));
    }
}

/// The subset of `launchctl print` the readiness logic understands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobState {
    pub running: bool,
    pub pid: i32,
    pub runs: i32,
    pub last_exit_code: Option<i32>,
    /// The numeric signal, never arbitrary launchctl text.
    pub last_signal: String,
    pub exit_reason: bool,
}

impl JobState {
    /// The job ran and is now gone, with a recorded reason.
    pub fn finished(&self) -> bool {
        !self.running
            && self.pid == 0
            && self.runs > 0
            && (self.last_exit_code.is_some() || !self.last_signal.is_empty() || self.exit_reason)
    }

    /// The job ran and exited successfully.
    pub fn successful_exit(&self) -> bool {
        self.finished()
            && self.last_exit_code == Some(0)
            && self.last_signal.is_empty()
            && !self.exit_reason
    }
}

/// Parse the job properties at the top level of a `launchctl print` output.
/// Nested resource coalitions with their own `state`/`pid` keys are ignored by
/// indentation, exactly as in Go.
pub fn parse_job_state(output: &str) -> JobState {
    let mut state = JobState::default();
    let mut property_indent: Option<usize> = None;
    for line in output.split('\n') {
        let trimmed = line.trim();
        let Some((key, value)) = trimmed.split_once(" = ") else {
            continue;
        };
        if !matches!(
            key,
            "state"
                | "pid"
                | "runs"
                | "last exit code"
                | "last terminating signal"
                | "last exit reason"
        ) {
            continue;
        }
        let indent = line
            .chars()
            .take_while(|character| *character == ' ' || *character == '\t')
            .count();
        let property_indent = *property_indent.get_or_insert(indent);
        if indent != property_indent {
            continue;
        }
        match key {
            "state" => state.running = value == "running",
            "pid" => state.pid = value.parse().unwrap_or(0),
            "runs" => state.runs = value.parse().unwrap_or(0),
            "last exit code" => state.last_exit_code = value.parse().ok(),
            "last terminating signal" => {
                for field in value.split_whitespace() {
                    let cleaned = field.trim_matches(|character| "():,".contains(character));
                    if let Ok(signal) = cleaned.parse::<i32>()
                        && signal > 0
                    {
                        state.last_signal = signal.to_string();
                        break;
                    }
                }
            }
            "last exit reason" => state.exit_reason = !value.is_empty(),
            _ => {}
        }
    }
    state
}

/// Observe a job with a one-second deadline through [`Engine::job_state`].
///
/// The parsed state is returned when loaded; absent jobs return an error
/// containing `"Could not find service"` so existing callers keep matching.
pub fn read_job_state(
    engine: &Engine,
    id: &str,
    cancelled: &dyn Fn() -> bool,
) -> (JobState, Result<(), EngineError>) {
    if cancelled() {
        return (JobState::default(), Err(EngineError::Cancelled));
    }
    match engine.job_state(id) {
        Ok(Some(runtime)) => (runtime.to_job_state(), Ok(())),
        Ok(None) => (
            JobState::default(),
            Err(EngineError::Invalid(format!(
                "Could not find service: {}",
                engine.target(id)
            ))),
        ),
        Err(error) => (JobState::default(), Err(error)),
    }
}

/// Wait until a job has a stable main process and its readiness check passes.
///
/// A job that records an exit code or terminating signal fails immediately,
/// even if some other process answers the readiness address.
pub fn wait_service(
    engine: &Engine,
    probe: &dyn HealthProbe,
    job: &str,
    service: &ServiceProbe,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), EngineError> {
    let deadline = Instant::now() + service.readiness_timeout();
    let not_ready = |cause: &str| {
        EngineError::Invalid(format!(
            "service {} did not become ready: {cause}",
            service.id
        ))
    };
    let mut since: Option<Instant> = None;
    let mut pid = 0;
    loop {
        if cancelled() {
            return Err(not_ready("context canceled"));
        }
        let (state, error) = read_job_state(engine, job, cancelled);
        if error.is_ok() && state.running && state.pid > 0 {
            if pid != state.pid || since.is_none() {
                pid = state.pid;
                since = Some(Instant::now());
            }
            if since.is_some_and(|started| started.elapsed() >= STABLE_PID)
                && probe_health(probe, &service.health).is_ok()
            {
                let (after, after_error) = read_job_state(engine, job, cancelled);
                if after_error.is_ok() && after.running && after.pid == pid {
                    return Ok(());
                }
            }
        } else {
            pid = 0;
            since = None;
            if error.is_ok() {
                if let Some(code) = state.last_exit_code {
                    return Err(EngineError::Invalid(format!(
                        "service {} exited with code {code} before becoming ready; read its logs",
                        service.id
                    )));
                }
                if !state.last_signal.is_empty() {
                    return Err(EngineError::Invalid(format!(
                        "service {} terminated by signal {} before becoming ready; read its logs",
                        service.id, state.last_signal
                    )));
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(not_ready("context deadline exceeded"));
        }
        std::thread::sleep(READINESS_POLL.min(remaining));
    }
}
