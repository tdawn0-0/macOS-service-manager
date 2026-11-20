//! The proxy socket LaunchDaemon: plist construction and helper calls.
//!
//! Ported from `network.go`'s socket-daemon part. The helper only writes and
//! bootstraps a system LaunchDaemon whose `UserName` is the calling user; the
//! proxy process runs as that user and receives its HTTPS listeners through
//! launchd TCP socket activation. The plist therefore carries
//! `MSM_DEV_SOCKET_ACTIVATION=1`, `HOME`, and one `HTTPS-<port>` socket per
//! listener port.

use std::path::{Path, PathBuf};

use plist::{Dictionary, Value};

use super::HelperError;
use super::client::Client;
use super::protocol::{ProxySocketInstallRequest, ProxySocketStopRequest};
use crate::engine::label_prefix;

/// The managed job id, shared with the engine's label helper.
pub const SOCKET_JOB_ID: &str = "proxy-sockets";
/// Where the socket job property list is written by default.
pub const DEFAULT_SOCKET_JOB_DIR: &str = "/Library/LaunchDaemons";
/// Marks the daemon as launchd socket-activated.
pub const SOCKET_ACTIVATION_ENV: &str = "MSM_DEV_SOCKET_ACTIVATION";

/// A fully built proxy socket job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketJob {
    pub label: String,
    pub path: PathBuf,
    pub target: String,
    pub plist: Vec<u8>,
}

/// Why a proxy socket job could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketJobError(pub String);

impl std::fmt::Display for SocketJobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SocketJobError {}

impl From<HelperError> for SocketJobError {
    fn from(error: HelperError) -> Self {
        SocketJobError(error.to_string())
    }
}

/// The LaunchDaemon label for one workspace root.
pub fn socket_label(root: &Path) -> String {
    format!(
        "xyz.jyeu.devbase.dev.{}.{SOCKET_JOB_ID}",
        label_prefix(root)
    )
}

/// The `system/<label>` target for one workspace root.
pub fn socket_target(root: &Path) -> String {
    format!("system/{}", socket_label(root))
}

/// The property-list path for one workspace root.
pub fn socket_job_path(root: &Path, job_dir: Option<&Path>) -> PathBuf {
    job_dir
        .unwrap_or_else(|| Path::new(DEFAULT_SOCKET_JOB_DIR))
        .join(format!("{}.plist", socket_label(root)))
}

