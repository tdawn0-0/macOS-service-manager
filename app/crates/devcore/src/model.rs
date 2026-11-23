//! The persisted model, ported from the Go `model.go`, `proxy_rules.go` and
//! `connection_profiles.go` types and validators.
//!
//! The JSON contract is exact: field names are camelCase, omitted optional
//! fields stay omitted, unknown fields are rejected (including inside nested
//! objects), and the schema version is validated before any field is decoded.
//! That strictness is what keeps credentials from ever entering the state
//! document: a profile or tunnel record carrying a password/token field does
//! not decode at all.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};

use crate::platform::HostTarget;
use crate::state::StateError;

/// The only state document version this build reads and writes.
pub const SCHEMA: i64 = 2;

/// The suffix every proxy rule domain uses.
pub const LOCAL_DOMAIN_SUFFIX: &str = ".devbase.localhost";

static NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,79}$").unwrap());
static JAVA_INSTALL_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._+\-]{0,119}$").unwrap());
static JAVA_FOLDER_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[1-9][0-9]*$").unwrap());
static PROXY_DOMAIN_PART_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$").unwrap());
static POSTGRES_BREW_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)(?:\.\d+)?(?:_\d+)?$").unwrap());
static REDIS_BREW_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^8\.2(?:\.\d+)?(?:_\d+)?$").unwrap());
static REDIS_EXACT_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^8\.2\.\d+$").unwrap());
static TUNNEL_CLIENT_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{1,39}$").unwrap());
static CLOUDFLARE_ACCOUNT_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9_-]{8,64}$").unwrap());
static TUNNEL_SCRIPT_DIGEST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{16,64}$").unwrap());

/// One installed (or installable) piece of managed software.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Package {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub java_install_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub java_product: String,
    /// Actual user-level installation folder; Java only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub java_folder: String,
    /// Homebrew canonical full name; empty for app archives.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub formula: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    /// `tar.gz`, `tar.xz`, `zip`, `homebrew`, or `system`.
    #[serde(default)]
    pub format: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub strip: i64,
    #[serde(default)]
    pub binaries: BTreeMap<String, String>,
    #[serde(default)]
    pub license: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub min_macos: String,
}

impl Package {
    /// `name@version-arch`, the identifier used everywhere in state.
    pub fn id(&self) -> String {
        format!("{}@{}-{}", self.name, self.version, self.arch)
    }

    /// Validate the manifest for the compiled host target.
    pub fn validate(&self) -> Result<(), StateError> {
        self.validate_for_target(HostTarget::current())
    }

    /// Validate the manifest against an explicit [`HostTarget`].
    pub fn validate_for_target(&self, target: HostTarget) -> Result<(), StateError> {
        if !self.java_install_name.is_empty()
            && (self.name != "java" || !JAVA_INSTALL_NAME_RE.is_match(&self.java_install_name))
        {
            return Err(StateError::invalid(
                "invalid Java catalog installation name",
            ));
        }
        if !self.java_folder.is_empty() {
            let base = java_folder_base(self);
            let suffix = self
                .java_folder
                .strip_prefix(&format!("{base}-"))
                .unwrap_or(&self.java_folder);
            let plain_name = std::path::Path::new(&self.java_folder)
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new(&self.java_folder));
            if self.name != "java"
                || !plain_name
                || (self.java_folder != base
                    && (suffix == self.java_folder || !JAVA_FOLDER_SUFFIX_RE.is_match(suffix)))
            {
                return Err(StateError::invalid("invalid Java installation folder"));
            }
        }
        if !target.accepts_model_arch(&self.arch) {
            return Err(StateError::invalid(if target.is_macos() {
                "managed software requires Apple Silicon (arm64); Intel packages are not supported"
            } else {
                "package architecture does not match this Linux host"
            }));
        }
        if !NAME_RE.is_match(&self.name) || !NAME_RE.is_match(&self.version) {
            return Err(StateError::invalid("invalid package identity"));
        }
        if self.name == "php" || self.name == "php-fpm" {
            return Err(StateError::invalid("PHP is not managed by this app"));
        }
        if self.name == "postgresql" || self.name == "redis" {
            if target.is_macos() {
                if self.formula.is_empty() {
                    return Err(StateError::invalid(
                        "PostgreSQL and Redis require a Homebrew package",
                    ));
                }
            } else if self.formula.is_empty() && self.format != "system" {
                return Err(StateError::invalid(
                    "PostgreSQL and Redis require a system installation or Homebrew package",
                ));
            }
        }
        let is_linux_system = target.is_linux() && self.format == "system";
        if is_linux_system {
            if !self.url.is_empty() && !valid_package_url(&self.url) {
                return Err(StateError::invalid("package URL must use HTTPS"));
            }
        } else if !valid_package_url(&self.url) {
            return Err(StateError::invalid("package URL must use HTTPS"));
        }
        if !crate::fs::valid_digest(&self.sha256) {
            return Err(StateError::invalid("package requires a SHA-256 checksum"));
        }
        if self.format == "homebrew" {
            if !valid_homebrew_package(self, target) {
                return Err(StateError::invalid(
                    "invalid Homebrew package source, version or binary layout",
                ));
            }
        } else if self.format == "system" {
            if !target.is_linux() {
                return Err(StateError::invalid("unsupported archive format"));
            }
            if !valid_system_package(self, target) {
                return Err(StateError::invalid(
                    "invalid system package source, version or binary layout",
                ));
            }
        } else if !self.formula.is_empty() {
            return Err(StateError::invalid(
                "Homebrew formula requires the Homebrew package format",
            ));
        } else if self.format != "tar.gz"
            && self.format != "zip"
            && !(target.is_linux() && self.format == "tar.xz")
        {
            return Err(StateError::invalid("unsupported archive format"));
        }
        if !(0..=8).contains(&self.strip) || self.binaries.is_empty() {
            return Err(StateError::invalid("invalid package layout"));
        }
        for (name, path) in &self.binaries {
            let valid_path = if is_linux_system {
                safe_system_binary_path(path)
            } else {
                crate::fs::safe_relative(path)
            };
            if !NAME_RE.is_match(name) || !valid_path {
                return Err(StateError::invalid("invalid binary path"));
            }
        }
        Ok(())
    }
}

