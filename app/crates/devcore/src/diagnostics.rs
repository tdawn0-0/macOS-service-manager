//! Diagnostics, logs and log rotation, ported from `diagnostics.go`,
//! `diagnostic_report.go`, `logs.go` and `log_rotation.go`.
//!
//! [`Diagnostics::snapshot`] is a bounded observation: a timeout yields
//! `Unknown`, never `Stopped`. [`Diagnostics::diagnose`] ships the Phase 2
//! package/database checks; CA/trust and proxy-layer checks are represented by
//! [`Phase3Probe`], which returns `Unknown` until the Phase 3/4 wiring lands.
//! [`DiagnosticReport::redacted_json`] replaces every resource with a
//! report-local alias and fixed summary/remedy text, so no domain, path or
//! secret can leave the machine.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::engine::{Engine, EngineError};
use crate::health::read_job_state;
use crate::launchd::Spec;
use crate::model::State;
use crate::software::Software;
use crate::state::{StateError, Store};

/// How long a snapshot may take.
pub const SNAPSHOT_BUDGET: Duration = Duration::from_secs(5);

/// How long a diagnosis may take.
pub const DIAGNOSE_BUDGET: Duration = Duration::from_secs(30);

/// The number of simultaneous observations in Go's worker pool.
pub const MAX_WORKERS: usize = 4;

/// A bounded read of one log file.
pub const LOG_TAIL_BYTES: i64 = 64 << 10;
pub const LOG_TAIL_LINES: usize = 400;

/// The rotation threshold and archive size (5 MiB).
pub const LOG_LIMIT: u64 = 5 << 20;

static LOG_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,239}$").unwrap());

/// Everything diagnostics/logs can fail with.
#[derive(Debug)]
pub enum DiagnosticError {
    State(StateError),
    Io(io::Error),
    Json(serde_json::Error),
    Invalid(String),
    Engine(EngineError),
}

impl DiagnosticError {
    pub fn invalid(message: impl Into<String>) -> Self {
        DiagnosticError::Invalid(message.into())
    }
}

impl std::fmt::Display for DiagnosticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticError::State(error) => error.fmt(f),
            DiagnosticError::Io(error) => error.fmt(f),
            DiagnosticError::Json(error) => error.fmt(f),
            DiagnosticError::Invalid(message) => f.write_str(message),
            DiagnosticError::Engine(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DiagnosticError {}

impl From<StateError> for DiagnosticError {
    fn from(error: StateError) -> Self {
        DiagnosticError::State(error)
    }
}

impl From<io::Error> for DiagnosticError {
    fn from(error: io::Error) -> Self {
        DiagnosticError::Io(error)
    }
}

impl From<serde_json::Error> for DiagnosticError {
    fn from(error: serde_json::Error) -> Self {
        DiagnosticError::Json(error)
    }
}

impl From<EngineError> for DiagnosticError {
    fn from(error: EngineError) -> Self {
        DiagnosticError::Engine(error)
    }
}

/// A status map keyed by `instance:<id>`, `proxy`, `tunnel`, `tunnel:token`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub states: BTreeMap<String, String>,
}

/// The connection probe used by diagnostics.
pub trait DialProbe: Send + Sync {
    fn dial(&self, address: &str, timeout: Duration) -> Result<(), String>;
}

/// The real probe: a bounded loopback TCP connect.
#[derive(Debug, Clone, Default)]
pub struct TcpDialProbe;

impl DialProbe for TcpDialProbe {
    fn dial(&self, address: &str, timeout: Duration) -> Result<(), String> {
        use std::net::ToSocketAddrs;
        let addresses = address
            .to_socket_addrs()
            .map_err(|error| error.to_string())?;
        let mut last = "connection did not complete".to_string();
        for address in addresses {
            match std::net::TcpStream::connect_timeout(&address, timeout) {
                Ok(_) => return Ok(()),
                Err(error) => last = error.to_string(),
            }
        }
        Err(last)
    }
}