/// Builds the socket daemon plist.
///
/// The proxy must run as a normal user: root's own jobs do not need a
/// user-scoped daemon and the Go builder rejects them.
pub fn build_socket_job(
    root: &Path,
    job_dir: Option<&Path>,
    executable: &Path,
    username: &str,
    ports: &[i64],
    home: &str,
) -> Result<SocketJob, SocketJobError> {
    if username.is_empty() || username == "root" {
        return Err(SocketJobError(
            "the development proxy must run as a normal user".to_string(),
        ));
    }
    // SAFETY: getuid has no preconditions.
    if unsafe { libc::getuid() } == 0 {
        return Err(SocketJobError(
            "the development proxy must run as a normal user".to_string(),
        ));
    }
    if !executable.is_absolute() {
        return Err(SocketJobError(
            "proxy executable must be absolute".to_string(),
        ));
    }
    let executable = executable
        .to_str()
        .ok_or_else(|| SocketJobError("proxy executable path is not valid UTF-8".to_string()))?;
    let root_text = root
        .to_str()
        .ok_or_else(|| SocketJobError("workspace root path is not valid UTF-8".to_string()))?;

    let mut unique: Vec<i64> = Vec::new();
    for port in ports {
        if !(1..=65535).contains(port) {
            return Err(SocketJobError(format!(
                "proxy listener port {port} is not between 1 and 65535"
            )));
        }
        if !unique.contains(port) {
            unique.push(*port);
        }
    }
    unique.sort_unstable();
    let mut sockets = Dictionary::new();
    for port in &unique {
        // Alphabetical order, the order howett/plist writes Go maps in.
        let mut entry = Dictionary::new();
        entry.insert("SockFamily".to_string(), Value::String("IPv4".to_string()));
        entry.insert(
            "SockNodeName".to_string(),
            Value::String("127.0.0.1".to_string()),
        );
        entry.insert("SockPassive".to_string(), Value::Boolean(true));
        entry.insert("SockProtocol".to_string(), Value::String("TCP".to_string()));
        entry.insert(
            "SockServiceName".to_string(),
            Value::String(port.to_string()),
        );
        entry.insert("SockType".to_string(), Value::String("stream".to_string()));
        sockets.insert(format!("HTTPS-{port}"), Value::Dictionary(entry));
    }

    let mut environment = Dictionary::new();
    environment.insert("HOME".to_string(), Value::String(home.to_string()));
    environment.insert(
        SOCKET_ACTIVATION_ENV.to_string(),
        Value::String("1".to_string()),
    );

    let label = socket_label(root);
    let mut doc = Dictionary::new();
    doc.insert(
        "EnvironmentVariables".to_string(),
        Value::Dictionary(environment),
    );
    doc.insert("Label".to_string(), Value::String(label.clone()));
    doc.insert(
        "ProcessType".to_string(),
        Value::String("Background".to_string()),
    );
    doc.insert(
        "ProgramArguments".to_string(),
        Value::Array(vec![
            Value::String(executable.to_string()),
            Value::String("--dev".to_string()),
            Value::String("serve".to_string()),
            Value::String(root_text.to_string()),
        ]),
    );
    doc.insert("RunAtLoad".to_string(), Value::Boolean(true));
    doc.insert("Sockets".to_string(), Value::Dictionary(sockets));
    doc.insert(
        "StandardErrorPath".to_string(),
        Value::String("/dev/null".to_string()),
    );
    doc.insert(
        "StandardOutPath".to_string(),
        Value::String("/dev/null".to_string()),
    );
    doc.insert("UserName".to_string(), Value::String(username.to_string()));
    doc.insert(
        "WorkingDirectory".to_string(),
        Value::String(root_text.to_string()),
    );

    let mut plist = Vec::new();
    Value::Dictionary(doc)
        .to_writer_xml(&mut plist)
        .map_err(|error| SocketJobError(error.to_string()))?;
    Ok(SocketJob {
        label,
        path: socket_job_path(root, job_dir),
        target: socket_target(root),
        plist,
    })
}

/// Whether two property lists describe the same document.
pub fn equivalent_plists(first: &[u8], second: &[u8]) -> bool {
    match (
        plist::Value::from_reader(std::io::Cursor::new(first)),
        plist::Value::from_reader(std::io::Cursor::new(second)),
    ) {
        (Ok(first), Ok(second)) => first == second,
        _ => false,
    }
}

/// Writes the socket daemon plist and bootstraps it through the helper.
pub fn install_socket_job(client: &Client, job: &SocketJob) -> Result<(), SocketJobError> {
    client
        .install_proxy_socket(&ProxySocketInstallRequest {
            plist: job.plist.clone(),
            job_path: path_text(&job.path),
            target: job.target.clone(),
        })
        .map_err(SocketJobError::from)
}

/// Boots out and removes the socket daemon plist through the helper.
pub fn stop_socket_job(client: &Client, job: &SocketJob) -> Result<(), SocketJobError> {
    client
        .stop_proxy_socket(&ProxySocketStopRequest {
            job_path: path_text(&job.path),
            target: job.target.clone(),
        })
        .map_err(SocketJobError::from)
}

/// Restarts the socket daemon through the helper.
pub fn kickstart_socket_job(client: &Client, root: &Path) -> Result<(), SocketJobError> {
    client
        .kickstart_proxy_socket(&socket_target(root))
        .map_err(SocketJobError::from)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
