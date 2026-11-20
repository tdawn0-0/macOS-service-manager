//! Software management, ported from the Go `catalog.go`, `packages.go`,
//! `package_details.go`, `package_integrity.go`, `package_repair.go`,
//! `package_swap_darwin.go`, `homebrew.go`, `cache.go` and `architecture.go`.
//!
//! The install pipeline is the same as the Go code: download to
//! `downloads/<sha256>`, stage beside the destination, verify (SHA-256,
//! traversal, symlink escape, arm64 slice), publish with
//! `renamex_np(RENAME_SWAP|RENAME_EXCL)`, and register the package in state
//! inside a transaction whose undo restores the previous tree and index.
//! Publication never falls back to a non-atomic replacement.
//!
//! The two test seams are injected: [`CommandRunner`] for `sw_vers`, `brew`
//! and `launchctl`, and [`HttpTransport`] for archives, SHASUMS and metadata.
//! Tests use fakes and never touch the network.

pub mod archive;
pub mod cache;
pub mod catalog;
pub mod details;
pub mod homebrew;
pub mod integrity;
pub mod macho;
pub mod packages;
pub mod repair;
pub mod swap;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::engine::{CommandRunner, SystemCommandRunner};
use crate::launchd::CommandError;
use crate::state::{StateError, Store};

/// A cancellation predicate, matching the `cancelled` closures the engine and
/// shutdown loops already take. Checked at the same points as Go's
/// `context.Context`.
pub type Cancelled<'a> = &'a dyn Fn() -> bool;

/// Everything the software layer can fail with. The Go message fragments are
/// preserved for the UI.
#[derive(Debug)]
pub enum SoftwareError {
    State(StateError),
    Io(io::Error),
    Json(serde_json::Error),
    Invalid(String),
    Command(CommandError),
    Fetch(String),
}

impl SoftwareError {
    pub fn invalid(message: impl Into<String>) -> Self {
        SoftwareError::Invalid(message.into())
    }

    /// Whether another process holds the package or an operation owns it.
    pub fn is_busy(&self) -> bool {
        matches!(self, SoftwareError::State(error) if error.is_busy())
    }
}

impl std::fmt::Display for SoftwareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoftwareError::State(error) => error.fmt(f),
            SoftwareError::Io(error) => error.fmt(f),
            SoftwareError::Json(error) => error.fmt(f),
            SoftwareError::Invalid(message) => f.write_str(message),
            SoftwareError::Command(error) => error.fmt(f),
            SoftwareError::Fetch(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SoftwareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SoftwareError::State(error) => Some(error),
            SoftwareError::Io(error) => Some(error),
            SoftwareError::Json(error) => Some(error),
            SoftwareError::Command(error) => Some(error),
            SoftwareError::Invalid(_) | SoftwareError::Fetch(_) => None,
        }
    }
}

impl From<StateError> for SoftwareError {
    fn from(error: StateError) -> Self {
        SoftwareError::State(error)
    }
}

impl From<io::Error> for SoftwareError {
    fn from(error: io::Error) -> Self {
        SoftwareError::Io(error)
    }
}

impl From<serde_json::Error> for SoftwareError {
    fn from(error: serde_json::Error) -> Self {
        SoftwareError::Json(error)
    }
}

impl From<CommandError> for SoftwareError {
    fn from(error: CommandError) -> Self {
        SoftwareError::Command(error)
    }
}

/// The HTTP seam for archives, Node `SHASUMS256.txt` and metadata probes.
/// Production uses [`CurlTransport`]; tests return fixture bytes.
pub trait HttpTransport: Send + Sync {
    /// Fetch `url` into `destination` and report the number of bytes written.
    fn download(&self, url: &str, destination: &Path) -> Result<u64, SoftwareError>;

    /// Fetch a small metadata document (at most 8 MiB).
    fn get(&self, url: &str) -> Result<Vec<u8>, SoftwareError>;

    /// The archive size from a HEAD request when the server reports it.
    fn size(&self, url: &str) -> Result<Option<i64>, SoftwareError>;
}

/// The real transport: `/usr/bin/curl` with HTTPS-only redirects, a redirect
/// cap, a deadline and the 2 GiB download limit. No TLS stack is linked, which
/// matches [`crate::health`].
#[derive(Debug, Clone, Default)]
pub struct CurlTransport;

impl CurlTransport {
    pub fn new() -> Self {
        CurlTransport
    }
}

fn curl(args: &[&str]) -> Result<std::process::Output, SoftwareError> {
    std::process::Command::new("/usr/bin/curl")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(SoftwareError::Io)
}

