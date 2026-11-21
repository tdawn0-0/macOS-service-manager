//! Systemd helper unit generation and Linux privileged helper install/uninstall scripts.
//!
//! On Linux the privileged helper runs via systemd socket activation:
//! - `/etc/systemd/system/devbase-helper.socket` listens on `/run/devbase-helper.sock`
//! - `/etc/systemd/system/devbase-helper.service` runs `/usr/libexec/devbase/devbase-helper --dev helper --owner-uid <uid>`
//!
//! For privileged HTTPS proxy ports (`< 1024`), the helper also installs
//! per-workspace `devbase-proxy-<prefix>.socket` + `.service` units that bind
//! loopback TCP sockets as root and pass them via `sd_listen_fds` to the
//! unprivileged user proxy process.

use std::path::{Path, PathBuf};

use super::paths::shell_quote_path;
use super::socket_job::{SOCKET_ACTIVATION_ENV, SocketJobError};
use crate::engine::label_prefix;
use crate::systemd::quote_systemd_arg;

/// Root-owned staging directory for the Linux privileged helper binary.
pub const LINUX_HELPER_DIR: &str = "/usr/libexec/devbase";
/// Staged Linux helper executable path.
pub const LINUX_HELPER_BIN: &str = "/usr/libexec/devbase/devbase-helper";
/// Systemd-activated Unix domain socket path on Linux.
pub const LINUX_HELPER_SOCKET: &str = "/run/devbase-helper.sock";
/// Systemd system unit directory.
pub const LINUX_SYSTEMD_DIR: &str = "/etc/systemd/system";
/// Systemd socket unit name for the helper.
pub const LINUX_HELPER_SOCKET_UNIT_NAME: &str = "devbase-helper.socket";
/// Systemd service unit name for the helper.
pub const LINUX_HELPER_SERVICE_UNIT_NAME: &str = "devbase-helper.service";
/// Full path to the helper `.socket` unit.
pub const LINUX_HELPER_SOCKET_UNIT: &str = "/etc/systemd/system/devbase-helper.socket";
/// Full path to the helper `.service` unit.
pub const LINUX_HELPER_SERVICE_UNIT: &str = "/etc/systemd/system/devbase-helper.service";

/// Renders `/etc/systemd/system/devbase-helper.socket`.
pub fn build_helper_socket_unit(socket_path: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Devbase Privileged Helper Socket\n\
         \n\
         [Socket]\n\
         ListenStream={socket_path}\n\
         SocketMode=0666\n\
         FileDescriptorName={}\n\
         \n\
         [Install]\n\
         WantedBy=sockets.target\n",
        super::SOCKET_NAME
    )
}

/// Renders `/etc/systemd/system/devbase-helper.service`.
pub fn build_helper_service_unit(staged_exe: &Path, owner_uid: u32) -> Result<String, String> {
    let staged = staged_exe
        .to_str()
        .ok_or_else(|| "staged helper path is not valid UTF-8".to_string())?;
    if !staged_exe.is_absolute() {
        return Err("staged helper path must be absolute".to_string());
    }
    Ok(format!(
        "[Unit]\n\
         Description=Devbase Privileged Helper\n\
         Requires={LINUX_HELPER_SOCKET_UNIT_NAME}\n\
         After={LINUX_HELPER_SOCKET_UNIT_NAME}\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} --dev helper --owner-uid {owner_uid}\n\
         StandardError=journal\n",
        quote_systemd_arg(staged)
    ))
}

/// Parsed fields inspected passively from `devbase-helper.service` + `.socket`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxHelperUnitDoc {
    pub exec_start: Vec<String>,
    pub listen_stream: String,
}

/// Passively parses a helper `.service` and `.socket` pair for status checks.
pub fn parse_linux_helper_units(service_raw: &str, socket_raw: &str) -> LinuxHelperUnitDoc {
    let mut doc = LinuxHelperUnitDoc::default();
    if let Ok(spec) = crate::systemd::parse_unit(LINUX_HELPER_SERVICE_UNIT_NAME, service_raw) {
        doc.exec_start = spec.exec_start;
    }
    for line in socket_raw.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("ListenStream=") {
            doc.listen_stream = value.trim().to_string();
        }
    }
    doc
}

