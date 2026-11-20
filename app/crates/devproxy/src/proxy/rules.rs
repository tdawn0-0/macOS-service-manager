//! Proxy rule tables, the published routing snapshot, and publication state.
//!
//! Ported from `proxy_rules.go`. `state.json` holds intent; `proxy/rules.json`
//! holds the last validated snapshot the HTTPS server reads. The snapshot is a
//! whole-table document at schema 1 and is decoded strictly, then re-normalized
//! with the same validator the GUI used.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use devcore::fs::{PRIVATE_FILE, atomic_write};
use devcore::model::{ProxyRule, State, normalize_proxy_rules};
use devcore::state::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::secrets::tunnel_auth_digest;

/// The only domain suffix a rule may use.
pub const LOCAL_DOMAIN_SUFFIX: &str = ".devbase.localhost";

/// Errors from snapshot decoding and publication.
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    State(#[from] devcore::state::StateError),
    #[error("{0}")]
    Invalid(String),
}

/// The complete on-disk routing snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedProxyConfig {
    #[serde(default)]
    pub schema_version: i64,
    #[serde(default)]
    pub listener_port: i64,
    #[serde(default)]
    pub rules: Vec<ProxyRule>,
}

impl PublishedProxyConfig {
    pub fn from_state(state: &State) -> Self {
        PublishedProxyConfig {
            schema_version: 1,
            listener_port: state.proxy_listener_port,
            rules: state.proxy_rules.clone(),
        }
    }

    pub fn from_routes(sites: &[ProxyRoute]) -> Self {
        let mut config = PublishedProxyConfig {
            schema_version: 1,
            listener_port: 443,
            rules: Vec::new(),
        };
        if let Some(first) = sites.first() {
            config.listener_port = first.listener_port;
        }
        config.rules = sites
            .iter()
            .map(|site| ProxyRule {
                domain: site.domain.clone(),
                local_port: site.local_port,
                upstream_host: site.upstream_host.clone(),
                ..ProxyRule::default()
            })
            .collect();
        config
    }
}

/// A derived transport entry, never a saved resource.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProxyRoute {
    pub domain: String,
    pub local_port: i64,
    /// Empty means this Mac's loopback, 127.0.0.1.
    pub upstream_host: String,
    pub listener_port: i64,
}

impl ProxyRoute {
    /// The address the rule forwards to, with `127.0.0.1` for loopback.
    pub fn upstream_host_port(&self) -> String {
        upstream_host_port(&self.upstream_host, self.local_port)
    }

    /// The HTTP origin of the local service.
    pub fn upstream_url(&self) -> String {
        format!("http://{}", self.upstream_host_port())
    }

    /// The live-table key: domain and listener port.
    pub fn route_id(&self) -> String {
        format!("{}:{}", self.domain, self.listener_port)
    }

    /// Validate one derived route. The saved rule table was validated already;
    /// this guards the published snapshot and the derived port set.
    pub fn validate(&self) -> Result<(), RuleError> {
        if self.domain.is_empty() {
            return Err(RuleError::Invalid("proxy route domain is empty".into()));
        }
        if !(1..=65535).contains(&self.local_port) {
            return Err(RuleError::Invalid(format!(
                "proxy route {} has an invalid local port",
                self.domain
            )));
        }
        if !(1..=65535).contains(&self.listener_port) {
            return Err(RuleError::Invalid(format!(
                "proxy route {} has an invalid HTTPS port",
                self.domain
            )));
        }
        if !self.upstream_host.is_empty() {
            let host = &self.upstream_host;
            if host.len() > 253
                || host.contains([':', '/', '@', '?', '#', '%', ' ', '\t'])
                || host.starts_with('-')
                || host.ends_with('-')
                || host.split('.').any(|part| {
                    part.is_empty()
                        || part.len() > 63
                        || part.starts_with('-')
                        || part.ends_with('-')
                })
            {
                return Err(RuleError::Invalid(format!(
                    "proxy route {} has an invalid upstream",
                    self.domain
                )));
            }
        }
        Ok(())
    }
}

