//! Port inspection, selection and preflight, ported from the Go `ports.go`.
//!
//! Status comes from binding IPv4 loopback, never from a PID or an `lsof`
//! exit code. Listener names and endpoints are a bounded read-only
//! observation. Every command runs through the injected
//! [`crate::engine::CommandRunner`].

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpListener;
use std::time::Duration;

use crate::model::State;

use super::{DatabaseError, Databases};

/// A failed loopback bind, with the OS condition the Go code branches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortBindFailure {
    AddrInUse,
    PermissionDenied,
    Other(String),
}

/// The bind/dial seam (`listenLoopback`/`dialLoopback` in the Go tests).
pub trait PortBinder: Send + Sync {
    /// Bind IPv4 loopback and report the port. The listener is closed before
    /// returning; `0` asks the kernel for an ephemeral port.
    fn bind_loopback(&self, port: u16) -> Result<u16, PortBindFailure>;

    /// Try to connect to IPv4 loopback with the given deadline.
    fn dial_loopback(&self, port: u16, timeout: Duration) -> Result<(), PortBindFailure>;
}

/// The production binder.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemPortBinder;

impl PortBinder for SystemPortBinder {
    fn bind_loopback(&self, port: u16) -> Result<u16, PortBindFailure> {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                let port = listener
                    .local_addr()
                    .map(|address| address.port())
                    .unwrap_or(port);
                Ok(port)
            }
            Err(error) => Err(match error.raw_os_error() {
                Some(libc::EADDRINUSE) => PortBindFailure::AddrInUse,
                Some(libc::EACCES) | Some(libc::EPERM) => PortBindFailure::PermissionDenied,
                _ => PortBindFailure::Other(error.to_string()),
            }),
        }
    }

    fn dial_loopback(&self, port: u16, timeout: Duration) -> Result<(), PortBindFailure> {
        match std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            timeout,
        ) {
            Ok(_) => Ok(()),
            Err(error) => Err(PortBindFailure::Other(error.to_string())),
        }
    }
}

/// A database/TCP port that could not be reserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortUnavailableError {
    pub port: i64,
    pub role: String,
    pub detail: String,
}

impl std::fmt::Display for PortUnavailableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} port {} is unavailable; inspect the listener or choose another local port: {}",
            self.role, self.port, self.detail
        )
    }
}

impl std::error::Error for PortUnavailableError {}

/// One process listening on a TCP port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortListener {
    pub pid: i64,
    pub uid: i64,
    pub name: String,
    pub address: String,
    /// The listener belongs to this app's HTTPS proxy job.
    pub own: bool,
}

/// The result of inspecting one port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInspection {
    pub port: i64,
    /// `Available`, `In use`, `Devbase proxy` or `Unknown`.
    pub status: String,
    pub checked_at: String,
    pub listeners: Vec<PortListener>,
    pub truncated: bool,
    pub detail: String,
}

/// One user-owned listening port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListeningPort {
    pub port: i64,
    pub process: String,
}

/// The reserved-port view used by [`Databases::available_local_port`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AvailablePorts {
    pub reserved: BTreeSet<i64>,
}

impl AvailablePorts {
    /// Ports an instance, the proxy listener and host-less proxy rules hold.
    pub fn from_state(state: &State) -> Self {
        let mut reserved = BTreeSet::new();
        for instance in state.instances.values() {
            reserved.insert(instance.port);
        }
        reserved.insert(state.proxy_listener_port);
        for rule in &state.proxy_rules {
            if rule.upstream_host.is_empty() {
                reserved.insert(rule.local_port);
            }
        }
        AvailablePorts { reserved }
    }
}

/// Normalise a listener value for display and comparison: control characters
/// and whitespace become a single space, capped at 240 bytes.
pub fn visible_port_text(value: &str) -> String {
    let mut out = String::new();
    for character in value.chars() {
        let character = if character.is_control() || character.is_whitespace() {
            ' '
        } else {
            character
        };
        out.push(character);
        if out.len() > 240 {
            break;
        }
    }
    out.trim().to_string()
}

fn is_lsof_field_tag(tag: u8) -> bool {
    matches!(tag, b'p' | b'c' | b'u' | b'f' | b'n')
}