/// Generated Linux systemd socket-activation unit pair for the local HTTPS proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxProxySocketUnits {
    pub socket_unit_name: String,
    pub service_unit_name: String,
    pub socket_path: PathBuf,
    pub service_path: PathBuf,
    pub socket_content: String,
    pub service_content: String,
}

/// The systemd unit stem for a workspace root's proxy socket daemon.
pub fn linux_proxy_unit_stem(root: &Path) -> String {
    format!("devbase-proxy-{}", label_prefix(root))
}

/// Builds `devbase-proxy-<prefix>.socket` and `devbase-proxy-<prefix>.service`
/// so systemd binds loopback TCP ports `<1024` as root and passes them via
/// `sd_listen_fds` to `devbase --dev serve <root>` running as `username`.
pub fn build_linux_proxy_socket_units(
    root: &Path,
    unit_dir: Option<&Path>,
    executable: &Path,
    username: &str,
    ports: &[i64],
    home: &str,
) -> Result<LinuxProxySocketUnits, SocketJobError> {
    if username.is_empty() || username == "root" {
        return Err(SocketJobError(
            "the development proxy must run as a normal user".to_string(),
        ));
    }
    if !executable.is_absolute() {
        return Err(SocketJobError(
            "proxy executable must be absolute".to_string(),
        ));
    }
    let executable_str = executable
        .to_str()
        .ok_or_else(|| SocketJobError("proxy executable path is not valid UTF-8".to_string()))?;
    let root_str = root
        .to_str()
        .ok_or_else(|| SocketJobError("workspace root path is not valid UTF-8".to_string()))?;

    let mut unique = Vec::new();
    for &port in ports {
        if !(1..=65535).contains(&port) {
            return Err(SocketJobError(format!(
                "proxy listener port {port} is not between 1 and 65535"
            )));
        }
        if !unique.contains(&port) {
            unique.push(port);
        }
    }
    unique.sort_unstable();
    if unique.is_empty() {
        return Err(SocketJobError(
            "proxy socket unit requires at least one listener port".to_string(),
        ));
    }

    let stem = linux_proxy_unit_stem(root);
    let socket_unit_name = format!("{stem}.socket");
    let service_unit_name = format!("{stem}.service");
    let dir = unit_dir.unwrap_or_else(|| Path::new(LINUX_SYSTEMD_DIR));

    let mut listen_lines = String::new();
    for port in &unique {
        listen_lines.push_str(&format!("ListenStream=127.0.0.1:{port}\n"));
    }
    let socket_content = format!(
        "[Unit]\n\
         Description=Devbase Local HTTPS Proxy Sockets\n\
         \n\
         [Socket]\n\
         {listen_lines}\
         ReusePort=true\n\
         Service={service_unit_name}\n\
         \n\
         [Install]\n\
         WantedBy=sockets.target\n"
    );

    let service_content = format!(
        "[Unit]\n\
         Description=Devbase Local HTTPS Proxy\n\
         Requires={socket_unit_name}\n\
         After={socket_unit_name}\n\
         \n\
         [Service]\n\
         Type=simple\n\
         User={username}\n\
         WorkingDirectory={root_str}\n\
         Environment=\"HOME={home}\"\n\
         Environment=\"{SOCKET_ACTIVATION_ENV}=1\"\n\
         ExecStart={} --dev serve {}\n\
         StandardOutput=null\n\
         StandardError=journal\n",
        quote_systemd_arg(executable_str),
        quote_systemd_arg(root_str),
    );

    Ok(LinuxProxySocketUnits {
        socket_path: dir.join(&socket_unit_name),
        service_path: dir.join(&service_unit_name),
        socket_unit_name,
        service_unit_name,
        socket_content,
        service_content,
    })
}