impl HttpTransport for CurlTransport {
    fn download(&self, url: &str, destination: &Path) -> Result<u64, SoftwareError> {
        require_https(url)?;
        let destination = destination
            .to_str()
            .ok_or_else(|| SoftwareError::invalid("download destination is not UTF-8"))?;
        let output = curl(&[
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "8",
            "--max-time",
            "1800",
            "--max-filesize",
            "2147483648",
            "--output",
            destination,
            url,
        ])?;
        if !output.status.success() {
            let _ = std::fs::remove_file(destination);
            return Err(SoftwareError::Fetch(format!(
                "download: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(std::fs::metadata(destination)?.len())
    }

    fn get(&self, url: &str) -> Result<Vec<u8>, SoftwareError> {
        require_https(url)?;
        let output = curl(&[
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "8",
            "--max-time",
            "60",
            "--max-filesize",
            "8388608",
            url,
        ])?;
        if !output.status.success() {
            return Err(SoftwareError::Fetch(format!(
                "catalog: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(output.stdout)
    }

    fn size(&self, url: &str) -> Result<Option<i64>, SoftwareError> {
        require_https(url)?;
        let output = curl(&[
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "8",
            "--max-time",
            "15",
            "--head",
            url,
        ])?;
        if !output.status.success() {
            return Ok(None);
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("content-length")
                && let Ok(size) = value.trim().parse::<i64>()
                && size > 0
            {
                return Ok(Some(size));
            }
        }
        Ok(None)
    }
}

fn require_https(url: &str) -> Result<(), SoftwareError> {
    match url::Url::parse(url) {
        Ok(parsed) if parsed.scheme() == "https" => Ok(()),
        _ => Err(SoftwareError::invalid("download URL must use HTTPS")),
    }
}

/// The software manager for one development home.
#[derive(Clone)]
pub struct Software {
    store: Store,
    runner: Arc<dyn CommandRunner>,
    http: Arc<dyn HttpTransport>,
}

impl std::fmt::Debug for Software {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Software")
            .field("root", &self.store.root())
            .finish()
    }
}

impl Software {
    /// Production seams: real commands and `curl`.
    pub fn new(store: Store) -> Self {
        Software {
            store,
            runner: Arc::new(SystemCommandRunner::new()),
            http: Arc::new(CurlTransport::new()),
        }
    }

    pub fn with_runner(mut self, runner: Arc<dyn CommandRunner>) -> Self {
        self.runner = runner;
        self
    }

    pub fn with_http(mut self, http: Arc<dyn HttpTransport>) -> Self {
        self.http = http;
        self
    }

    /// Override the Homebrew Cellar (tests and non-default installs).
    pub fn with_brew_cellar(mut self, cellar: impl Into<PathBuf>) -> Self {
        self.store = self.store.with_brew_cellar(cellar);
        self
    }

    /// Receive the same progress text the Go `Event` sink gets.
    pub fn with_event(mut self, event: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.store = self.store.with_event(event);
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) fn runner(&self) -> &dyn CommandRunner {
        self.runner.as_ref()
    }

    pub(crate) fn http(&self) -> &dyn HttpTransport {
        self.http.as_ref()
    }
}

pub(crate) const MAX_DOWNLOAD: i64 = 2 << 30;
pub(crate) const MAX_EXPANDED: i64 = 8 << 30;
pub(crate) const MAX_INTEGRITY_INDEX: usize = 32 << 20;
pub(crate) const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);
pub(crate) const BREW_PROGRAM: &str = "/opt/homebrew/bin/brew";

/// The integrity index path (`integrity/<id>.json`).
pub(crate) fn integrity_path(store: &Store, id: &str) -> PathBuf {
    store.root().join("integrity").join(format!("{id}.json"))
}

/// A 16-hex random identifier, like the Go `randomID`.
pub(crate) fn random_id() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if file.read_exact(&mut bytes).is_ok() {
            return crate::fs::encode_hex(&bytes);
        }
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    crate::fs::encode_hex(&(now ^ u64::from(std::process::id())).to_be_bytes())
}

/// Go's `versionLess`: three numeric components, missing ones zero, no error
/// for suffixes (`26.0` compares as 26.0.0).
pub(crate) fn version_less(a: &str, b: &str) -> bool {
    fn parts(value: &str) -> [i64; 3] {
        let mut out = [0i64; 3];
        for (index, part) in value.split('.').take(3).enumerate() {
            let digits: String = part
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();
            out[index] = digits.parse::<i64>().unwrap_or(0);
        }
        out
    }
    let a = parts(a);
    let b = parts(b);
    a < b
}

/// RFC 3339 UTC timestamp for integrity results, without a date-time crate.
pub(crate) fn utc_now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// Stream a file's SHA-256, honoring cancellation. `None` when it cannot be
/// read (missing, directory, permission).
pub(crate) fn file_digest(path: &Path, cancelled: Cancelled<'_>) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        if cancelled() {
            return None;
        }
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return None,
        }
    }
    Some(crate::fs::encode_hex(&hasher.finalize()))
}

/// Whether `path` already holds exactly `expected`.
pub(crate) fn has_digest_file(path: &Path, expected: &str, cancelled: Cancelled<'_>) -> bool {
    file_digest(path, cancelled).as_deref() == Some(expected)
}

/// Whether the canonical form of `path` stays inside `root`, resolving every
/// existing symlink prefix (the Go `inside` helper used for archive checks).
pub(crate) fn canonical_inside(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut candidate = path.to_path_buf();
    loop {
        if let Ok(real) = candidate.canonicalize() {
            let mut real = real;
            for part in suffix.iter().rev() {
                real.push(part);
            }
            return crate::fs::inside(&root, &real);
        }
        let Some(name) = candidate.file_name().map(|name| name.to_os_string()) else {
            return false;
        };
        suffix.push(name);
        let Some(parent) = candidate.parent().map(Path::to_path_buf) else {
            return false;
        };
        if parent == candidate {
            return false;
        }
        candidate = parent;
    }
}

#[cfg(test)]
mod tests;