fn valid_package_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    // Go's `u.User != nil` rejects any userinfo, including an empty `@`.
    if let Some(rest) = raw.strip_prefix("https://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.contains('@') {
            return false;
        }
    }
    url.scheme() == "https"
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

/// The Homebrew formula name a `(name, version)` pair must use.
pub fn homebrew_formula_for(name: &str, version: &str) -> Option<String> {
    let version = version.strip_prefix('v').unwrap_or(version);
    match name {
        "postgresql" => POSTGRES_BREW_VERSION_RE
            .captures(version)
            .map(|captures| format!("postgresql@{}", &captures[1])),
        "redis" if REDIS_BREW_VERSION_RE.is_match(version) => Some("redis@8.2".to_string()),
        _ => None,
    }
}

/// The exact binary layout a Homebrew keg has.
pub fn homebrew_binaries(name: &str) -> BTreeMap<String, String> {
    let names: &[&str] = if name == "redis" {
        &["redis-server", "redis-cli"]
    } else {
        &[
            "postgres",
            "initdb",
            "pg_dump",
            "pg_restore",
            "psql",
            "createdb",
        ]
    };
    names
        .iter()
        .map(|binary| ((*binary).to_string(), format!("bin/{binary}")))
        .collect()
}

fn valid_homebrew_package(package: &Package, target: HostTarget) -> bool {
    let Some(formula) = homebrew_formula_for(&package.name, &package.version) else {
        return false;
    };
    let arch_ok = if target.is_macos() {
        package.arch == "arm64"
    } else {
        target.accepts_model_arch(&package.arch)
    };
    if !arch_ok || package.strip != 0 || package.formula != formula {
        return false;
    }
    let base = package.version.split('_').next().unwrap_or_default();
    match package.name.as_str() {
        "postgresql" if base.matches('.').count() != 1 => return false,
        "redis" if !REDIS_EXACT_VERSION_RE.is_match(base) => return false,
        "postgresql" | "redis" => {}
        _ => return false,
    }
    package.binaries == homebrew_binaries(&package.name)
}

/// Validates a Linux `format == "system"` package (system-installed PostgreSQL or Redis).
fn valid_system_package(package: &Package, target: HostTarget) -> bool {
    if !target.is_linux()
        || !package.formula.is_empty()
        || package.strip != 0
        || !target.accepts_model_arch(&package.arch)
    {
        return false;
    }
    if !matches!(package.name.as_str(), "postgresql" | "redis") {
        return false;
    }
    let expected = homebrew_binaries(&package.name);
    if package.binaries.len() != expected.len() {
        return false;
    }
    expected.keys().all(|key| {
        package
            .binaries
            .get(key)
            .is_some_and(|p| safe_system_binary_path(p))
    })
}