/// Builds the single root shell script that installs the Linux systemd helper
/// through one `pkexec` prompt.
#[allow(clippy::too_many_arguments)]
pub fn install_linux_helper_script(
    source_exe: &Path,
    helper_dir: &Path,
    staged_exe: &Path,
    socket_unit_path: &Path,
    service_unit_path: &Path,
    tmp_socket_unit: &Path,
    tmp_service_unit: &Path,
    socket_path: &Path,
) -> String {
    let socket_name = socket_unit_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(LINUX_HELPER_SOCKET_UNIT_NAME);
    let service_name = service_unit_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(LINUX_HELPER_SERVICE_UNIT_NAME);
    let quoted_socket = shell_quote_path(socket_path);
    [
        "set -eu".to_string(),
        format!("systemctl stop {service_name} {socket_name} >/dev/null 2>&1 || true"),
        format!("rm -f {quoted_socket}"),
        format!("install -d -m 0755 -o root -g root {}", shell_quote_path(helper_dir)),
        format!(
            "install -m 0755 -o root -g root {} {}",
            shell_quote_path(source_exe),
            shell_quote_path(staged_exe)
        ),
        format!(
            "install -m 0644 -o root -g root {} {}",
            shell_quote_path(tmp_socket_unit),
            shell_quote_path(socket_unit_path)
        ),
        format!(
            "install -m 0644 -o root -g root {} {}",
            shell_quote_path(tmp_service_unit),
            shell_quote_path(service_unit_path)
        ),
        "systemctl daemon-reload".to_string(),
        format!("systemctl enable --now {socket_name}"),
        format!(
            "i=0; while [ ! -S {quoted_socket} ] && [ \"$i\" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done"
        ),
    ]
    .join("; ")
}

/// Builds the single root shell script that stops and removes the Linux helper.
pub fn uninstall_linux_helper_script(
    socket_unit_path: &Path,
    service_unit_path: &Path,
    helper_dir: &Path,
    socket_path: &Path,
) -> String {
    let socket_name = socket_unit_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(LINUX_HELPER_SOCKET_UNIT_NAME);
    let service_name = service_unit_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(LINUX_HELPER_SERVICE_UNIT_NAME);
    [
        "set -eu".to_string(),
        format!("systemctl disable --now {socket_name} >/dev/null 2>&1 || true"),
        format!("systemctl stop {service_name} >/dev/null 2>&1 || true"),
        format!("rm -f {}", shell_quote_path(socket_unit_path)),
        format!("rm -f {}", shell_quote_path(service_unit_path)),
        format!("rm -rf {}", shell_quote_path(helper_dir)),
        format!("rm -f {}", shell_quote_path(socket_path)),
        "systemctl daemon-reload >/dev/null 2>&1 || true".to_string(),
    ]
    .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_parses_linux_helper_systemd_units() {
        let socket = build_helper_socket_unit(LINUX_HELPER_SOCKET);
        assert!(socket.contains("ListenStream=/run/devbase-helper.sock"));
        assert!(socket.contains("SocketMode=0666"));
        assert!(socket.contains("FileDescriptorName=helper"));

        let service = build_helper_service_unit(Path::new(LINUX_HELPER_BIN), 1000).unwrap();
        assert!(service.contains(
            "ExecStart=/usr/libexec/devbase/devbase-helper --dev helper --owner-uid 1000"
        ));
        assert!(service.contains("Requires=devbase-helper.socket"));

        let parsed = parse_linux_helper_units(&service, &socket);
        assert_eq!(parsed.listen_stream, LINUX_HELPER_SOCKET);
        assert_eq!(
            parsed.exec_start,
            vec![
                "/usr/libexec/devbase/devbase-helper",
                "--dev",
                "helper",
                "--owner-uid",
                "1000"
            ]
        );
    }

    #[test]
    fn builds_linux_proxy_socket_units_with_deduped_sorted_ports() {
        let units = build_linux_proxy_socket_units(
            Path::new("/home/alice/.local/share/devbase"),
            None,
            Path::new("/usr/bin/devbase"),
            "alice",
            &[443, 8443, 443],
            "/home/alice",
        )
        .unwrap();
        assert!(units.socket_unit_name.starts_with("devbase-proxy-"));
        assert!(units.socket_unit_name.ends_with(".socket"));
        assert!(units.service_unit_name.ends_with(".service"));
        assert!(
            units
                .socket_content
                .contains("ListenStream=127.0.0.1:443\nListenStream=127.0.0.1:8443\n")
        );
        assert!(units.service_content.contains("User=alice"));
        assert!(
            units
                .service_content
                .contains("MSM_DEV_SOCKET_ACTIVATION=1")
        );
        assert!(
            units
                .service_content
                .contains("--dev serve /home/alice/.local/share/devbase")
        );

        assert!(
            build_linux_proxy_socket_units(
                Path::new("/tmp/root"),
                None,
                Path::new("/usr/bin/devbase"),
                "root",
                &[443],
                "/root",
            )
            .is_err()
        );
    }
}
