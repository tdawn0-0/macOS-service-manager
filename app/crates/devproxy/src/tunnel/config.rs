//! The published tunnel snapshot, live status file, and route validation.
//!
//! Ported from `tunnel_config.go`. `proxy/tunnel.json` (schema 1) describes the
//! Workers the background process should connect to; `proxy/tunnel-status.json`
//! is the only IPC back to the GUI.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use devcore::fs::{PRIVATE_FILE, atomic_write};
use devcore::model::{ProxyRule, State};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::secrets::SecretStore;

/// Errors from tunnel snapshots and status.
#[derive(Debug, thiserror::Error)]
pub enum TunnelConfigError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    State(#[from] devcore::state::StateError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

/// One deployed Worker mapped to a local loopback port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelRoute {
    pub worker_name: String,
    pub public_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connect_url: String,
    pub local_domain: String,
    pub local_port: i64,
    /// Empty means 127.0.0.1.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub upstream_host: String,
}

impl TunnelRoute {
    /// The public hostname, lowercased.
    pub fn public_host(&self) -> String {
        Url::parse(&self.public_url)
            .ok()
            .and_then(|url| url.host_str().map(|host| host.to_lowercase()))
            .unwrap_or_default()
    }

    /// The `/__devbase/connect` WebSocket URL derived from the public URL.
    pub fn web_socket_url(&self) -> Result<String, TunnelConfigError> {
        let base = if self.connect_url.trim().is_empty() {
            self.public_url.trim()
        } else {
            self.connect_url.trim()
        };
        let mut url = Url::parse(base)
            .map_err(|_| TunnelConfigError::Invalid("invalid tunnel route URL".into()))?;
        if url.host_str().is_none() {
            return Err(TunnelConfigError::Invalid(
                "invalid tunnel route URL".into(),
            ));
        }
        let scheme = match url.scheme().to_lowercase().as_str() {
            "https" => "wss".to_string(),
            "http" => "ws".to_string(),
            "wss" => "wss".to_string(),
            "ws" => "ws".to_string(),
            _ => {
                return Err(TunnelConfigError::Invalid(
                    "unsupported tunnel URL scheme".into(),
                ));
            }
        };
        url.set_scheme(&scheme)
            .map_err(|_| TunnelConfigError::Invalid("unsupported tunnel URL scheme".into()))?;
        url.set_path("/__devbase/connect");
        url.set_query(None);
        url.set_fragment(None);
        Ok(url.to_string())
    }

    /// The `Host` the local upstream sees for tunneled requests.
    pub fn local_host_header(&self) -> String {
        if self.upstream_host.is_empty() {
            format!("localhost:{}", self.local_port)
        } else {
            format!("{}:{}", self.upstream_host, self.local_port)
        }
    }

    /// The local HTTP origin for this route.
    pub fn upstream_origin(&self) -> String {
        let host = if self.upstream_host.is_empty() {
            "127.0.0.1"
        } else {
            &self.upstream_host
        };
        format!("http://{host}:{}", self.local_port)
    }
}

/// The complete `proxy/tunnel.json` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelPublishedConfig {
    pub schema_version: i64,
    pub client_id: String,
    #[serde(default)]
    pub routes: Vec<TunnelRoute>,
}

/// Derive a validated tunnel snapshot from saved state.
pub fn tunnel_config_from_state(state: &State) -> Result<TunnelPublishedConfig, TunnelConfigError> {
    let client_id = if state.tunnel_client_id.is_empty() {
        format!("mac-{}", short_id())
    } else {
        state.tunnel_client_id.clone()
    };
    let mut routes = Vec::new();
    for rule in &state.proxy_rules {
        if !rule.tunnel {
            continue;
        }
        let worker_name = rule.resolved_tunnel_worker();
        let public_url = state
            .tunnel_workers
            .get(&worker_name)
            .filter(|worker| !worker.public_url.is_empty())
            .map(|worker| worker.public_url.clone())
            .or_else(|| (!rule.tunnel_url.is_empty()).then(|| rule.tunnel_url.clone()))
            .ok_or_else(|| {
                TunnelConfigError::Invalid(format!(
                    "deploy Cloudflare Worker before starting tunnel for {}",
                    rule.domain
                ))
            })?;
        routes.push(TunnelRoute {
            worker_name,
            public_url: public_url.clone(),
            connect_url: public_url,
            local_domain: rule.domain.clone(),
            local_port: rule.local_port,
            upstream_host: rule.upstream_host.clone(),
        });
    }
    routes.sort_by(|a, b| a.worker_name.cmp(&b.worker_name));
    Ok(TunnelPublishedConfig {
        schema_version: 1,
        client_id,
        routes,
    })
}