fn extract_lsof_pid_field(field: &str) -> Option<String> {
    for line in field.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(digits) = line.strip_prefix('p')
            && !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Some(format!("p{digits}"));
        }
    }
    None
}

/// Parse `lsof -F0pcun` output. The limit bounds the returned listeners.
pub fn parse_port_listeners_limit(output: &str, limit: usize) -> Vec<PortListener> {
    let mut current = PortListener {
        pid: 0,
        uid: -1,
        name: String::new(),
        address: String::new(),
        own: false,
    };
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    'fields: for raw in output.split('\0') {
        let trimmed = raw.trim_start_matches(['\r', '\n']);
        if trimmed.is_empty() {
            continue;
        }
        let extracted;
        let field = if is_lsof_field_tag(trimmed.as_bytes()[0]) {
            trimmed
        } else {
            match extract_lsof_pid_field(trimmed) {
                Some(value) => {
                    extracted = value;
                    extracted.as_str()
                }
                None => continue,
            }
        };
        let mut characters = field.chars();
        let tag = characters.next().unwrap_or('\0');
        let value = characters.as_str();
        match tag {
            'p' => {
                current = PortListener {
                    pid: 0,
                    uid: -1,
                    name: String::new(),
                    address: String::new(),
                    own: false,
                };
                current.pid = value.parse().unwrap_or(0);
            }
            'c' => current.name = visible_port_text(value),
            'u' => {
                if let Ok(uid) = value.parse() {
                    current.uid = uid;
                }
            }
            'n' => {
                current.address = visible_port_text(value);
                if current.pid < 1 || current.address.is_empty() {
                    continue;
                }
                let key = format!("{}:{}", current.pid, current.address);
                if seen.insert(key) {
                    results.push(current.clone());
                    if limit > 0 && results.len() >= limit {
                        break 'fields;
                    }
                }
            }
            _ => {}
        }
    }
    results.sort_by(|a, b| a.pid.cmp(&b.pid).then(a.address.cmp(&b.address)));
    results
}

/// [`parse_port_listeners_limit`] with the Go default cap of 65.
pub fn parse_port_listeners(output: &str) -> Vec<PortListener> {
    parse_port_listeners_limit(output, 65)
}

/// Parse `netstat -anv -p tcp` LISTEN rows for a target port.
pub fn parse_netstat_listeners(output: &str, port: i64) -> Vec<PortListener> {
    let port_text = port.to_string();
    let mut seen = BTreeSet::new();
    let mut results: Vec<PortListener> = Vec::new();
    for line in output.split('\n') {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 11 {
            continue;
        }
        if !matches!(fields[0], "tcp4" | "tcp6" | "tcp46") || fields[5] != "LISTEN" {
            continue;
        }
        let local = fields[3];
        let Some((host, parsed_port)) = local.rsplit_once('.') else {
            continue;
        };
        if host.is_empty() || parsed_port != port_text {
            continue;
        }
        let address = if host.contains(':') {
            format!("[{host}]:{port_text}")
        } else {
            format!("{host}:{port_text}")
        };
        let mut pid = 0;
        let mut raw_name = String::new();
        for index in (10..fields.len()).rev() {
            let Some((prefix, candidate)) = fields[index].rsplit_once(':') else {
                continue;
            };
            let Ok(parsed) = candidate.parse::<i64>() else {
                continue;
            };
            if parsed < 1 {
                continue;
            }
            pid = parsed;
            let mut parts: Vec<&str> = Vec::new();
            if index > 10 {
                parts.extend_from_slice(&fields[10..index]);
            }
            if !prefix.is_empty() {
                parts.push(prefix);
            }
            raw_name = parts.join(" ");
            break;
        }
        if pid < 1 {
            continue;
        }
        let name = match visible_port_text(&raw_name) {
            empty if empty.is_empty() => "unknown".to_string(),
            name => name,
        };
        if !seen.insert(format!("{pid}:{address}")) {
            continue;
        }
        results.push(PortListener {
            pid,
            uid: -1,
            name,
            address,
            own: false,
        });
        if results.len() >= 65 {
            break;
        }
    }
    results.sort_by(|a, b| a.pid.cmp(&b.pid).then(a.address.cmp(&b.address)));
    results
}