/// The address a rule forwards to.
pub fn upstream_host_port(host: &str, port: i64) -> String {
    let host = if host.is_empty() { "127.0.0.1" } else { host };
    format!("{host}:{port}")
}

/// Derive sorted routes from saved state.
pub fn routes_from_state(state: &State) -> Vec<ProxyRoute> {
    let mut sites: Vec<ProxyRoute> = state
        .proxy_rules
        .iter()
        .map(|rule| ProxyRoute {
            domain: rule.domain.clone(),
            local_port: rule.local_port,
            upstream_host: rule.upstream_host.clone(),
            listener_port: state.proxy_listener_port,
        })
        .collect();
    sites.sort_by(|a, b| a.domain.cmp(&b.domain));
    sites
}

/// Derive sorted routes from a validated snapshot.
pub fn routes_from_config(config: &PublishedProxyConfig) -> Vec<ProxyRoute> {
    let mut sites: Vec<ProxyRoute> = config
        .rules
        .iter()
        .map(|rule| ProxyRoute {
            domain: rule.domain.clone(),
            local_port: rule.local_port,
            upstream_host: rule.upstream_host.clone(),
            listener_port: config.listener_port,
        })
        .collect();
    sites.sort_by(|a, b| a.domain.cmp(&b.domain));
    sites
}

/// Validate a complete route set and reject duplicate domain/port pairs.
pub fn validate_proxy_routes(sites: &[ProxyRoute]) -> Result<(), RuleError> {
    let mut seen = BTreeSet::new();
    for site in sites {
        site.validate()?;
        let key = site.route_id();
        if !seen.insert(key.clone()) {
            return Err(RuleError::Invalid(format!("duplicate proxy route {key}")));
        }
    }
    Ok(())
}

/// Distinct listener ports, sorted.
pub fn listener_ports(sites: &[ProxyRoute]) -> Vec<i64> {
    let mut ports: Vec<i64> = sites.iter().map(|site| site.listener_port).collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Whether any route needs a privileged (below 1024) listener.
pub fn privileged_ports(sites: &[ProxyRoute]) -> bool {
    sites.iter().any(|site| site.listener_port < 1024)
}

/// The `proxy/rules.json` path.
pub fn published_config_path(root: &Path) -> PathBuf {
    root.join("proxy").join("rules.json")
}

/// Parse and validate snapshot bytes. Unknown fields, trailing bytes, a wrong
/// schema version, and invalid rules are all rejected.
pub fn parse_published_config(bytes: &[u8]) -> Result<PublishedProxyConfig, RuleError> {
    let raw: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| RuleError::Invalid(format!("invalid proxy routing snapshot: {error}")))?;
    if !raw.get("rules").is_some_and(serde_json::Value::is_array) {
        return Err(RuleError::Invalid(
            "proxy routing snapshot must include a rules array".into(),
        ));
    }
    let mut config: PublishedProxyConfig = serde_json::from_slice(bytes)
        .map_err(|error| RuleError::Invalid(format!("invalid proxy routing snapshot: {error}")))?;
    if config.schema_version != 1 {
        return Err(RuleError::Invalid(format!(
            "unsupported proxy routing schema {}",
            config.schema_version
        )));
    }
    config.rules = normalize_proxy_rules(config.listener_port, &config.rules)?;
    Ok(config)
}

/// Read and validate `proxy/rules.json`.
pub fn read_published_config(root: &Path) -> Result<PublishedProxyConfig, RuleError> {
    let bytes = std::fs::read(published_config_path(root))?;
    parse_published_config(&bytes)
}

/// Write the snapshot atomically with mode 0600.
pub fn write_published_config(root: &Path, config: &PublishedProxyConfig) -> Result<(), RuleError> {
    let mut bytes =
        serde_json::to_vec_pretty(config).map_err(|error| RuleError::Invalid(error.to_string()))?;
    bytes.push(b'\n');
    atomic_write(&published_config_path(root), &bytes, PRIVATE_FILE)?;
    Ok(())
}