/// Whether `path` is a safe absolute path to a system executable (no traversal or control chars).
pub fn safe_system_binary_path(path: &str) -> bool {
    use std::path::{Component, Path};
    if path.is_empty() || path.contains(['\0', '\r', '\n']) {
        return false;
    }
    let p = Path::new(path);
    if !p.is_absolute() || p.file_name().is_none() {
        return false;
    }
    p.components()
        .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

/// The install folder base for a Java package.
pub fn java_folder_base(package: &Package) -> String {
    if !package.java_install_name.is_empty() {
        return package.java_install_name.clone();
    }
    let version = package.version.split('_').next().unwrap_or_default();
    format!("temurin-{version}")
}

/// How frequently periodic maintenance runs an automatic backup for a running instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BackupCadence {
    #[default]
    Daily,
    Weekly,
}

impl BackupCadence {
    /// Minimum elapsed window between automatic backups.
    pub fn interval_secs(self) -> u64 {
        match self {
            BackupCadence::Daily => 24 * 60 * 60,
            BackupCadence::Weekly => 7 * 24 * 60 * 60,
        }
    }
}

fn default_backup_keep_last() -> u32 {
    5
}

/// Optional automatic backup schedule and retention policy for an instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupSchedule {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub cadence: BackupCadence,
    #[serde(default = "default_backup_keep_last")]
    pub keep_last: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
}

impl Default for BackupSchedule {
    fn default() -> Self {
        BackupSchedule {
            enabled: false,
            cadence: BackupCadence::Daily,
            keep_last: default_backup_keep_last(),
            last_run_at: None,
        }
    }
}

/// A database instance registered in state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Instance {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data_identity: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data_directory: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data_owner: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub package: String,
    #[serde(default)]
    pub port: i64,
    #[serde(default)]
    pub persistent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_schedule: Option<BackupSchedule>,
}

/// Instance-owned connection metadata. Credentials are never stored here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionProfile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub instance_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub database: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mongo_auth_source: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub redis_db: i64,
}

/// One local HTTP port served by the HTTPS proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyRule {
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub local_port: i64,
    /// Where the local port is: an IP address or host name. Empty means this
    /// Mac's loopback, 127.0.0.1.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub upstream_host: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tunnel: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tunnel_auth: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tunnel_worker: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tunnel_domain: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tunnel_url: String,
}

/// A Cloudflare account zone the UI can target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudflareZone {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// A Cloudflare DNS record cached in state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudflareDnsRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default)]
    pub zone_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub record_type: String,
}

/// A deployed Cloudflare Worker for one tunneled rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelWorkerState {
    #[serde(default)]
    pub script_name: String,
    #[serde(default)]
    pub public_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub custom_domain: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub zone_id: String,
    #[serde(default)]
    pub local_domain: String,
    #[serde(default)]
    pub local_port: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub script_digest: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_digest: String,
}

/// The complete `state.json` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uv_executable: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runtime_defaults: BTreeMap<String, String>,
    pub connection_profiles: BTreeMap<String, ConnectionProfile>,
    pub proxy_rules: Vec<ProxyRule>,
    pub proxy_listener_port: i64,
    #[serde(default)]
    pub proxy_rules_revision: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cloudflare_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cloudflare_subdomain: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cloudflare_zones: Vec<CloudflareZone>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cloudflare_dns: Vec<CloudflareDnsRecord>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tunnel_client_id: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tunnel_workers: BTreeMap<String, TunnelWorkerState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub package_integrity: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub log_rotation_disabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_default: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub favorite_ports: Vec<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watched_ports: Vec<i64>,
    pub schema_version: i64,
    pub packages: BTreeMap<String, Package>,
    pub instances: BTreeMap<String, Instance>,
}

impl Default for State {
    /// The exact `emptyState()` document: schema 2, empty collections, and
    /// the HTTPS listener on 443.
    fn default() -> Self {
        State {
            uv_executable: String::new(),
            runtime_defaults: BTreeMap::new(),
            connection_profiles: BTreeMap::new(),
            proxy_rules: Vec::new(),
            proxy_listener_port: 443,
            proxy_rules_revision: 0,
            cloudflare_account_id: String::new(),
            cloudflare_subdomain: String::new(),
            cloudflare_zones: Vec::new(),
            cloudflare_dns: Vec::new(),
            tunnel_client_id: String::new(),
            tunnel_workers: BTreeMap::new(),
            package_integrity: BTreeMap::new(),
            log_rotation_disabled: false,
            node_default: String::new(),
            favorite_ports: Vec::new(),
            watched_ports: Vec::new(),
            schema_version: SCHEMA,
            packages: BTreeMap::new(),
            instances: BTreeMap::new(),
        }
    }
}