fn split_host_port(address: &str) -> Option<(String, i64)> {
    if let Some(rest) = address.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        return Some((host.to_string(), port.parse().ok()?));
    }
    let (host, port) = address.rsplit_once(':')?;
    if host.is_empty() || host.contains(':') {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

/// Split an `ss` local address (`127.0.0.1:5432`, `[::1]:5432`, `::1:5432`,
/// `*:3000`, `127.0.0.53%lo:53`) into `(normalized_display_address, port)`.
fn split_ss_address(local: &str) -> Option<(String, i64)> {
    let (raw_host, port_str) = local.rsplit_once(':')?;
    let port: i64 = port_str.parse().ok()?;
    if !(1..=65535).contains(&port) {
        return None;
    }
    let host_unbracketed = raw_host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(raw_host);
    let host = host_unbracketed
        .split('%')
        .next()
        .unwrap_or(host_unbracketed);
    if host.is_empty() {
        return None;
    }
    let display = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    Some((display, port))
}

/// Extract `(process_name, pid)` tuples from an `ss -p` `users:(...)` column.
fn parse_ss_users(field: &str) -> Vec<(String, i64)> {
    let Some(rest) = field.strip_prefix("users:(") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = rest;
    while let Some(open) = cursor.find('(') {
        cursor = &cursor[open + 1..];
        let Some(close) = cursor.find(')') else {
            break;
        };
        let entry = &cursor[..close];
        cursor = &cursor[close + 1..];

        let mut name = String::new();
        let mut pid: i64 = 0;
        for part in entry.split(',') {
            let part = part.trim();
            if let Some(quoted) = part.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                name = visible_port_text(quoted);
            } else if let Some(pid_str) = part.strip_prefix("pid=")
                && let Ok(parsed) = pid_str.parse::<i64>()
                && parsed > 0
            {
                pid = parsed;
            }
        }
        if pid > 0 {
            if name.is_empty() {
                name = "unknown".to_string();
            }
            out.push((name, pid));
        }
    }
    out
}

/// Parse Linux `ss -Hltpn` (or `ss -Hltpne`) output into [`PortListener`] rows.
///
/// When `filter_port` is `Some(port)`, only rows listening on that TCP port are
/// returned. `limit == 0` disables the entry cap.
pub fn parse_ss_listeners(
    output: &str,
    filter_port: Option<i64>,
    limit: usize,
) -> Vec<PortListener> {
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    'lines: for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 || fields[0] != "LISTEN" {
            continue;
        }
        let Some((address, port)) = split_ss_address(fields[3]) else {
            continue;
        };
        if let Some(want) = filter_port
            && port != want
        {
            continue;
        }
        let mut uid: i64 = -1;
        let mut users: Vec<(String, i64)> = Vec::new();
        for token in &fields[5..] {
            if token.starts_with("users:(") {
                users.extend(parse_ss_users(token));
            } else if let Some(raw_uid) = token.strip_prefix("uid:")
                && let Ok(parsed) = raw_uid.parse::<i64>()
            {
                uid = parsed;
            }
        }
        if users.is_empty() {
            // Root/other-user listener without process info exposed to non-root caller.
            if seen.insert(format!("0:{address}")) {
                results.push(PortListener {
                    pid: 0,
                    uid,
                    name: "unknown".to_string(),
                    address,
                    own: false,
                });
                if limit > 0 && results.len() >= limit {
                    break 'lines;
                }
            }
            continue;
        }
        for (name, pid) in users {
            if !seen.insert(format!("{pid}:{address}")) {
                continue;
            }
            results.push(PortListener {
                pid,
                uid,
                name,
                address: address.clone(),
                own: false,
            });
            if limit > 0 && results.len() >= limit {
                break 'lines;
            }
        }
    }
    results.sort_by(|a, b| a.pid.cmp(&b.pid).then(a.address.cmp(&b.address)));
    results
}