/// The publication state shown by the GUI: `Unknown`, `Pending`, or
/// `Published`.
pub fn publication_state(store: &Store) -> Result<&'static str, RuleError> {
    let state = store.read()?;
    let published = match read_published_config(store.root()) {
        Ok(published) => published,
        Err(_) => return Ok("Unknown"),
    };
    if published.listener_port != state.proxy_listener_port
        || !same_proxy_routing(&published.rules, &state.proxy_rules)
    {
        return Ok("Pending");
    }
    Ok("Published")
}

/// Compare what the HTTPS proxy routes: domain, local port, and upstream.
/// Tunnel settings belong to the Cloudflare tunnel and must not make a
/// published proxy look pending.
pub fn same_proxy_routing(a: &[ProxyRule], b: &[ProxyRule]) -> bool {
    let strip = |rules: &[ProxyRule]| -> Vec<(String, i64, String)> {
        let mut out: Vec<(String, i64, String)> = rules
            .iter()
            .map(|rule| {
                (
                    rule.domain.clone(),
                    rule.local_port,
                    rule.upstream_host.clone(),
                )
            })
            .collect();
        out.sort();
        out
    };
    strip(a) == strip(b)
}

/// The exact observation fingerprint of one proxied rule.
pub fn proxy_rule_fingerprint(listener_port: i64, rule: &ProxyRule) -> String {
    let payload = format!(
        "{listener_port}:{}:{}",
        rule.domain,
        upstream_host_port(&rule.upstream_host, rule.local_port)
    );
    hex::encode(Sha256::digest(payload.as_bytes()))
}

/// The public URL a tunneled rule currently targets, preferring deployed
/// Worker state over the preview derived from the account subdomain.
pub fn resolve_tunnel_public_url(state: &State, rule: &ProxyRule) -> String {
    if !rule.tunnel {
        return String::new();
    }
    let worker = rule.resolved_tunnel_worker();
    if let Some(worker_state) = state.tunnel_workers.get(&worker)
        && !worker_state.public_url.is_empty()
    {
        return worker_state.public_url.trim().to_string();
    }
    preview_tunnel_url(rule, &state.cloudflare_subdomain)
}

/// The public HTTPS URL a tunneled rule would have once deployed.
pub fn preview_tunnel_url(rule: &ProxyRule, cloudflare_subdomain: &str) -> String {
    if !rule.tunnel {
        return String::new();
    }
    let custom = rule.effective_tunnel_domain();
    if !custom.is_empty() {
        return format!("https://{custom}");
    }
    let subdomain = cloudflare_subdomain.trim().to_lowercase();
    if !subdomain.is_empty() {
        let worker = rule.resolved_tunnel_worker();
        if !worker.is_empty() {
            return format!("https://{worker}.{subdomain}.workers.dev");
        }
    }
    rule.tunnel_url.trim().to_string()
}

/// The fingerprint of a tunnel target, including the access-token digest when
/// protection is enabled.
pub fn tunnel_rule_fingerprint(rule: &ProxyRule, public_url: &str, auth_digest: &str) -> String {
    let payload = format!(
        "{}:{}:{}:{}:{}",
        rule.domain,
        upstream_host_port(&rule.upstream_host, rule.local_port),
        public_url,
        rule.tunnel_auth,
        auth_digest
    );
    hex::encode(Sha256::digest(payload.as_bytes()))
}

