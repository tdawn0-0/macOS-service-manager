//! The privileged helper, ported from Go's `internal/privhelper`.
//!
//! The helper is not a separate daemon protocol: it is the Devbase executable
//! itself, staged as a whole bundle under
//! `/Library/PrivilegedHelperTools/xyz.jyeu.devbase.helper/bundle`, launched by
//! launchd on demand as root, serving HTTP/1.1 over a launchd-activated Unix
//! socket. Authorization is two-layered: the peer's effective UID must equal
//! the installing UID, then `SecCodeCheckValidity` checks the peer's audit
//! token against the helper's own designated requirement. The helper therefore
//! serves only the Devbase build it was installed from.
//!
//! Everything that needs a password goes through [`admin::run_admin`], which
//! runs one command through `osascript` with administrator privileges. Only
//! [`client::Client::install`] and [`client::Client::uninstall`] call it.
//!
//! All machine interaction tests need to fake is injected: the server takes a
//! [`server::CommandRunner`], a [`auth::PeerAuth`] and a [`server::Chowner`];
//! the client takes an [`admin::AdminRunner`]. The real implementations are
//! [`server::SystemCommandRunner`], [`auth::SystemPeerAuth`],
//! [`server::SystemChown`] and [`admin::OsascriptAdmin`].

pub mod activate;
pub mod admin;
pub mod auth;
pub mod client;
pub mod http;
pub mod paths;
pub mod plist_doc;
pub mod protocol;
pub mod server;
pub mod socket_job;
pub mod systemd_unit;
pub mod testenv;
pub mod validate;

pub use activate::{
    LinuxSocketActivator, SD_LISTEN_FDS_START, SocketActivator, SystemSocketActivator,
    parse_sd_listen_fds, run_activated, serve_activated,
};
pub use admin::{AdminError, AdminRunner, OsascriptAdmin, PkexecAdmin, default_admin_runner};
pub use auth::{LinuxPeerAuth, PeerAuth, SystemPeerAuth, verify_peer_exe_hash};
pub use client::{BinaryInfo, Client, ClientConfig, DefaultClientGuard, DefaultHelper};
pub use plist_doc::{
    HelperPlistDoc, app_bundle_for_executable, build_helper_plist, current_binary_info,
    expected_staged_executable, has_owner_arg, parse_helper_plist,
};
pub use protocol::{
    CaInstallRequest, CaRemoveRequest, ErrorResponse, LaunchdControlRequest, LaunchdRemoveRequest,
    LaunchdSaveRequest, LaunchdSaveResponse, LinuxProxySocketInstallRequest,
    LinuxProxySocketStopRequest, OkResponse, PortInspectRequest, PortInspectResponse,
    ProxySocketInstallRequest, ProxySocketKickstartRequest, ProxySocketStopRequest, StatusResponse,
    SystemdControlRequest, SystemdRemoveRequest, SystemdSaveRequest,
};
pub use server::{CaTrustTarget, Server, ServerConfig, detect_linux_ca_trust_target};
pub use systemd_unit::{
    LINUX_HELPER_BIN, LINUX_HELPER_DIR, LINUX_HELPER_SERVICE_UNIT, LINUX_HELPER_SERVICE_UNIT_NAME,
    LINUX_HELPER_SOCKET, LINUX_HELPER_SOCKET_UNIT, LINUX_HELPER_SOCKET_UNIT_NAME,
    LINUX_SYSTEMD_DIR, LinuxProxySocketUnits, build_helper_service_unit, build_helper_socket_unit,
    build_linux_proxy_socket_units, install_linux_helper_script, uninstall_linux_helper_script,
};

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_admin;
#[cfg(test)]
mod tests_auth;
#[cfg(test)]
mod tests_client;
#[cfg(test)]
mod tests_endpoints;
#[cfg(test)]
mod tests_idle;
#[cfg(test)]
mod tests_install;
#[cfg(test)]
mod tests_linux;
#[cfg(test)]
mod tests_paths;
#[cfg(test)]
mod tests_protocol;
#[cfg(test)]
mod tests_socket_job;
#[cfg(test)]
mod tests_status;
#[cfg(test)]
mod tests_testenv;