/// The wire shape used for strict decoding. Go's `encoding/json` accepts a
/// missing field as its zero value, so every field is defaulted here; the
/// three collections that must be present are `Option` so absence and `null`
/// are distinguishable from an empty collection.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StateWire {
    #[serde(default)]
    uv_executable: String,
    #[serde(default)]
    runtime_defaults: Option<BTreeMap<String, String>>,
    #[serde(default)]
    connection_profiles: Option<BTreeMap<String, ConnectionProfile>>,
    #[serde(default)]
    proxy_rules: Option<Vec<ProxyRule>>,
    #[serde(default)]
    proxy_listener_port: i64,
    #[serde(default)]
    proxy_rules_revision: u64,
    #[serde(default)]
    cloudflare_account_id: String,
    #[serde(default)]
    cloudflare_subdomain: String,
    #[serde(default)]
    cloudflare_zones: Option<Vec<CloudflareZone>>,
    #[serde(default)]
    cloudflare_dns: Option<Vec<CloudflareDnsRecord>>,
    #[serde(default)]
    tunnel_client_id: String,
    #[serde(default)]
    tunnel_workers: Option<BTreeMap<String, TunnelWorkerState>>,
    #[serde(default)]
    package_integrity: Option<BTreeMap<String, String>>,
    #[serde(default)]
    log_rotation_disabled: bool,
    #[serde(default)]
    node_default: String,
    #[serde(default)]
    favorite_ports: Option<Vec<i64>>,
    #[serde(default)]
    watched_ports: Option<Vec<i64>>,
    #[serde(default)]
    schema_version: i64,
    #[serde(default)]
    packages: Option<BTreeMap<String, Package>>,
    #[serde(default)]
    instances: Option<BTreeMap<String, Instance>>,
}

fn normalize_saved_ports(ports: Vec<i64>, field: &str) -> Result<Vec<i64>, StateError> {
    let mut normalized = std::collections::BTreeSet::new();
    for port in ports {
        if !(1..=65535).contains(&port) {
            return Err(StateError::invalid(format!(
                "invalid port {port} in {field}: must be 1–65535"
            )));
        }
        normalized.insert(port);
    }
    Ok(normalized.into_iter().collect())
}