/// Parse Linux `/proc/net/tcp` (`ipv6 = false`) or `/proc/net/tcp6` (`ipv6 = true`)
/// for listening (`st == "0A"`) sockets.
pub fn parse_proc_net_tcp(
    content: &str,
    ipv6: bool,
    filter_port: Option<i64>,
) -> Vec<PortListener> {
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 || fields[3] != "0A" {
            continue;
        }
        let Some((hex_ip, hex_port)) = fields[1].split_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(hex_port, 16) else {
            continue;
        };
        let port = i64::from(port);
        if port < 1 {
            continue;
        }
        if let Some(want) = filter_port
            && port != want
        {
            continue;
        }
        let address = format_proc_hex_addr(hex_ip, ipv6, port);
        let uid = fields[7].parse::<i64>().unwrap_or(-1);
        if !seen.insert(format!("{uid}:{address}")) {
            continue;
        }
        results.push(PortListener {
            pid: 0,
            uid,
            name: "unknown".to_string(),
            address,
            own: false,
        });
    }
    results.sort_by(|a, b| a.address.cmp(&b.address));
    results
}

fn format_proc_hex_addr(hex_ip: &str, ipv6: bool, port: i64) -> String {
    if !ipv6
        && hex_ip.len() == 8
        && let Ok(val) = u32::from_str_radix(hex_ip, 16)
    {
        let octets = val.to_le_bytes();
        return format!(
            "{}.{}.{}.{}:{port}",
            octets[0], octets[1], octets[2], octets[3]
        );
    }
    if ipv6 && hex_ip.len() == 32 {
        let mut bytes = [0u8; 16];
        let mut ok = true;
        for chunk in 0..4 {
            let start = chunk * 8;
            if let Ok(word) = u32::from_str_radix(&hex_ip[start..start + 8], 16) {
                bytes[chunk * 4..chunk * 4 + 4].copy_from_slice(&word.to_le_bytes());
            } else {
                ok = false;
                break;
            }
        }
        if ok {
            let ip = std::net::Ipv6Addr::from(bytes);
            return format!("[{ip}]:{port}");
        }
    }
    format!("{hex_ip}:{port}")
}

/// Select a free instance port, trying the preferred one first (when it is
/// not reserved) and then 32 kernel-assigned ports.
pub fn free_instance_port(
    binder: &dyn PortBinder,
    preferred: i64,
    reserved: &BTreeSet<i64>,
) -> Result<i64, DatabaseError> {
    if preferred != 0 {
        if !(1024..=65535).contains(&preferred) {
            return Err(DatabaseError::invalid(
                "locked database port must be 1024–65535",
            ));
        }
        if !reserved.contains(&preferred) && binder.bind_loopback(preferred as u16).is_ok() {
            return Ok(preferred);
        }
    }
    for _ in 0..32 {
        let port = binder.bind_loopback(0).map_err(|failure| match failure {
            PortBindFailure::Other(detail) => DatabaseError::Io(std::io::Error::other(detail)),
            other => DatabaseError::Io(std::io::Error::other(format!("{other:?}"))),
        })?;
        let port = i64::from(port);
        if !reserved.contains(&port) {
            return Ok(port);
        }
    }
    Err(DatabaseError::invalid(
        "could not select a free database port",
    ))
}

/// Refuse a specific port unless it binds on IPv4 loopback. This is the
/// `freeDatabasePort` preflight every create and start performs.
pub fn free_database_port(binder: &dyn PortBinder, port: i64) -> Result<(), DatabaseError> {
    match binder.bind_loopback(port as u16) {
        Ok(_) => Ok(()),
        Err(failure) => Err(DatabaseError::PortUnavailable(PortUnavailableError {
            port,
            role: "database".to_string(),
            detail: match failure {
                PortBindFailure::AddrInUse => "address already in use".to_string(),
                PortBindFailure::PermissionDenied => "permission denied".to_string(),
                PortBindFailure::Other(detail) => detail,
            },
        })),
    }
}

impl Databases {
    /// `AvailableLocalPort`: reserve configured ports and pick a free one.
    pub fn available_local_port(
        &self,
        cancelled: super::Cancelled<'_>,
    ) -> Result<i64, DatabaseError> {
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let state = self.store().read()?;
        free_instance_port(
            self.binder(),
            0,
            &AvailablePorts::from_state(&state).reserved,
        )
    }