use std::fmt;

/// The launchd job label for the Devbase privileged helper.
pub const LABEL: &str = "xyz.jyeu.devbase.helper";
/// The system LaunchDaemon property list path for the helper.
pub const PLIST_PATH: &str = "/Library/LaunchDaemons/xyz.jyeu.devbase.helper.plist";
/// The root-owned directory holding the staged helper bundle or binary.
pub const HELPER_DIR: &str = "/Library/PrivilegedHelperTools/xyz.jyeu.devbase.helper";
/// The launchd-managed Unix domain socket path.
pub const SOCKET_PATH: &str = "/var/run/xyz.jyeu.devbase.helper.sock";
/// The Mach/launchd `Sockets` dictionary key.
pub const SOCKET_NAME: &str = "helper";
/// The helper HTTP-over-Unix protocol version.
pub const PROTOCOL_VERSION: i64 = 1;
/// The helper's standard error log, written by launchd.
pub const STDERR_LOG: &str = "/var/log/xyz.jyeu.devbase.helper.log";
/// The largest request body and response read, matching `server.go`.
pub const MAX_REQUEST_BYTES: usize = 4 << 20;

/// The installed helper when it is ready, or [`HelperError::NotInstalled`].
pub fn require() -> Result<std::sync::Arc<Client>, HelperError> {
    Client::require()
}

/// The message callers show when no helper is installed.
pub const NOT_INSTALLED_MESSAGE: &str =
    "privileged helper is not installed; install the helper in Devbase first";

/// What the helper tells a caller whose code it does not trust.
pub const PEER_REJECTED_MESSAGE: &str = "peer is not the Devbase build this helper was installed from; reinstall the helper in Settings";

/// The passive installation state of the privileged helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HelperState {
    NotInstalled,
    Installed,
}

impl HelperState {
    pub const fn as_str(self) -> &'static str {
        match self {
            HelperState::NotInstalled => "not-installed",
            HelperState::Installed => "installed",
        }
    }
}

impl fmt::Display for HelperState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The passive state read without launching the helper or prompting for a
/// password.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HelperStatus {
    #[serde(rename = "State")]
    pub state: HelperState,
    #[serde(rename = "Message")]
    pub message: String,
}

/// A helper client failure, with the two runtime conditions callers branch on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperError {
    /// No helper is installed; root work cannot run.
    NotInstalled,
    /// The installed helper refused this build.
    PeerRejected,
    /// The client is disabled in this environment.
    Disabled,
    /// Any other failure, carrying the message to show.
    Message(String),
}

impl HelperError {
    /// Reports whether the helper must be installed again before it serves this
    /// build, including when the exact rejection text lost its type.
    pub fn needs_reinstall(&self) -> bool {
        match self {
            HelperError::PeerRejected => true,
            HelperError::Message(message) => message.contains(PEER_REJECTED_MESSAGE),
            _ => false,
        }
    }

    pub fn is_peer_rejected(&self) -> bool {
        matches!(self, HelperError::PeerRejected)
    }

    pub fn is_not_installed(&self) -> bool {
        matches!(self, HelperError::NotInstalled)
    }

    pub fn message(&self) -> &str {
        match self {
            HelperError::NotInstalled => NOT_INSTALLED_MESSAGE,
            HelperError::PeerRejected => PEER_REJECTED_MESSAGE,
            HelperError::Disabled => "privileged helper is not enabled",
            HelperError::Message(message) => message,
        }
    }
}

impl fmt::Display for HelperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for HelperError {}

/// `NeedsReinstall` over a message that may or may not carry the typed error.
pub fn needs_reinstall_message(message: &str) -> bool {
    message.contains(PEER_REJECTED_MESSAGE)
}