impl State {
    /// Strictly validate a decoded wire document. The caller has already
    /// checked `schemaVersion`.
    pub(crate) fn from_wire(wire: StateWire) -> Result<State, StateError> {
        let packages = wire
            .packages
            .ok_or_else(|| StateError::invalid("incomplete state"))?;
        let instances = wire
            .instances
            .ok_or_else(|| StateError::invalid("incomplete state"))?;
        let proxy_rules = wire
            .proxy_rules
            .ok_or_else(|| StateError::invalid("incomplete state"))?;
        if !(1..=65535).contains(&wire.proxy_listener_port) {
            return Err(StateError::invalid("incomplete state"));
        }
        let favorite_ports =
            normalize_saved_ports(wire.favorite_ports.unwrap_or_default(), "favoritePorts")?;
        let watched_ports =
            normalize_saved_ports(wire.watched_ports.unwrap_or_default(), "watchedPorts")?;
        let state = State {
            uv_executable: wire.uv_executable,
            runtime_defaults: wire.runtime_defaults.unwrap_or_default(),
            connection_profiles: wire.connection_profiles.unwrap_or_default(),
            proxy_rules,
            proxy_listener_port: wire.proxy_listener_port,
            proxy_rules_revision: wire.proxy_rules_revision,
            cloudflare_account_id: wire.cloudflare_account_id,
            cloudflare_subdomain: wire.cloudflare_subdomain,
            cloudflare_zones: wire.cloudflare_zones.unwrap_or_default(),
            cloudflare_dns: wire.cloudflare_dns.unwrap_or_default(),
            tunnel_client_id: wire.tunnel_client_id,
            tunnel_workers: wire.tunnel_workers.unwrap_or_default(),
            package_integrity: wire.package_integrity.unwrap_or_default(),
            log_rotation_disabled: wire.log_rotation_disabled,
            node_default: wire.node_default,
            favorite_ports,
            watched_ports,
            schema_version: wire.schema_version,
            packages,
            instances,
        };
        for (id, profile) in &state.connection_profiles {
            let Some(instance) = state.instances.get(&profile.instance_id) else {
                return Err(StateError::invalid("invalid connection profile state"));
            };
            if id != &profile.id || validate_connection_profile(profile, &instance.kind).is_err() {
                return Err(StateError::invalid("invalid connection profile state"));
            }
        }
        normalize_proxy_rules(state.proxy_listener_port, &state.proxy_rules)
            .map_err(|error| StateError::invalid(format!("invalid state proxy rules: {error}")))?;
        validate_tunnel_state(&state).map_err(|error| {
            StateError::invalid(format!("invalid state tunnel settings: {error}"))
        })?;
        Ok(state)
    }
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = StateWire::deserialize(deserializer)?;
        State::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

/// Decode a state document strictly (unknown fields and trailing data are
/// errors). The schema version must already have been checked by the caller.
pub fn decode_state_strict(bytes: &[u8]) -> Result<State, StateError> {
    let wire: StateWire = serde_json::from_slice(bytes).map_err(StateError::ReadJson)?;
    State::from_wire(wire)
}

/// A validated identifier: the 80-character package/instance shape.
pub fn valid_id(value: &str) -> bool {
    NAME_RE.is_match(value)
}

fn profile_text(value: &str, required: bool) -> bool {
    if value.len() > 160 || (required && value.trim().is_empty()) {
        return false;
    }
    !value.contains(['\0', '\r', '\n', '\t'])
}

/// Validate profile metadata for the owning engine.
pub fn validate_connection_profile(
    profile: &ConnectionProfile,
    kind: &str,
) -> Result<(), StateError> {
    if !valid_id(&profile.id)
        || !valid_id(&profile.instance_id)
        || !profile_text(&profile.name, true)
    {
        return Err(StateError::invalid("invalid connection profile identity"));
    }
    if !profile_text(&profile.database, kind != "redis")
        || !profile_text(&profile.username, kind != "redis")
        || !profile_text(&profile.mongo_auth_source, false)
    {
        return Err(StateError::invalid("invalid connection profile field"));
    }
    match kind {
        "mysql" | "postgresql" => {
            if !profile.mongo_auth_source.is_empty() || profile.redis_db != 0 {
                return Err(StateError::invalid("unsupported connection profile option"));
            }
        }
        "mongodb" => {
            if profile.redis_db != 0 || profile.mongo_auth_source.is_empty() {
                return Err(StateError::invalid(
                    "MongoDB requires an authentication database",
                ));
            }
        }
        "redis" => {
            if (!profile.username.is_empty() && profile.username != "default")
                || !profile.database.is_empty()
                || !profile.mongo_auth_source.is_empty()
                || !(0..=15).contains(&profile.redis_db)
            {
                return Err(StateError::invalid(
                    "Redis supports only default user and database 0–15",
                ));
            }
        }
        _ => return Err(StateError::invalid("unsupported database engine")),
    }
    Ok(())
}

/// The full local domain for a rule name.
pub fn local_domain(name: &str) -> String {
    let name = name.trim().to_lowercase();
    if name.is_empty() || name.ends_with(LOCAL_DOMAIN_SUFFIX) {
        return name;
    }
    format!("{name}{LOCAL_DOMAIN_SUFFIX}")
}

/// Derive the Cloudflare Worker script name for a local domain.
pub fn default_tunnel_worker_name(domain: &str) -> String {
    let domain = domain.trim().to_lowercase();
    let stripped = domain.strip_suffix(LOCAL_DOMAIN_SUFFIX).unwrap_or(&domain);
    let slug = stripped.replace('.', "-");
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        return String::new();
    }
    let mut name = format!("devbase-{slug}");
    if name.len() > 63 {
        let mut cut = 63;
        while !name.is_char_boundary(cut) {
            cut -= 1;
        }
        name.truncate(cut);
        name = name.trim_end_matches('-').to_string();
    }
    name
}

impl ProxyRule {
    /// The Worker script name for this rule.
    pub fn resolved_tunnel_worker(&self) -> String {
        let worker = self.tunnel_worker.trim().to_lowercase();
        if !worker.is_empty() {
            return worker;
        }
        default_tunnel_worker_name(&self.domain)
    }