    /// `InspectPort`: bind first, then observe listeners when the port is in
    /// use. A bind failure other than `EADDRINUSE` yields `Unknown`, never a
    /// fabricated listener list.
    pub fn inspect_port(
        &self,
        port: i64,
        cancelled: super::Cancelled<'_>,
    ) -> Result<PortInspection, DatabaseError> {
        let mut inspection = PortInspection {
            port,
            status: "Unknown".to_string(),
            checked_at: crate::software::utc_now_rfc3339(),
            listeners: Vec::new(),
            truncated: false,
            detail: String::new(),
        };
        if !(1..=65535).contains(&port) {
            return Err(DatabaseError::invalid("port must be 1–65535"));
        }
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        match self.binder().bind_loopback(port as u16) {
            Ok(_) => {
                inspection.status = "Available".to_string();
                inspection.detail = "IPv4 loopback was available at this check. Saving a suggestion does not reserve it.".to_string();
                return Ok(inspection);
            }
            Err(PortBindFailure::PermissionDenied) => {
                let dial = self
                    .binder()
                    .dial_loopback(port as u16, Duration::from_millis(500));
                if cancelled() {
                    return Err(DatabaseError::Cancelled);
                }
                match dial {
                    Ok(()) => {}
                    Err(PortBindFailure::Other(detail))
                        if detail.to_lowercase().contains("refused") =>
                    {
                        inspection.status = "Available".to_string();
                        inspection.detail = "IPv4 loopback had no active listener. Privileged ports below 1024 use launchd authorization when applying the proxy.".to_string();
                        return Ok(inspection);
                    }
                    Err(PortBindFailure::Other(detail)) => {
                        inspection.detail =
                            format!("Could not verify IPv4 loopback availability: {detail}");
                        return Ok(inspection);
                    }
                    Err(failure) => {
                        inspection.detail =
                            format!("Could not verify IPv4 loopback availability: {failure:?}");
                        return Ok(inspection);
                    }
                }
            }
            Err(PortBindFailure::Other(detail)) => {
                inspection.detail = format!("Could not test IPv4 loopback availability: {detail}");
                return Ok(inspection);
            }
            Err(PortBindFailure::AddrInUse) => {}
        }

        inspection.status = "In use".to_string();
        inspection.listeners = self.observe_port_listeners(port, cancelled)?;
        if inspection.listeners.len() > 64 {
            inspection.listeners.truncate(64);
            inspection.truncated = true;
        }
        let own = self.mark_own_proxy_listeners(port, &mut inspection.listeners, cancelled)?;
        if inspection.listeners.is_empty() {
            inspection.detail = "IPv4 loopback is in use. Listener details were unavailable or the process changed during this check.".to_string();
        } else if own {
            inspection.status = "Devbase proxy".to_string();
            inspection.detail = "This port is held by the Devbase HTTPS proxy. Saving it keeps the proxy where it is.".to_string();
        } else {
            inspection.detail = "Listeners reported for this TCP port. Names and PIDs do not prove app ownership. Stop an external service in its own app, or choose another port.".to_string();
        }
        Ok(inspection)
    }

    #[cfg(target_os = "macos")]
    fn observe_port_listeners(
        &self,
        port: i64,
        cancelled: super::Cancelled<'_>,
    ) -> Result<Vec<PortListener>, DatabaseError> {
        let query_timeout = Duration::from_secs(2);
        let mut listeners = Vec::new();
        let lsof = self.engine().run_with_timeout(
            "/usr/sbin/lsof",
            &[
                "-nP",
                "-a",
                &format!("-iTCP:{port}"),
                "-sTCP:LISTEN",
                "+c0",
                "-F0pcun",
            ],
            query_timeout,
        );
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        if let Ok(output) = lsof {
            listeners = parse_port_listeners(&output);
        }
        if listeners.is_empty()
            && let Ok(output) = self.engine().run_with_timeout(
                "/usr/sbin/netstat",
                &["-anv", "-p", "tcp"],
                query_timeout,
            )
        {
            listeners = parse_netstat_listeners(&output, port);
        }
        Ok(listeners)
    }