/// The access-token digest used by the tunnel fingerprint.
pub fn rule_auth_digest(token: &str) -> String {
    tunnel_auth_digest(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devcore::model::ProxyRule;
    use tempfile::TempDir;

    fn rule(domain: &str, port: i64) -> ProxyRule {
        ProxyRule {
            domain: domain.into(),
            local_port: port,
            ..ProxyRule::default()
        }
    }

    fn store_with(rules: Vec<ProxyRule>, port: i64, revision: u64) -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let mut state = store.read().unwrap();
        state.proxy_rules = rules;
        state.proxy_listener_port = port;
        state.proxy_rules_revision = revision;
        store.write_state(&state).unwrap();
        (dir, store)
    }

    #[test]
    fn snapshot_round_trips_and_strictly_decodes() {
        let dir = TempDir::new().unwrap();
        let config = PublishedProxyConfig {
            schema_version: 1,
            listener_port: 443,
            rules: vec![rule("app.devbase.localhost", 3000)],
        };
        write_published_config(dir.path(), &config).unwrap();
        assert_eq!(read_published_config(dir.path()).unwrap(), config);

        let base = serde_json::to_value(&config).unwrap();
        let mut unknown = base.clone();
        unknown["extra"] = serde_json::json!(true);
        assert!(parse_published_config(&serde_json::to_vec(&unknown).unwrap()).is_err());

        let mut schema = base.clone();
        schema["schemaVersion"] = serde_json::json!(2);
        assert!(parse_published_config(&serde_json::to_vec(&schema).unwrap()).is_err());

        let mut trailing = serde_json::to_vec(&base).unwrap();
        trailing.extend_from_slice(b"{}");
        assert!(parse_published_config(&trailing).is_err());

        let mut arbitrary = base.clone();
        arbitrary["rules"][0]["upstream"] = serde_json::json!("http://example.com");
        assert!(parse_published_config(&serde_json::to_vec(&arbitrary).unwrap()).is_err());

        let mut missing_rules = base;
        missing_rules.as_object_mut().unwrap().remove("rules");
        assert!(parse_published_config(&serde_json::to_vec(&missing_rules).unwrap()).is_err());
    }

    #[test]
    fn invalid_rules_are_rejected_on_decode() {
        let bad = serde_json::json!({
            "schemaVersion": 1,
            "listenerPort": 443,
            "rules": [{"domain": "example.com", "localPort": 3000}]
        });
        let error = parse_published_config(&serde_json::to_vec(&bad).unwrap()).unwrap_err();
        assert!(error.to_string().contains(".devbase.localhost"));

        let looped = serde_json::json!({
            "schemaVersion": 1,
            "listenerPort": 3000,
            "rules": [{"domain": "app.devbase.localhost", "localPort": 3000}]
        });
        assert!(parse_published_config(&serde_json::to_vec(&looped).unwrap()).is_err());
    }

    #[test]
    fn normalization_rejects_worker_and_domain_collisions() {
        let mut first = rule("a.devbase.localhost", 3000);
        first.tunnel = true;
        first.tunnel_worker = "devbase-shared".into();
        let mut second = rule("b.devbase.localhost", 3001);
        second.tunnel = true;
        second.tunnel_worker = "devbase-shared".into();
        let duplicate_worker = serde_json::json!({
            "schemaVersion": 1,
            "listenerPort": 443,
            "rules": [first, second]
        });
        let error =
            parse_published_config(&serde_json::to_vec(&duplicate_worker).unwrap()).unwrap_err();
        assert!(error.to_string().contains("duplicate tunnel worker"));

        let mut first = rule("a.devbase.localhost", 3000);
        first.tunnel = true;
        first.tunnel_domain = "same.example.com".into();
        let mut second = rule("b.devbase.localhost", 3001);
        second.tunnel = true;
        second.tunnel_domain = "same.example.com".into();
        let duplicate_domain = serde_json::json!({
            "schemaVersion": 1,
            "listenerPort": 443,
            "rules": [first, second]
        });
        let error =
            parse_published_config(&serde_json::to_vec(&duplicate_domain).unwrap()).unwrap_err();
        assert!(error.to_string().contains("duplicate tunnel custom domain"));

        let looped = serde_json::json!({
            "schemaVersion": 1,
            "listenerPort": 3000,
            "rules": [rule("a.devbase.localhost", 3000)]
        });
        assert!(parse_published_config(&serde_json::to_vec(&looped).unwrap()).is_err());

        let workers_dev = serde_json::json!({
            "schemaVersion": 1,
            "listenerPort": 443,
            "rules": [{
                "domain": "a.devbase.localhost",
                "localPort": 3000,
                "tunnel": true,
                "tunnelDomain": "app.example.workers.dev"
            }]
        });
        let error = parse_published_config(&serde_json::to_vec(&workers_dev).unwrap()).unwrap_err();
        assert!(error.to_string().contains("clear Custom domain"));
    }

    #[test]
    fn publication_state_tracks_routing_only() {
        let (_dir, store) = store_with(vec![rule("app.devbase.localhost", 3000)], 443, 1);
        assert_eq!(publication_state(&store).unwrap(), "Unknown");

        let mut state = store.read().unwrap();
        let config = PublishedProxyConfig::from_state(&state);
        write_published_config(store.root(), &config).unwrap();
        assert_eq!(publication_state(&store).unwrap(), "Published");

        state.proxy_rules[0].tunnel = true;
        state.proxy_rules[0].tunnel_url = "https://x.workers.dev".into();
        store.write_state(&state).unwrap();
        assert_eq!(
            publication_state(&store).unwrap(),
            "Published",
            "tunnel settings never make the proxy pending"
        );

        state.proxy_rules[0].local_port = 3001;
        store.write_state(&state).unwrap();
        assert_eq!(publication_state(&store).unwrap(), "Pending");
    }

    #[test]
    fn routes_derive_ports_and_reject_duplicates() {
        let state = State {
            proxy_listener_port: 443,
            proxy_rules: vec![
                rule("b.devbase.localhost", 3001),
                rule("a.devbase.localhost", 3000),
            ],
            ..State::default()
        };
        let sites = routes_from_state(&state);
        assert_eq!(sites[0].domain, "a.devbase.localhost");
        assert_eq!(listener_ports(&sites), vec![443]);
        assert!(privileged_ports(&sites));
        validate_proxy_routes(&sites).unwrap();

        let mut duplicate = sites.clone();
        duplicate.push(sites[0].clone());
        assert!(validate_proxy_routes(&duplicate).is_err());
    }

    #[test]
    fn fingerprints_are_stable_and_sensitive_to_routing() {
        let first = proxy_rule_fingerprint(443, &rule("app.devbase.localhost", 3000));
        assert_eq!(
            first,
            proxy_rule_fingerprint(443, &rule("app.devbase.localhost", 3000))
        );
        assert_ne!(
            first,
            proxy_rule_fingerprint(443, &rule("app.devbase.localhost", 3001))
        );
        assert_ne!(
            first,
            proxy_rule_fingerprint(8443, &rule("app.devbase.localhost", 3000))
        );

        let mut tunneled = rule("app.devbase.localhost", 3000);
        tunneled.tunnel = true;
        tunneled.tunnel_auth = true;
        let digest = rule_auth_digest("abcdefghijklmnop");
        let a = tunnel_rule_fingerprint(&tunneled, "https://app.workers.dev", &digest);
        let b = tunnel_rule_fingerprint(&tunneled, "https://app.workers.dev", "");
        assert_ne!(a, b);
    }

    #[test]
    fn preview_url_prefers_custom_domain_then_workers_dev() {
        let mut rule = rule("app.devbase.localhost", 3000);
        rule.tunnel = true;
        assert_eq!(
            preview_tunnel_url(&rule, "example"),
            "https://devbase-app.example.workers.dev"
        );
        rule.tunnel_domain = "https://app.example.com/".into();
        assert_eq!(
            preview_tunnel_url(&rule, "example"),
            "https://app.example.com"
        );
        rule.tunnel_domain.clear();
        rule.tunnel_url = "https://saved.example.com".into();
        assert_eq!(preview_tunnel_url(&rule, ""), "https://saved.example.com");
    }
}