    /// The normalized custom domain, or empty for automatic workers.dev.
    pub fn effective_tunnel_domain(&self) -> String {
        normalize_tunnel_host(&self.tunnel_domain)
    }
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn normalize_tunnel_host(raw: &str) -> String {
    let mut host = raw.trim();
    if let Some(rest) = host.strip_prefix("https://") {
        host = rest;
    } else if let Some(rest) = host.strip_prefix("http://") {
        host = rest;
    }
    host.trim_end_matches('/').to_lowercase()
}

fn validate_public_tunnel_hostname(host: &str) -> Result<(), StateError> {
    if host.is_empty() {
        return Ok(());
    }
    if host.len() > 253 || host.contains(['/', ':', ' ', '\t', '\r', '\n']) {
        return Err(StateError::invalid(
            "custom domain must be a hostname without scheme or path",
        ));
    }
    if !host.contains('.') {
        return Err(StateError::invalid(
            "custom domain must include a zone domain (e.g. app.example.com)",
        ));
    }
    if host.ends_with(".localhost") {
        return Err(StateError::invalid(
            "custom domain cannot be a local .localhost domain",
        ));
    }
    if host.ends_with(".workers.dev") {
        return Err(StateError::invalid(
            "clear Custom domain to use automatic workers.dev URLs",
        ));
    }
    for part in host.split('.') {
        if !PROXY_DOMAIN_PART_RE.is_match(part) {
            return Err(StateError::invalid(format!(
                "invalid custom domain {host:?}"
            )));
        }
    }
    Ok(())
}

fn normalize_upstream_host(host: &str) -> Result<String, StateError> {
    let host = host.trim().to_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host {
        "" | "127.0.0.1" | "localhost" => return Ok(String::new()),
        _ => {}
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_unspecified() || ip.is_multicast() {
            return Err(StateError::invalid(
                "upstream address must be a host, not 0.0.0.0, :: or a multicast address",
            ));
        }
        return Ok(ip.to_string());
    }
    if host.len() > 253 || host.contains([':', '/', '@', '?', '#', '%', ' ', '\t']) {
        return Err(StateError::invalid(
            "upstream must be an IP address or a host name, without a scheme, port or path",
        ));
    }
    let trimmed = host.trim_end_matches('.');
    for part in trimmed.split('.') {
        if !PROXY_DOMAIN_PART_RE.is_match(part) {
            return Err(StateError::invalid(
                "upstream must be an IP address or a valid host name",
            ));
        }
    }
    Ok(trimmed.to_string())
}

/// Normalize and validate the complete proxy rule table.
pub fn normalize_proxy_rules(
    listener_port: i64,
    rules: &[ProxyRule],
) -> Result<Vec<ProxyRule>, StateError> {
    if !(1..=65535).contains(&listener_port) {
        return Err(StateError::invalid("HTTPS listener port must be 1–65535"));
    }
    let mut normalized = Vec::with_capacity(rules.len());
    let mut seen = std::collections::HashSet::with_capacity(rules.len());
    let mut seen_workers = std::collections::HashSet::with_capacity(rules.len());
    let mut seen_tunnel_domains = std::collections::HashSet::with_capacity(rules.len());
    for (index, rule) in rules.iter().enumerate() {
        let number = index + 1;
        let mut rule = rule.clone();
        rule.domain = rule.domain.trim().to_lowercase();
        if rule.domain.len() > 253
            || !rule.domain.ends_with(LOCAL_DOMAIN_SUFFIX)
            || rule.domain.len() == LOCAL_DOMAIN_SUFFIX.len()
        {
            return Err(StateError::invalid(format!(
                "rule {number}: use a name under {LOCAL_DOMAIN_SUFFIX}"
            )));
        }
        for part in rule.domain.split('.') {
            if !PROXY_DOMAIN_PART_RE.is_match(part) {
                return Err(StateError::invalid(format!(
                    "rule {number}: invalid domain"
                )));
            }
        }
        let host = normalize_upstream_host(&rule.upstream_host)
            .map_err(|error| StateError::invalid(format!("rule {number}: {error}")))?;
        rule.upstream_host = host.clone();
        // The HTTPS port only collides with a server on this Mac's loopback.
        if !(1..=65535).contains(&rule.local_port)
            || (host.is_empty() && rule.local_port == listener_port)
        {
            return Err(StateError::invalid(format!(
                "rule {number}: local port must be 1–65535 and differ from HTTPS port"
            )));
        }
        if !seen.insert(rule.domain.clone()) {
            return Err(StateError::invalid(format!(
                "duplicate proxy domain {}",
                rule.domain
            )));
        }
        rule.tunnel_worker = rule.tunnel_worker.trim().to_lowercase();
        if !rule.tunnel_worker.is_empty()
            && (!PROXY_DOMAIN_PART_RE.is_match(&rule.tunnel_worker)
                || !rule.tunnel_worker.starts_with("devbase-")
                || rule.tunnel_worker.len() <= "devbase-".len())
        {
            return Err(StateError::invalid(format!(
                "rule {number}: worker name must start with devbase- and use valid DNS label characters"
            )));
        }
        rule.tunnel_domain = normalize_tunnel_host(&rule.tunnel_domain);
        validate_public_tunnel_hostname(&rule.tunnel_domain)
            .map_err(|error| StateError::invalid(format!("rule {number}: {error}")))?;
        rule.tunnel_url = rule.tunnel_url.trim().to_string();
        if rule.tunnel {
            let worker = rule.resolved_tunnel_worker();
            if !PROXY_DOMAIN_PART_RE.is_match(&worker)
                || !worker.starts_with("devbase-")
                || worker.len() <= "devbase-".len()
            {
                return Err(StateError::invalid(format!(
                    "rule {number}: invalid worker name {worker:?}"
                )));
            }
            if !seen_workers.insert(worker.clone()) {
                return Err(StateError::invalid(format!(
                    "duplicate tunnel worker {worker}"
                )));
            }
            if !rule.tunnel_domain.is_empty()
                && !seen_tunnel_domains.insert(rule.tunnel_domain.clone())
            {
                return Err(StateError::invalid(format!(
                    "duplicate tunnel custom domain {}",
                    rule.tunnel_domain
                )));
            }
        } else {
            rule.tunnel_url.clear();
        }
        normalized.push(rule);
    }
    Ok(normalized)
}

fn normalize_tunnel_client_id(raw: &str) -> Result<String, StateError> {
    let id = raw.trim().to_lowercase();
    if id.is_empty() {
        return Ok(id);
    }
    if !TUNNEL_CLIENT_ID_RE.is_match(&id) {
        return Err(StateError::invalid(
            "client ID must be 2–40 lowercase letters, digits, or hyphens",
        ));
    }
    Ok(id)
}

fn normalize_cloudflare_account_id(raw: &str) -> Result<String, StateError> {
    let id = raw.trim();
    if id.is_empty() {
        return Ok(String::new());
    }
    if !CLOUDFLARE_ACCOUNT_ID_RE.is_match(id) {
        return Err(StateError::invalid(
            "Cloudflare account ID must be 8–64 alphanumeric characters, hyphens, or underscores",
        ));
    }
    Ok(id.to_string())
}

/// Validate the tunnel-related fields of a decoded state document.
pub fn validate_tunnel_state(state: &State) -> Result<(), StateError> {
    if normalize_tunnel_client_id(&state.tunnel_client_id)? != state.tunnel_client_id {
        return Err(StateError::invalid("tunnel client ID is not normalized"));
    }
    if normalize_cloudflare_account_id(&state.cloudflare_account_id)? != state.cloudflare_account_id
    {
        return Err(StateError::invalid(
            "Cloudflare account ID is not normalized",
        ));
    }
    let subdomain = state.cloudflare_subdomain.trim();
    if !subdomain.is_empty()
        && (subdomain.to_lowercase() != subdomain || !PROXY_DOMAIN_PART_RE.is_match(subdomain))
    {
        return Err(StateError::invalid(
            "Cloudflare workers.dev subdomain is invalid",
        ));
    }
    for (name, worker) in &state.tunnel_workers {
        if !name.starts_with("devbase-")
            || worker.script_name != *name
            || !PROXY_DOMAIN_PART_RE.is_match(name)
        {
            return Err(StateError::invalid(format!(
                "invalid deployed tunnel worker {name:?}"
            )));
        }
        if worker.public_url.is_empty() || !(1..=65535).contains(&worker.local_port) {
            return Err(StateError::invalid(format!(
                "incomplete tunnel worker state {name:?}"
            )));
        }
        if !worker.script_digest.is_empty()
            && !TUNNEL_SCRIPT_DIGEST_RE.is_match(&worker.script_digest)
        {
            return Err(StateError::invalid(format!(
                "invalid tunnel worker digest {name:?}"
            )));
        }
        if !worker.auth_digest.is_empty() && !TUNNEL_SCRIPT_DIGEST_RE.is_match(&worker.auth_digest)
        {
            return Err(StateError::invalid(format!(
                "invalid tunnel worker auth digest {name:?}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_digest() -> String {
        "a".repeat(64)
    }

    fn linux_archive(arch: &str, format: &str) -> Package {
        Package {
            name: "mysql".into(),
            version: "8.4.13".into(),
            arch: arch.into(),
            url: "https://dev.mysql.com/get/Downloads/MySQL-8.4/mysql-8.4.13-linux-glibc2.28-x86_64.tar.xz".into(),
            sha256: sample_digest(),
            format: format.into(),
            strip: 1,
            binaries: BTreeMap::from([("mysqld".into(), "bin/mysqld".into())]),
            license: "GPL-2.0".into(),
            ..Package::default()
        }
    }

    fn system_postgres(arch: &str) -> Package {
        Package {
            name: "postgresql".into(),
            version: "17.4".into(),
            arch: arch.into(),
            url: String::new(),
            sha256: sample_digest(),
            format: "system".into(),
            strip: 0,
            binaries: BTreeMap::from([
                ("postgres".into(), "/usr/bin/postgres".into()),
                ("initdb".into(), "/usr/bin/initdb".into()),
                ("pg_dump".into(), "/usr/bin/pg_dump".into()),
                ("pg_restore".into(), "/usr/bin/pg_restore".into()),
                ("psql".into(), "/usr/bin/psql".into()),
                ("createdb".into(), "/usr/bin/createdb".into()),
            ]),
            license: "PostgreSQL".into(),
            ..Package::default()
        }
    }

    #[test]
    fn model_validation_respects_macos_and_linux_targets() {
        // Linux x86_64 accepts amd64 & x86_64 and tar.xz
        assert!(
            linux_archive("amd64", "tar.xz")
                .validate_for_target(HostTarget::LINUX_AMD64)
                .is_ok()
        );
        assert!(
            linux_archive("x86_64", "tar.xz")
                .validate_for_target(HostTarget::LINUX_AMD64)
                .is_ok()
        );
        assert!(
            linux_archive("arm64", "tar.xz")
                .validate_for_target(HostTarget::LINUX_AMD64)
                .is_err()
        );

        // Linux aarch64 accepts arm64 & aarch64 and tar.xz
        assert!(
            linux_archive("arm64", "tar.xz")
                .validate_for_target(HostTarget::LINUX_ARM64)
                .is_ok()
        );
        assert!(
            linux_archive("aarch64", "tar.xz")
                .validate_for_target(HostTarget::LINUX_ARM64)
                .is_ok()
        );
        assert!(
            linux_archive("amd64", "tar.xz")
                .validate_for_target(HostTarget::LINUX_ARM64)
                .is_err()
        );

        // macOS rejects amd64 and tar.xz
        assert!(
            linux_archive("amd64", "tar.gz")
                .validate_for_target(HostTarget::MACOS_ARM64)
                .is_err()
        );
        assert!(
            linux_archive("arm64", "tar.xz")
                .validate_for_target(HostTarget::MACOS_ARM64)
                .is_err()
        );
        assert!(
            linux_archive("arm64", "tar.gz")
                .validate_for_target(HostTarget::MACOS_ARM64)
                .is_ok()
        );
    }

    #[test]
    fn linux_system_package_validation_accepts_postgres_and_redis_only() {
        let pg = system_postgres("amd64");
        assert!(pg.validate_for_target(HostTarget::LINUX_AMD64).is_ok());
        assert!(pg.validate_for_target(HostTarget::MACOS_ARM64).is_err());

        let mut traversal = system_postgres("amd64");
        traversal
            .binaries
            .insert("postgres".into(), "/usr/bin/../bin/postgres".into());
        assert!(
            traversal
                .validate_for_target(HostTarget::LINUX_AMD64)
                .is_err()
        );

        let mut relative = system_postgres("amd64");
        relative
            .binaries
            .insert("postgres".into(), "bin/postgres".into());
        assert!(
            relative
                .validate_for_target(HostTarget::LINUX_AMD64)
                .is_err()
        );

        let redis = Package {
            name: "redis".into(),
            version: "7.4.2".into(),
            arch: "arm64".into(),
            sha256: sample_digest(),
            format: "system".into(),
            binaries: BTreeMap::from([
                ("redis-server".into(), "/usr/bin/redis-server".into()),
                ("redis-cli".into(), "/usr/bin/redis-cli".into()),
            ]),
            ..Package::default()
        };
        assert!(redis.validate_for_target(HostTarget::LINUX_ARM64).is_ok());
        assert!(redis.validate_for_target(HostTarget::MACOS_ARM64).is_err());
    }
}