    #[cfg(not(target_os = "macos"))]
    fn observe_port_listeners(
        &self,
        port: i64,
        cancelled: super::Cancelled<'_>,
    ) -> Result<Vec<PortListener>, DatabaseError> {
        let query_timeout = Duration::from_secs(2);
        let ss = self
            .engine()
            .run_with_timeout("ss", &["-Hltpne"], query_timeout)
            .or_else(|_| {
                self.engine()
                    .run_with_timeout("ss", &["-Hltpn"], query_timeout)
            });
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        if let Ok(output) = ss {
            return Ok(parse_ss_listeners(&output, Some(port), 65));
        }
        if let Ok(output) = self.engine().run_with_timeout(
            "lsof",
            &[
                "-nP",
                "-a",
                &format!("-iTCP:{port}"),
                "-sTCP:LISTEN",
                "+c0",
                "-F0pcun",
            ],
            query_timeout,
        ) {
            return Ok(parse_port_listeners(&output));
        }
        let mut proc_listeners = Vec::new();
        if let Ok(tcp) = std::fs::read_to_string("/proc/net/tcp") {
            proc_listeners.extend(parse_proc_net_tcp(&tcp, false, Some(port)));
        }
        if let Ok(tcp6) = std::fs::read_to_string("/proc/net/tcp6") {
            proc_listeners.extend(parse_proc_net_tcp(&tcp6, true, Some(port)));
        }
        Ok(proc_listeners)
    }

    /// `ListeningUserPorts`: current-UID TCP listeners at 1024+ excluding the
    /// proxy and managed instances, deduplicated and sorted.
    #[cfg(target_os = "macos")]
    pub fn listening_user_ports(
        &self,
        cancelled: super::Cancelled<'_>,
    ) -> Result<Vec<ListeningPort>, DatabaseError> {
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let state = self.store().read()?;
        let uid = unsafe { libc::getuid() } as i64;
        let timeout = Duration::from_secs(2);
        let output = match self.engine().run_with_timeout(
            "/usr/sbin/lsof",
            &[
                "-w",
                "-nP",
                "-a",
                "-u",
                &uid.to_string(),
                "-iTCP",
                "-sTCP:LISTEN",
                "+c0",
                "-F0pcun",
            ],
            timeout,
        ) {
            Ok(output) => output,
            Err(crate::engine::EngineError::Command(error))
                if error.status == Some(1) && error.stdout.trim().is_empty() =>
            {
                return Ok(Vec::new());
            }
            Err(error) => return Err(DatabaseError::Engine(error)),
        };
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        // The Go list excludes only the proxy listener and managed instances;
        // proxy rules are ordinary user ports for this view.
        let mut excluded = BTreeSet::new();
        excluded.insert(state.proxy_listener_port);
        for instance in state.instances.values() {
            excluded.insert(instance.port);
        }
        let mut by_port: BTreeMap<i64, String> = BTreeMap::new();
        for listener in parse_port_listeners_limit(&output, 0) {
            if listener.uid != uid {
                continue;
            }
            let Some((_host, port)) = split_host_port(&listener.address) else {
                continue;
            };
            if !(1024..=65535).contains(&port) || excluded.contains(&port) {
                continue;
            }
            match by_port.get(&port) {
                Some(previous) if !(previous.is_empty() && !listener.name.is_empty()) => {}
                _ => {
                    by_port.insert(port, listener.name);
                }
            }
        }
        Ok(by_port
            .into_iter()
            .map(|(port, process)| ListeningPort { port, process })
            .collect())
    }