/// The Phase 3/4 checks that cannot run in Phase 2. Each reports `Unknown` so
/// the UI distinguishes "not observed yet" from "stopped" or "passed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase3Probe {
    Ca,
    Trust,
    Dns,
    Upstream,
    ProxyTls,
    Http,
}

impl Phase3Probe {
    pub const ALL: [Phase3Probe; 6] = [
        Phase3Probe::Ca,
        Phase3Probe::Trust,
        Phase3Probe::Dns,
        Phase3Probe::Upstream,
        Phase3Probe::ProxyTls,
        Phase3Probe::Http,
    ];

    /// The fixed status while the probe is unwired.
    pub fn status(self) -> &'static str {
        "Unknown"
    }

    /// The allowlisted layer name.
    pub fn layer(self) -> &'static str {
        match self {
            Phase3Probe::Ca => "CA",
            Phase3Probe::Trust => "Trust",
            Phase3Probe::Dns => "DNS",
            Phase3Probe::Upstream => "Upstream",
            Phase3Probe::ProxyTls => "Proxy TLS",
            Phase3Probe::Http => "HTTP",
        }
    }

    /// A fixed summary that names no path, domain or command output.
    pub fn summary(self) -> &'static str {
        match self {
            Phase3Probe::Ca | Phase3Probe::Trust => {
                "Local certificate checks are available after HTTPS support is enabled."
            }
            _ => "Proxy checks are available after HTTPS support is enabled.",
        }
    }

    pub fn remedy(self) -> &'static str {
        "Review the related setting in the app."
    }
}

/// One diagnostic check. It deliberately contains no command output,
/// environment, headers, log data or raw errors; `resource` is the only
/// identifying field and is replaced on export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticCheck {
    pub layer: String,
    pub resource: String,
    pub status: String,
    pub summary: String,
    pub remedy: String,
    pub target: DiagnosticTarget,
}

/// UI-only routing for a check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticTarget {
    pub kind: String,
    pub id: String,
}