fn short_id() -> String {
    let raw: [u8; 8] = rand::random();
    hex::encode(raw)[..8].to_string()
}

/// The published tunnel snapshot path.
pub fn published_config_path(root: &Path) -> PathBuf {
    root.join("proxy").join("tunnel.json")
}

/// Parse and strictly validate a tunnel snapshot.
pub fn parse_published_config(bytes: &[u8]) -> Result<TunnelPublishedConfig, TunnelConfigError> {
    let config: TunnelPublishedConfig = serde_json::from_slice(bytes)?;
    if config.schema_version != 1 {
        return Err(TunnelConfigError::Invalid(
            "unsupported tunnel snapshot schema".into(),
        ));
    }
    if config.client_id.is_empty() {
        return Err(TunnelConfigError::Invalid(
            "tunnel snapshot missing client ID".into(),
        ));
    }
    if !valid_client_id(&config.client_id) {
        return Err(TunnelConfigError::Invalid(
            "client ID must be 2–40 lowercase letters, digits, or hyphens".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for route in &config.routes {
        if !route.worker_name.starts_with("devbase-")
            || route.public_url.is_empty()
            || !(1..=65535).contains(&route.local_port)
        {
            return Err(TunnelConfigError::Invalid(
                "invalid tunnel route in snapshot".into(),
            ));
        }
        let upstream = normalize_upstream_host(&route.upstream_host)?;
        if upstream != route.upstream_host {
            return Err(TunnelConfigError::Invalid(
                "invalid tunnel route upstream in snapshot".into(),
            ));
        }
        if !seen.insert(route.worker_name.clone()) {
            return Err(TunnelConfigError::Invalid(format!(
                "duplicate tunnel route {}",
                route.worker_name
            )));
        }
        if route.public_host().is_empty() {
            return Err(TunnelConfigError::Invalid(
                "invalid tunnel route URL".into(),
            ));
        }
        route.web_socket_url()?;
    }
    Ok(config)
}

/// Read and strictly validate `proxy/tunnel.json`.
pub fn read_published_config(root: &Path) -> Result<TunnelPublishedConfig, TunnelConfigError> {
    let bytes = std::fs::read(published_config_path(root))?;
    parse_published_config(&bytes)
}

/// Read `proxy/tunnel.json` together with the digest that guards reloads.
pub fn read_published_config_with_digest(
    root: &Path,
) -> Result<(TunnelPublishedConfig, [u8; 32]), TunnelConfigError> {
    let bytes = std::fs::read(published_config_path(root))?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    Ok((parse_published_config(&bytes)?, digest))
}

/// Write `proxy/tunnel.json` atomically with mode 0600.
pub fn write_published_config(
    root: &Path,
    config: &TunnelPublishedConfig,
) -> Result<(), TunnelConfigError> {
    let mut bytes = serde_json::to_vec_pretty(config)?;
    bytes.push(b'\n');
    atomic_write(&published_config_path(root), &bytes, PRIVATE_FILE)?;
    Ok(())
}

/// Ensure the snapshot exists for the given state, persisting a generated
/// client ID into `state`.
pub fn write_config_from_state(
    root: &Path,
    state: &mut State,
) -> Result<TunnelPublishedConfig, TunnelConfigError> {
    let config = tunnel_config_from_state(state)?;
    if state.tunnel_client_id.is_empty() {
        state.tunnel_client_id = config.client_id.clone();
    }
    write_published_config(root, &config)?;
    Ok(config)
}

/// Whether a published tunnel snapshot already exists.
pub fn published_config_exists(root: &Path) -> bool {
    std::fs::metadata(published_config_path(root)).is_ok()
}

/// A hostname or Worker already claimed by another active client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelConflict {
    pub host: String,
    pub owner: String,
}

/// The live status document written by the background tunnel process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelStatusSnapshot {
    pub pid: i64,
    pub phase: String,
    pub connected: i64,
    pub total: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<TunnelConflict>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub worker_states: std::collections::BTreeMap<String, String>,
    pub updated_at: DateTime<Utc>,
}

impl Default for TunnelStatusSnapshot {
    fn default() -> Self {
        TunnelStatusSnapshot {
            pid: 0,
            phase: "Stopped".into(),
            connected: 0,
            total: 0,
            conflicts: Vec::new(),
            worker_states: Default::default(),
            updated_at: Utc::now(),
        }
    }
}

/// Write the status snapshot atomically, stamping `updatedAt`.
pub fn write_status(
    root: &Path,
    mut status: TunnelStatusSnapshot,
) -> Result<(), TunnelConfigError> {
    status.updated_at = Utc::now();
    let mut bytes = serde_json::to_vec_pretty(&status)?;
    bytes.push(b'\n');
    let secrets = SecretStore::new(root);
    secrets.init().map_err(|error| match error {
        crate::secrets::SecretError::Io(error) => TunnelConfigError::Io(error),
        other => TunnelConfigError::Invalid(other.to_string()),
    })?;
    atomic_write(&secrets.tunnel_status_path(), &bytes, PRIVATE_FILE)?;
    Ok(())
}

/// Read the status snapshot; a missing or corrupt file is simply absent.
pub fn read_status(root: &Path) -> Option<TunnelStatusSnapshot> {
    let bytes = std::fs::read(SecretStore::new(root).tunnel_status_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The state field validation the snapshot decoder relies on.
pub fn valid_client_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (2..=40).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Mirror `normalizeUpstreamHost`: empty, loopback spellings collapse to empty;
/// IPs are canonical; hostnames are lowercased and validated.
pub fn normalize_upstream_host(host: &str) -> Result<String, TunnelConfigError> {
    let host = host.trim().to_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if matches!(host, "" | "127.0.0.1" | "localhost") {
        return Ok(String::new());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip.is_unspecified() || ip.is_multicast() {
            return Err(TunnelConfigError::Invalid(
                "upstream address must be a host, not 0.0.0.0, :: or a multicast address".into(),
            ));
        }
        return Ok(ip.to_string());
    }
    if host.len() > 253 || host.contains([':', '/', '@', '?', '#', '%', ' ', '\t']) {
        return Err(TunnelConfigError::Invalid(
            "upstream must be an IP address or a host name, without a scheme, port or path".into(),
        ));
    }
    let trimmed = host.trim_end_matches('.');
    for part in trimmed.split('.') {
        if !valid_dns_label(part) {
            return Err(TunnelConfigError::Invalid(
                "upstream must be an IP address or a valid host name".into(),
            ));
        }
    }
    Ok(trimmed.to_string())
}

fn valid_dns_label(label: &str) -> bool {
    if label.is_empty() || label.len() > 63 {
        return false;
    }
    let bytes = label.as_bytes();
    let first = bytes[0];
    let last = bytes[bytes.len() - 1];
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && (last.is_ascii_lowercase() || last.is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Resolve the Worker for a rule's local domain.
pub fn worker_for_domain(state: &State, domain: &str) -> String {
    let domain = devcore::model::local_domain(domain);
    for rule in &state.proxy_rules {
        if rule.domain == domain {
            return rule.resolved_tunnel_worker();
        }
    }
    devcore::model::default_tunnel_worker_name(&domain)
}

/// Whether the saved tunnel target differs from what is deployed.
pub fn needs_sync(state: &State, secrets: &SecretStore, current_worker_digest: &str) -> bool {
    let mut desired = BTreeSet::new();
    for rule in &state.proxy_rules {
        if !rule.tunnel {
            continue;
        }
        let name = rule.resolved_tunnel_worker();
        desired.insert(name.clone());
        let Some(worker) = state.tunnel_workers.get(&name) else {
            if rule.tunnel_url.is_empty() {
                return true;
            }
            continue;
        };
        let mut want_auth_digest = String::new();
        if rule.tunnel_auth {
            let token = secrets.tunnel_access_token(&name).unwrap_or_default();
            if token.is_empty() {
                return true;
            }
            want_auth_digest = crate::secrets::tunnel_auth_digest(&token);
        }
        if worker.public_url.is_empty()
            || worker.custom_domain != rule.effective_tunnel_domain()
            || worker.script_digest != current_worker_digest
            || worker.auth_digest != want_auth_digest
        {
            return true;
        }
    }
    state
        .tunnel_workers
        .keys()
        .any(|name| !desired.contains(name))
}

/// The tunnel rule's validated upstream origin from state.
pub fn rule_upstream(rule: &ProxyRule) -> String {
    let host = if rule.upstream_host.is_empty() {
        "127.0.0.1"
    } else {
        &rule.upstream_host
    };
    format!("http://{host}:{}", rule.local_port)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use devcore::model::TunnelWorkerState;
    use tempfile::TempDir;

    use super::*;

    fn rule(domain: &str, port: i64) -> ProxyRule {
        ProxyRule {
            domain: domain.into(),
            local_port: port,
            tunnel: true,
            ..ProxyRule::default()
        }
    }

    #[test]
    fn snapshot_publishes_routes_sorted_by_worker() {
        let state = State {
            proxy_rules: vec![
                rule("z.devbase.localhost", 3001),
                rule("a.devbase.localhost", 3000),
                ProxyRule {
                    domain: "plain.devbase.localhost".into(),
                    local_port: 3002,
                    ..ProxyRule::default()
                },
            ],
            tunnel_workers: [
                (
                    "devbase-z".into(),
                    TunnelWorkerState {
                        script_name: "devbase-z".into(),
                        public_url: "https://devbase-z.example.workers.dev".into(),
                        local_domain: "z.devbase.localhost".into(),
                        local_port: 3001,
                        ..TunnelWorkerState::default()
                    },
                ),
                (
                    "devbase-a".into(),
                    TunnelWorkerState {
                        script_name: "devbase-a".into(),
                        public_url: "https://devbase-a.example.workers.dev".into(),
                        local_domain: "a.devbase.localhost".into(),
                        local_port: 3000,
                        ..TunnelWorkerState::default()
                    },
                ),
            ]
            .into(),
            ..State::default()
        };
        let config = tunnel_config_from_state(&state).unwrap();
        assert_eq!(config.schema_version, 1);
        assert!(valid_client_id(&config.client_id));
        assert_eq!(config.routes.len(), 2);
        assert_eq!(config.routes[0].worker_name, "devbase-a");
        assert_eq!(config.routes[0].connect_url, config.routes[0].public_url);
    }

    #[test]
    fn missing_public_url_is_an_actionable_error() {
        let state = State {
            proxy_rules: vec![rule("app.devbase.localhost", 3000)],
            ..State::default()
        };
        let error = tunnel_config_from_state(&state).unwrap_err();
        assert_eq!(
            error.to_string(),
            "deploy Cloudflare Worker before starting tunnel for app.devbase.localhost"
        );
    }

    #[test]
    fn snapshot_is_strict_and_rejects_bad_routes() {
        let good = serde_json::json!({
            "schemaVersion": 1,
            "clientId": "mac-12345678",
            "routes": [{
                "workerName": "devbase-app",
                "publicUrl": "https://devbase-app.example.workers.dev",
                "localDomain": "app.devbase.localhost",
                "localPort": 3000
            }]
        });
        assert!(parse_published_config(&serde_json::to_vec(&good).unwrap()).is_ok());

        let mut unknown = good.clone();
        unknown["extra"] = serde_json::json!(1);
        assert!(parse_published_config(&serde_json::to_vec(&unknown).unwrap()).is_err());

        let mut trailing = serde_json::to_vec(&good).unwrap();
        trailing.push(b'x');
        assert!(parse_published_config(&trailing).is_err());

        for (key, value) in [
            ("schemaVersion", serde_json::json!(2)),
            ("clientId", serde_json::json!("")),
            ("clientId", serde_json::json!("BAD")),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(
                parse_published_config(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "{key}"
            );
        }

        let mut duplicate = good.clone();
        let route = duplicate["routes"][0].clone();
        duplicate["routes"].as_array_mut().unwrap().push(route);
        assert!(parse_published_config(&serde_json::to_vec(&duplicate).unwrap()).is_err());

        let mut bad_upstream = good;
        bad_upstream["routes"][0]["upstreamHost"] = serde_json::json!("http://x");
        assert!(parse_published_config(&serde_json::to_vec(&bad_upstream).unwrap()).is_err());
    }

    #[test]
    fn websocket_url_rewrites_scheme_and_path() {
        let mut route = TunnelRoute {
            worker_name: "devbase-app".into(),
            public_url: "https://devbase-app.example.workers.dev/some?x=1#f".into(),
            connect_url: String::new(),
            local_domain: "app.devbase.localhost".into(),
            local_port: 3000,
            upstream_host: String::new(),
        };
        assert_eq!(
            route.web_socket_url().unwrap(),
            "wss://devbase-app.example.workers.dev/__devbase/connect"
        );
        route.connect_url = "http://localhost:8787/base".into();
        assert_eq!(
            route.web_socket_url().unwrap(),
            "ws://localhost:8787/__devbase/connect"
        );
        route.connect_url = "ftp://localhost/".into();
        assert!(route.web_socket_url().is_err());
        assert_eq!(route.local_host_header(), "localhost:3000");
        assert_eq!(route.public_host(), "devbase-app.example.workers.dev");
    }

    #[test]
    fn status_is_private_and_round_trips() {
        let dir = TempDir::new().unwrap();
        let status = TunnelStatusSnapshot {
            phase: "Connected".into(),
            connected: 1,
            total: 2,
            worker_states: [("devbase-app".to_string(), "Connected".to_string())].into(),
            conflicts: vec![TunnelConflict {
                host: "app.example.com".into(),
                owner: "laptop".into(),
            }],
            ..TunnelStatusSnapshot::default()
        };
        write_status(dir.path(), status.clone()).unwrap();
        let stored = read_status(dir.path()).unwrap();
        assert_eq!(stored.phase, "Connected");
        assert_eq!(stored.worker_states["devbase-app"], "Connected");
        assert_eq!(stored.conflicts[0].owner, "laptop");
        let path = SecretStore::new(dir.path()).tunnel_status_path();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn sync_detects_missing_and_stale_workers() {
        let dir = TempDir::new().unwrap();
        let secrets = SecretStore::new(dir.path());
        let mut state = State {
            proxy_rules: vec![rule("app.devbase.localhost", 3000)],
            ..State::default()
        };
        assert!(needs_sync(&state, &secrets, "abcdef0123456789"));
        state.tunnel_workers.insert(
            "devbase-app".into(),
            TunnelWorkerState {
                script_name: "devbase-app".into(),
                public_url: "https://devbase-app.example.workers.dev".into(),
                local_domain: "app.devbase.localhost".into(),
                local_port: 3000,
                script_digest: "abcdef0123456789".into(),
                ..TunnelWorkerState::default()
            },
        );
        assert!(!needs_sync(&state, &secrets, "abcdef0123456789"));
        assert!(needs_sync(&state, &secrets, "0000000000000000"));

        state.proxy_rules[0].tunnel_auth = true;
        assert!(needs_sync(&state, &secrets, "abcdef0123456789"));
        let token = secrets.ensure_tunnel_access_token("devbase-app").unwrap();
        state
            .tunnel_workers
            .get_mut("devbase-app")
            .unwrap()
            .auth_digest = crate::secrets::tunnel_auth_digest(&token);
        assert!(!needs_sync(&state, &secrets, "abcdef0123456789"));
    }
}