    /// `ListeningUserPorts` (Linux): current-UID TCP listeners at 1024+ via
    /// `ss -Hltpne` (falling back to `/proc/net/tcp{,6}` and `lsof`).
    #[cfg(not(target_os = "macos"))]
    pub fn listening_user_ports(
        &self,
        cancelled: super::Cancelled<'_>,
    ) -> Result<Vec<ListeningPort>, DatabaseError> {
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let state = self.store().read()?;
        let uid = unsafe { libc::getuid() } as i64;
        let timeout = Duration::from_secs(2);

        let listeners = if let Ok(output) = self
            .engine()
            .run_with_timeout("ss", &["-Hltpne"], timeout)
            .or_else(|_| self.engine().run_with_timeout("ss", &["-Hltpn"], timeout))
        {
            parse_ss_listeners(&output, None, 0)
        } else if let Ok(output) = self.engine().run_with_timeout(
            "lsof",
            &[
                "-w",
                "-nP",
                "-a",
                "-u",
                &uid.to_string(),
                "-iTCP",
                "-sTCP:LISTEN",
                "+c0",
                "-F0pcun",
            ],
            timeout,
        ) {
            parse_port_listeners_limit(&output, 0)
        } else {
            let mut proc_listeners = Vec::new();
            if let Ok(tcp) = std::fs::read_to_string("/proc/net/tcp") {
                proc_listeners.extend(parse_proc_net_tcp(&tcp, false, None));
            }
            if let Ok(tcp6) = std::fs::read_to_string("/proc/net/tcp6") {
                proc_listeners.extend(parse_proc_net_tcp(&tcp6, true, None));
            }
            proc_listeners
        };
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }

        let mut excluded = BTreeSet::new();
        excluded.insert(state.proxy_listener_port);
        for instance in state.instances.values() {
            excluded.insert(instance.port);
        }
        let mut by_port: BTreeMap<i64, String> = BTreeMap::new();
        for mut listener in listeners {
            if listener.uid < 0
                && listener.pid > 0
                && let Some(proc_uid) = proc_pid_uid(listener.pid)
            {
                listener.uid = proc_uid;
            }
            if listener.uid != uid {
                continue;
            }
            let Some((_host, port)) = split_host_port(&listener.address) else {
                continue;
            };
            if !(1024..=65535).contains(&port) || excluded.contains(&port) {
                continue;
            }
            match by_port.get(&port) {
                Some(previous) if !(previous.is_empty() && !listener.name.is_empty()) => {}
                _ => {
                    by_port.insert(port, listener.name);
                }
            }
        }
        Ok(by_port
            .into_iter()
            .map(|(port, process)| ListeningPort { port, process })
            .collect())
    }

    #[cfg(target_os = "macos")]
    fn mark_own_proxy_listeners(
        &self,
        port: i64,
        listeners: &mut [PortListener],
        cancelled: super::Cancelled<'_>,
    ) -> Result<bool, DatabaseError> {
        if listeners.is_empty() {
            return Ok(false);
        }
        let state = match self.store().read() {
            Ok(state) => state,
            Err(_) => return Ok(false),
        };
        if state.proxy_listener_port != port {
            return Ok(false);
        }
        let target = format!("system/{}", self.engine().label("proxy-sockets"));
        let output = match self.engine().run_with_timeout(
            "/bin/launchctl",
            &["print", &target],
            Duration::from_secs(2),
        ) {
            Ok(output) => output,
            Err(_) => return Ok(false),
        };
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let pid = launchctl_pid(&output);
        let mut all = true;
        for listener in listeners.iter_mut() {
            listener.own = (pid > 0 && listener.pid == pid) || (pid == 0 && listener.pid == 1);
            all = all && listener.own;
        }
        Ok(all)
    }

    #[cfg(not(target_os = "macos"))]
    fn mark_own_proxy_listeners(
        &self,
        port: i64,
        listeners: &mut [PortListener],
        cancelled: super::Cancelled<'_>,
    ) -> Result<bool, DatabaseError> {
        if listeners.is_empty() {
            return Ok(false);
        }
        let state = match self.store().read() {
            Ok(state) => state,
            Err(_) => return Ok(false),
        };
        if state.proxy_listener_port != port {
            return Ok(false);
        }
        let stem = crate::helper::systemd_unit::linux_proxy_unit_stem(self.store().root());
        let service_unit = format!("{stem}.service");
        let output = match self.engine().run_with_timeout(
            "systemctl",
            &[
                "show",
                "--no-pager",
                "-p",
                "MainPID",
                "--value",
                &service_unit,
            ],
            Duration::from_secs(2),
        ) {
            Ok(output) => output,
            Err(_) => return Ok(false),
        };
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let pid = output.trim().parse::<i64>().unwrap_or(0);
        let socket_listening = if pid == 0 {
            let socket_unit = format!("{stem}.socket");
            self.engine()
                .run_with_timeout(
                    "systemctl",
                    &[
                        "show",
                        "--no-pager",
                        "-p",
                        "ActiveState",
                        "--value",
                        &socket_unit,
                    ],
                    Duration::from_secs(2),
                )
                .is_ok_and(|out| out.trim() == "active" || out.trim() == "listening")
        } else {
            false
        };
        let mut all = true;
        for listener in listeners.iter_mut() {
            listener.own = (pid > 0 && listener.pid == pid)
                || (pid == 0 && socket_listening && (listener.pid == 1 || listener.pid == 0));
            all = all && listener.own;
        }
        Ok(all)
    }
}