/// A complete diagnostic report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticReport {
    pub schema_version: i64,
    pub generated_at: String,
    pub redacted: bool,
    pub checks: Vec<DiagnosticCheck>,
    pub proxy_revision: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportedCheck {
    layer: String,
    resource: String,
    status: String,
    summary: String,
    remedy: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportedReport {
    schema_version: i64,
    generated_at: String,
    redacted: bool,
    checks: Vec<ExportedCheck>,
}

fn safe_layer(layer: &str) -> &str {
    match layer {
        "Package" | "Database" | "CA" | "Trust" | "DNS" | "Upstream" | "Proxy TLS" | "HTTP" => {
            layer
        }
        _ => "Other",
    }
}

fn safe_status(status: &str) -> &str {
    match status {
        "OK" | "Warning" | "Failed" | "Stopped" | "Disabled" | "Unknown" => status,
        _ => "Unknown",
    }
}

impl DiagnosticReport {
    /// The export form: resource aliases only, allowlisted layer/status, and
    /// fixed summary/remedy text.
    pub fn redacted_json(&self) -> Result<Vec<u8>, DiagnosticError> {
        let mut aliases: BTreeMap<String, String> = BTreeMap::new();
        let mut checks = Vec::with_capacity(self.checks.len());
        for check in &self.checks {
            let next = aliases.len() + 1;
            let alias = aliases
                .entry(check.resource.clone())
                .or_insert_with(|| format!("resource-{next}"));
            let layer = safe_layer(&check.layer);
            let status = safe_status(&check.status);
            checks.push(ExportedCheck {
                layer: layer.to_string(),
                resource: alias.clone(),
                status: status.to_string(),
                summary: format!("{layer} check: {status}"),
                remedy: "Review the related setting in the app.".to_string(),
            });
        }
        let exported = ExportedReport {
            schema_version: self.schema_version,
            generated_at: self.generated_at.clone(),
            redacted: true,
            checks,
        };
        let mut bytes = serde_json::to_vec_pretty(&exported)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// Diagnostics over one development home.
#[derive(Clone)]
pub struct Diagnostics {
    store: Store,
    engine: Engine,
    software: Software,
    dial: std::sync::Arc<dyn DialProbe>,
    program: PathBuf,
    databases: Option<crate::databases::Databases>,
}

impl std::fmt::Debug for Diagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Diagnostics")
            .field("root", &self.store.root())
            .finish()
    }
}

impl Diagnostics {
    pub fn new(store: Store, engine: Engine, software: Software) -> Self {
        Diagnostics {
            store,
            engine,
            software,
            dial: std::sync::Arc::new(TcpDialProbe),
            program: PathBuf::from("/Applications/Devbase.app/Contents/MacOS/Devbase"),
            databases: None,
        }
    }

    pub fn with_dial_probe(mut self, dial: std::sync::Arc<dyn DialProbe>) -> Self {
        self.dial = dial;
        self
    }

    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// Inject a configured [`crate::databases::Databases`] facade for periodic
    /// scheduled backups (tests pass one wired to fake command/protocol seams).
    pub fn with_databases(mut self, databases: crate::databases::Databases) -> Self {
        self.databases = Some(databases);
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    /// Observe runtime status within five seconds. Timed-out entries are
    /// `Unknown`; `Stopped` only comes from a definite launchd absence.
    pub fn snapshot(&self, state: &State) -> RuntimeSnapshot {
        let deadline = self.now() + SNAPSHOT_BUDGET;
        let mut out = RuntimeSnapshot::default();
        let token_configured = self.has_cloudflare_token();
        if token_configured {
            out.states
                .insert("tunnel:token".into(), "Configured".into());
        }
        for id in state.instances.keys() {
            let key = format!("instance:{id}");
            let status = self.observe_instance(id, deadline);
            out.states.insert(key, status);
        }
        let proxy_plist = self.engine.job_path("proxy");
        let socket_plist = self.engine.job_path("socket");
        let proxy_configured =
            !state.proxy_rules.is_empty() || proxy_plist.exists() || socket_plist.exists();
        if proxy_configured {
            let status = self.observe_service("proxy", true, deadline);
            out.states.insert("proxy".into(), status);
        }
        let tunnel_plist = self.engine.job_path("tunnel");
        let tunnel_configured = token_configured
            || !state.cloudflare_account_id.is_empty()
            || !state.tunnel_workers.is_empty()
            || state.proxy_rules.iter().any(|rule| rule.tunnel)
            || tunnel_plist.exists();
        if tunnel_configured {
            let status = self.observe_service("tunnel", false, deadline);
            out.states.insert("tunnel".into(), status);
        }
        out
    }

    /// Whether a plausible Cloudflare token file exists, never reading it.
    fn has_cloudflare_token(&self) -> bool {
        std::fs::metadata(self.store.root().join("proxy").join("cloudflare.key"))
            .is_ok_and(|metadata| metadata.is_file() && (10..=4096).contains(&metadata.len()))
    }

    fn remaining(&self, deadline: Instant) -> Option<Duration> {
        let remaining = deadline.saturating_duration_since(self.now());
        (!remaining.is_zero()).then_some(remaining)
    }

    fn observe_instance(&self, id: &str, deadline: Instant) -> String {
        if self.remaining(deadline).is_none() {
            return "Unknown".into();
        }
        let (job, error) = read_job_state(&self.engine, &format!("db-{id}"), &|| {
            self.now() >= deadline
        });
        if self.now() >= deadline {
            return "Unknown".into();
        }
        match error {
            Ok(()) => {
                if job.running {
                    "Running".into()
                } else {
                    "Failed".into()
                }
            }
            Err(_) => "Stopped".into(),
        }
    }

    fn observe_service(&self, id: &str, socket: bool, deadline: Instant) -> String {
        if self.remaining(deadline).is_none() {
            return "Unknown".into();
        }
        let (job, error) = read_job_state(&self.engine, id, &|| self.now() >= deadline);
        if self.now() >= deadline {
            return "Unknown".into();
        }
        let socket_loaded = socket && self.socket_loaded();
        match error {
            Ok(()) => {
                if job.running || socket_loaded {
                    "Running".into()
                } else {
                    "Failed".into()
                }
            }
            Err(error) => {
                let message = error.to_string();
                if message.contains("Could not find service")
                    || message.contains("Could not find specified service")
                {
                    if socket_loaded {
                        "Running".into()
                    } else {
                        "Stopped".into()
                    }
                } else {
                    "Unknown".into()
                }
            }
        }
    }

    fn socket_loaded(&self) -> bool {
        read_job_state(&self.engine, "socket", &|| false).1.is_ok()
    }

    /// Phase 2 diagnosis: installed-package verification plus database
    /// launchd/TCP checks. Phase 3 CA/proxy checks report `Unknown` through
    /// [`Phase3Probe`].
    pub fn diagnose(&self) -> Result<DiagnosticReport, DiagnosticError> {
        let state = self.store.read()?;
        let started = Instant::now();
        let mut report = DiagnosticReport {
            schema_version: 1,
            generated_at: crate::software::utc_now_rfc3339(),
            redacted: false,
            checks: Vec::new(),
            proxy_revision: state.proxy_rules_revision,
        };
        let mut add = |layer: &str, resource: &str, status: &str, summary: &str, remedy: &str| {
            let target = match layer {
                "Package" => DiagnosticTarget {
                    kind: "software".into(),
                    id: resource.to_string(),
                },
                "Database" => DiagnosticTarget {
                    kind: "database".into(),
                    id: resource.trim_start_matches("Database ").to_string(),
                },
                "CA" | "Trust" => DiagnosticTarget {
                    kind: "ca".into(),
                    id: String::new(),
                },
                _ => DiagnosticTarget {
                    kind: "proxy".into(),
                    id: resource.to_string(),
                },
            };
            report.checks.push(DiagnosticCheck {
                layer: layer.into(),
                resource: resource.into(),
                status: status.into(),
                summary: summary.into(),
                remedy: remedy.into(),
                target,
            });
        };

        for id in state.packages.keys() {
            if started.elapsed() >= DIAGNOSE_BUDGET {
                break;
            }
            match self.software.verify_package(id, &|| false) {
                Err(_) => add(
                    "Package",
                    id,
                    "Warning",
                    "Installed-file verification did not complete.",
                    "Retry Check files in Software before using this version.",
                ),
                Ok(result) => match result.status.as_str() {
                    "Verified" => add(
                        "Package",
                        id,
                        "OK",
                        "Installed contents, permissions and symlink targets match the recorded installation index.",
                        "",
                    ),
                    "Homebrew managed" => add(
                        "Package",
                        id,
                        "OK",
                        "Homebrew keg receipt and required executables match the selected source and version. Full contents and dependencies are managed by Homebrew.",
                        "",
                    ),
                    "Not recorded" if state.packages[id].formula.is_empty() => add(
                        "Package",
                        id,
                        "Warning",
                        "No trusted installation index is recorded for this version.",
                        "Stop its users, then use Repair in Software to restore from the verified archive.",
                    ),
                    _ => {
                        if state.packages[id].formula.is_empty() {
                            add(
                                "Package",
                                id,
                                "Failed",
                                "Installed files or the installation index need repair.",
                                "Stop its users, then use Repair in Software to restore the same exact version.",
                            )
                        } else {
                            add(
                                "Package",
                                id,
                                "Failed",
                                "The selected Homebrew keg or a required executable is missing or invalid.",
                                "Stop its users and repair with Homebrew, or select an available version in Software. The app will not switch keg versions automatically.",
                            )
                        }
                    }
                },
            }
        }

        for (id, instance) in &state.instances {
            if started.elapsed() >= DIAGNOSE_BUDGET {
                break;
            }
            let resource = format!("Database {id}");
            let loaded = read_job_state(&self.engine, &format!("db-{id}"), &|| false)
                .1
                .is_ok();
            if !loaded {
                add(
                    "Database",
                    &resource,
                    "Stopped",
                    "Database service is not registered in launchd.",
                    "Start the instance in Databases when it is needed.",
                );
                continue;
            }
            let address = format!("127.0.0.1:{}", instance.port);
            if self.dial.dial(&address, Duration::from_secs(1)).is_err() {
                add(
                    "Database",
                    &resource,
                    "Failed",
                    "Database is registered but its loopback port is not reachable.",
                    "Read the instance logs and check for a conflicting port.",
                );
            } else {
                add(
                    "Database",
                    &resource,
                    "OK",
                    "Database loopback port is reachable; authentication was not tested.",
                    "",
                );
            }
        }

        // Phase 3/4 probes: present in the report, but explicitly unobserved.
        add(
            Phase3Probe::Ca.layer(),
            "Local CA",
            Phase3Probe::Ca.status(),
            Phase3Probe::Ca.summary(),
            Phase3Probe::Ca.remedy(),
        );
        add(
            Phase3Probe::Trust.layer(),
            "Local CA",
            Phase3Probe::Trust.status(),
            Phase3Probe::Trust.summary(),
            Phase3Probe::Trust.remedy(),
        );
        for rule in &state.proxy_rules {
            for probe in [
                Phase3Probe::Dns,
                Phase3Probe::Upstream,
                Phase3Probe::ProxyTls,
                Phase3Probe::Http,
            ] {
                add(
                    probe.layer(),
                    &rule.domain,
                    probe.status(),
                    probe.summary(),
                    probe.remedy(),
                );
            }
        }
        Ok(report)
    }

    /// Write a new private, redacted report under `reports/`.
    pub fn export_diagnostics(
        &self,
        report: &DiagnosticReport,
    ) -> Result<PathBuf, DiagnosticError> {
        let bytes = report.redacted_json()?;
        let dir = self.store.root().join("reports");
        crate::fs::create_private_dir_all(&dir)?;
        let path = dir.join(format!("diagnostics-{}.json", random_hex()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(path)
    }
}

fn random_hex() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom")
        && file.read_exact(&mut bytes).is_ok()
    {
        return crate::fs::encode_hex(&bytes);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    crate::fs::encode_hex(&(now ^ u64::from(std::process::id())).to_be_bytes())
}

/// The end of a log file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogSnapshot {
    pub lines: Vec<String>,
    pub size: i64,
    pub truncated: bool,
}

/// Read at most the last `max_lines` lines from the last `max_bytes`.
pub fn tail(path: &Path, max_bytes: i64, max_lines: usize) -> io::Result<LogSnapshot> {
    let mut file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    let mut snapshot = LogSnapshot {
        size: metadata.len() as i64,
        ..LogSnapshot::default()
    };
    let size = metadata.len() as i64;
    let offset = if size > max_bytes {
        size - max_bytes
    } else {
        0
    };
    snapshot.truncated = offset > 0;
    file.seek(SeekFrom::Start(offset as u64))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    if offset > 0
        && let Some(index) = buffer.iter().position(|byte| *byte == b'\n')
    {
        buffer.drain(..=index);
    }
    let mut text = String::from_utf8_lossy(&buffer).into_owned();
    while text.ends_with('\n') {
        text.pop();
    }
    text = text.replace("\r\n", "\n");
    if text.is_empty() {
        return Ok(snapshot);
    }
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
        snapshot.truncated = true;
    }
    snapshot.lines = lines;
    Ok(snapshot)
}

/// The bounded log view for one managed job.
pub fn logs(store: &Store, id: &str) -> Result<String, DiagnosticError> {
    if !LOG_ID_RE.is_match(id) {
        return Err(DiagnosticError::invalid("invalid job ID"));
    }
    let mut out = String::new();
    for suffix in [".log", ".err.log"] {
        let path = store.root().join("logs").join(format!("{id}{suffix}"));
        let snapshot = match tail(&path, LOG_TAIL_BYTES, LOG_TAIL_LINES) {
            Ok(snapshot) => snapshot,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        out.push_str(&format!("{id}{suffix}"));
        if snapshot.truncated {
            out.push_str(" (last 400 lines / 64 KiB)");
        }
        out.push_str(":\n");
        out.push_str(&snapshot.lines.join("\n"));
        out.push_str("\n\n");
    }
    if out.is_empty() {
        return Ok("No logs yet. Start the service to capture its output.".into());
    }
    Ok(out)
}

impl Diagnostics {
    /// Bounded log view for one job.
    pub fn logs(&self, id: &str) -> Result<String, DiagnosticError> {
        logs(&self.store, id)
    }

    /// Whether log rotation is disabled in state.
    pub fn log_rotation_disabled(&self) -> Result<bool, DiagnosticError> {
        Ok(self.store.read()?.log_rotation_disabled)
    }

    /// Toggle log rotation and start or stop the maintenance job.
    pub fn set_log_rotation(&self, enabled: bool) -> Result<(), DiagnosticError> {
        self.store.with_state(|state| {
            state.log_rotation_disabled = !enabled;
            Ok(())
        })?;
        if enabled {
            self.ensure_log_maintenance()
        } else {
            self.stop_log_maintenance()
        }
    }

    fn maintenance_job_path(&self) -> PathBuf {
        self.engine.job_path("maintenance")
    }

    /// Write the scheduled maintenance job and load it if needed.
    pub fn ensure_log_maintenance(&self) -> Result<(), DiagnosticError> {
        if self.store.read()?.log_rotation_disabled {
            return Ok(());
        }
        let path = self.maintenance_job_path();
        let exists = path.exists();
        let spec = Spec {
            label: self.engine.label("maintenance"),
            args: vec![
                self.program.to_string_lossy().into_owned(),
                "--dev".into(),
                "maintain".into(),
                self.store.root().to_string_lossy().into_owned(),
            ],
            working_dir: self.store.root().to_string_lossy().into_owned(),
            start_interval: 60,
            run_at_load: true,
            ..Spec::default()
        };
        let current = if exists {
            Some(std::fs::read(&path)?)
        } else {
            None
        };
        let data = spec
            .marshal(current.as_deref())
            .map_err(|error| DiagnosticError::invalid(error.to_string()))?;
        crate::fs::atomic_write(&path, &data, 0o600)?;
        if read_job_state(&self.engine, "maintenance", &|| false)
            .1
            .is_ok()
        {
            return Ok(());
        }
        self.engine.run_command(
            "launchctl",
            &["bootstrap", &self.engine.domain(), &path.to_string_lossy()],
            &|| false,
        )?;
        Ok(())
    }

    fn stop_log_maintenance(&self) -> Result<(), DiagnosticError> {
        self.engine.stop_job("maintenance", &|| false)?;
        match std::fs::remove_file(self.maintenance_job_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Copy/truncate oversized logs using an exclusive `logs.rotate.lock`.
    /// Concurrent invocations return immediately.
    pub fn rotate_logs(&self) -> Result<(), DiagnosticError> {
        self.store.init()?;
        let lock_path = self.store.root().join("logs.rotate.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Ok(());
            }
            return Err(error.into());
        }
        let logs_dir = self.store.root().join("logs");
        let entries = match std::fs::read_dir(&logs_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() || !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".log") {
                continue;
            }
            rotate_log(&entry.path())
                .map_err(|error| DiagnosticError::invalid(format!("rotate {name}: {error}")))?;
        }
        Ok(())
    }

    /// Rotate oversized logs and execute any due automatic database backups
    /// plus retention pruning (the `--dev maintain` entry point).
    pub fn maintain(&self) -> Result<(), DiagnosticError> {
        self.maintain_at(SystemTime::now())
    }

    /// Periodic maintenance at an explicit wall-clock instant.
    pub fn maintain_at(&self, now: SystemTime) -> Result<(), DiagnosticError> {
        self.rotate_logs()?;
        self.run_scheduled_backups_at(now)
    }

    fn run_scheduled_backups_at(&self, now: SystemTime) -> Result<(), DiagnosticError> {
        self.store.init()?;
        let lock_path = self.store.root().join("backups.maintain.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Ok(());
            }
            return Err(error.into());
        }
        let state = self.store.read()?;
        let due: Vec<(String, crate::model::Instance, u32)> = state
            .instances
            .iter()
            .filter_map(|(id, instance)| {
                let schedule = instance.backup_schedule.as_ref()?;
                if crate::databases::backups::schedule_is_due(schedule, now) {
                    Some((id.clone(), instance.clone(), schedule.keep_last.max(1)))
                } else {
                    None
                }
            })
            .collect();
        if due.is_empty() {
            return Ok(());
        }
        let databases = self
            .databases
            .clone()
            .unwrap_or_else(|| crate::databases::Databases::new(self.store.clone()));
        let stamp = crate::databases::backups::format_rfc3339_utc(now);
        for (id, instance, keep_last) in due {
            let (job, error) = read_job_state(&self.engine, &format!("db-{id}"), &|| false);
            if error.is_err() || !job.running {
                continue;
            }
            let targets = scheduled_backup_targets(&databases, &instance);
            if targets.is_empty() {
                continue;
            }
            let mut backed_up = false;
            for db_name in targets {
                match databases.backup(&id, &db_name, &|| false) {
                    Ok(_) => {
                        backed_up = true;
                    }
                    Err(error) => {
                        self.store.emit(&format!(
                            "Automatic backup failed for {id}/{db_name}: {error}"
                        ));
                    }
                }
            }
            if backed_up {
                if let Err(error) = databases.prune_backups(&id, keep_last, &|| false) {
                    self.store.emit(&format!(
                        "Automatic backup pruning failed for {id}: {error}"
                    ));
                }
                let stamp_copy = stamp.clone();
                let id_copy = id.clone();
                let _ = self.store.with_state(|latest| {
                    if let Some(current) = latest.instances.get_mut(&id_copy)
                        && let Some(schedule) = current.backup_schedule.as_mut()
                    {
                        schedule.last_run_at = Some(stamp_copy);
                    }
                    Ok(())
                });
            }
        }
        Ok(())
    }
}

fn scheduled_backup_targets(
    databases: &crate::databases::Databases,
    instance: &crate::model::Instance,
) -> Vec<String> {
    let catalog = databases
        .inspect_instance_catalog(&instance.id, &|| false)
        .ok();
    let user_dbs: Vec<String> = catalog
        .map(|cat| {
            cat.databases
                .into_iter()
                .filter(|db| {
                    if instance.kind == "redis" {
                        !db.is_empty()
                    } else {
                        !crate::databases::inventory::is_protected_database(db)
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if !user_dbs.is_empty() {
        return user_dbs;
    }
    match instance.kind.as_str() {
        "postgresql" => vec!["postgres".to_string()],
        "redis" => vec!["0".to_string()],
        "mongodb" => vec!["admin".to_string()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;

fn rotate_log(path: &Path) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() <= LOG_LIMIT {
        return Ok(());
    }
    let start = metadata.len() - LOG_LIMIT;
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::with_capacity(LOG_LIMIT as usize);
    file.read_to_end(&mut tail)?;
    let archive = PathBuf::from(format!("{}.1", path.display()));
    let previous = PathBuf::from(format!("{}.2", path.display()));
    match std::fs::rename(&archive, &previous) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    crate::fs::atomic_write(&archive, &tail, 0o600)?;
    file.set_len(0)?;
    Ok(())
}