#[cfg(target_os = "macos")]
fn launchctl_pid(output: &str) -> i64 {
    for line in output.split('\n') {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("pid = ") {
            return value.trim().parse().unwrap_or(0);
        }
    }
    0
}

#[cfg(not(target_os = "macos"))]
fn proc_pid_uid(pid: i64) -> Option<i64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse::<i64>().ok();
        }
    }
    None
}

#[cfg(test)]
mod linux_port_tests {
    use super::*;

    #[test]
    fn parses_ss_hltpn_and_extended_output() {
        let sample = "\
LISTEN 0 4096 127.0.0.1:5432 0.0.0.0:* users:((\"postgres\",pid=1234,fd=6)) uid:1000 ino:101
LISTEN 0 128 0.0.0.0:8080 0.0.0.0:* users:((\"node\",pid=5678,fd=20),(\"node\",pid=5679,fd=20)) uid:1000
LISTEN 0 4096 [::1]:5432 [::]:* users:((\"postgres\",pid=1234,fd=7)) uid:1000
LISTEN 0 511 *:3000 *:* users:((\"vite\",pid=9999,fd=12))
LISTEN 0 4096 127.0.0.53%lo:53 0.0.0.0:* users:((\"systemd-resolve\",pid=412,fd=15))
LISTEN 0 4096 127.0.0.1:443 0.0.0.0:*
ESTAB 0 0 127.0.0.1:5432 127.0.0.1:50000 users:((\"postgres\",pid=1234,fd=9))
";
        let pg = parse_ss_listeners(sample, Some(5432), 65);
        assert_eq!(pg.len(), 2);
        assert_eq!(pg[0].pid, 1234);
        assert_eq!(pg[0].uid, 1000);
        assert_eq!(pg[0].name, "postgres");
        assert_eq!(pg[0].address, "127.0.0.1:5432");
        assert_eq!(pg[1].address, "[::1]:5432");

        let node = parse_ss_listeners(sample, Some(8080), 65);
        assert_eq!(node.len(), 2);
        assert_eq!(node[0].pid, 5678);
        assert_eq!(node[1].pid, 5679);
        assert_eq!(node[0].name, "node");

        let dns = parse_ss_listeners(sample, Some(53), 65);
        assert_eq!(dns.len(), 1);
        assert_eq!(dns[0].address, "127.0.0.53:53");
        assert_eq!(dns[0].name, "systemd-resolve");

        let https = parse_ss_listeners(sample, Some(443), 65);
        assert_eq!(https.len(), 1);
        assert_eq!(https[0].pid, 0);
        assert_eq!(https[0].name, "unknown");

        // All listening ports (ignores ESTAB)
        let all = parse_ss_listeners(sample, None, 0);
        assert_eq!(all.len(), 7);

        // Respects limit
        let capped = parse_ss_listeners(sample, None, 2);
        assert_eq!(capped.len(), 2);
    }

    #[test]
    fn parses_proc_net_tcp_and_tcp6() {
        let tcp = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1538 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345
   1: 0100007F:1538 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 12346
";
        // 0x1538 == 5432, 0100007F == 127.0.0.1
        let parsed = parse_proc_net_tcp(tcp, false, Some(5432));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].address, "127.0.0.1:5432");
        assert_eq!(parsed[0].uid, 1000);

        let tcp6 = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:1538 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 22345
";
        let parsed6 = parse_proc_net_tcp(tcp6, true, Some(5432));
        assert_eq!(parsed6.len(), 1);
        assert_eq!(parsed6[0].address, "[::1]:5432");
        assert_eq!(parsed6[0].uid, 1000);
    }
}
