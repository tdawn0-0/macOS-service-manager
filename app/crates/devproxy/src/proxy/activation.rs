//! Explicit HTTPS and tunnel observations for one rule.
//!
//! Ported from `proxy_activation.go`. Results are never cached: a result is
//! current only while the revision and fingerprint still match and it is at
//! most 30 seconds old. Failures are fixed strings, never raw errors.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use devcore::state::{StateError, Store};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::secrets::SecretStore;

use super::rules::{
    proxy_rule_fingerprint, resolve_tunnel_public_url, rule_auth_digest, tunnel_rule_fingerprint,
};

/// A single, explicit observation; it contains no response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyHttpResult {
    pub domain: String,
    pub fingerprint: String,
    pub revision: u64,
    pub status_code: u16,
    #[serde(with = "duration_millis")]
    pub duration: Duration,
    pub checked_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

mod duration_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(value.as_millis() as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_millis(u64::deserialize(deserializer)?))
    }
}

impl ProxyHttpResult {
    /// A result is current only with the same generation and a 30 s age.
    pub fn is_current(&self, now: DateTime<Utc>, revision: u64, fingerprint: &str) -> bool {
        self.revision == revision
            && self.fingerprint == fingerprint
            && (now - self.checked_at).num_seconds() >= 0
            && (now - self.checked_at) <= chrono::TimeDelta::seconds(30)
    }
}

const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const TUNNEL_CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const BODY_LIMIT: usize = 64 << 10;

/// Build the current fingerprint and revision for a proxied rule.
pub fn proxy_fingerprint(store: &Store, domain: &str) -> Result<(String, u64), StateError> {
    let state = store.read()?;
    let domain = domain.trim().to_lowercase();
    for rule in &state.proxy_rules {
        if rule.domain == domain {
            return Ok((
                proxy_rule_fingerprint(state.proxy_listener_port, rule),
                state.proxy_rules_revision,
            ));
        }
    }
    Err(StateError::invalid("proxy rule is missing"))
}

/// Build the current fingerprint and revision for a tunneled rule.
pub fn tunnel_fingerprint(store: &Store, domain: &str) -> Result<(String, u64), StateError> {
    let state = store.read()?;
    let secrets = SecretStore::new(store.root());
    let domain = domain.trim().to_lowercase();
    for rule in &state.proxy_rules {
        if rule.domain == domain {
            if !rule.tunnel {
                return Err(StateError::invalid("tunnel is not enabled for this rule"));
            }
            let public_url = resolve_tunnel_public_url(&state, rule);
            let auth_digest = if rule.tunnel_auth {
                secrets
                    .tunnel_access_token(&rule.resolved_tunnel_worker())
                    .map(|token| rule_auth_digest(&token))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            return Ok((
                tunnel_rule_fingerprint(rule, &public_url, &auth_digest),
                state.proxy_rules_revision,
            ));
        }
    }
    Err(StateError::invalid("proxy rule is missing"))
}

/// One bounded `GET /` through the local HTTPS listener.
pub async fn check_proxy_rule_http(
    store: &Store,
    domain: &str,
    revision: u64,
    fingerprint: &str,
) -> Result<ProxyHttpResult, StateError> {
    let mut result = ProxyHttpResult {
        domain: domain.trim().to_lowercase(),
        fingerprint: fingerprint.to_string(),
        revision,
        status_code: 0,
        duration: Duration::ZERO,
        checked_at: Utc::now(),
        failure: None,
    };
    let state = store.read()?;
    let Some(rule) = state
        .proxy_rules
        .iter()
        .find(|rule| rule.domain == result.domain)
    else {
        result.failure = Some("Stale".into());
        return Err(StateError::invalid("proxy rule changed; check again"));
    };
    if state.proxy_rules_revision != revision
        || proxy_rule_fingerprint(state.proxy_listener_port, rule) != fingerprint
    {
        result.failure = Some("Stale".into());
        return Err(StateError::invalid("proxy rule changed; check again"));
    }

    crate::ensure_crypto_provider();
    let port = state.proxy_listener_port;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port as u16));
    let client = match reqwest::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .http1_only()
        .tls_built_in_root_certs(true)
        .resolve(&result.domain, address)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            result.failure = Some(classify_error(&error));
            result.checked_at = Utc::now();
            return Ok(result);
        }
    };
    let started = Instant::now();
    let response = client
        .get(format!("https://{}/", result.domain))
        .send()
        .await;
    result.duration = started.elapsed();
    result.checked_at = Utc::now();
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            result.failure = Some(classify_error(&error));
            return Ok(result);
        }
    };
    result.status_code = response.status().as_u16();
    // Drain a bounded amount so the listener sees a complete request.
    if let Ok(bytes) = response.bytes().await {
        let _ = &bytes[..bytes.len().min(BODY_LIMIT)];
    }

    let latest = store.read()?;
    if latest.proxy_rules_revision != revision {
        result.failure = Some("Stale".into());
        result.status_code = 0;
        return Ok(result);
    }
    let same = latest.proxy_rules.iter().any(|next| {
        next.domain == result.domain
            && proxy_rule_fingerprint(latest.proxy_listener_port, next) == fingerprint
    });
    if !same {
        result.failure = Some("Stale".into());
        result.status_code = 0;
    }
    Ok(result)
}

/// One bounded `GET /` to a rule's public tunnel URL.
pub async fn check_tunnel_rule_http(
    store: &Store,
    domain: &str,
    revision: u64,
    fingerprint: &str,
) -> Result<ProxyHttpResult, StateError> {
    let mut result = ProxyHttpResult {
        domain: domain.trim().to_lowercase(),
        fingerprint: fingerprint.to_string(),
        revision,
        status_code: 0,
        duration: Duration::ZERO,
        checked_at: Utc::now(),
        failure: None,
    };
    let state = store.read()?;
    let secrets = SecretStore::new(store.root());
    let Some(rule) = state
        .proxy_rules
        .iter()
        .find(|rule| rule.domain == result.domain)
    else {
        result.failure = Some("Stale".into());
        return Err(StateError::invalid("tunnel rule changed; check again"));
    };
    if !rule.tunnel || state.proxy_rules_revision != revision {
        result.failure = Some("Stale".into());
        return Err(StateError::invalid("tunnel rule changed; check again"));
    }
    let public_url = resolve_tunnel_public_url(&state, rule);
    let mut access_token = String::new();
    if rule.tunnel_auth
        && let Ok(token) = secrets.tunnel_access_token(&rule.resolved_tunnel_worker())
        && !token.is_empty()
    {
        access_token = token;
    }
    let auth_digest = if access_token.is_empty() {
        String::new()
    } else {
        rule_auth_digest(&access_token)
    };
    if tunnel_rule_fingerprint(rule, &public_url, &auth_digest) != fingerprint {
        result.failure = Some("Stale".into());
        return Err(StateError::invalid("tunnel rule changed; check again"));
    }
    if public_url.is_empty() {
        result.checked_at = Utc::now();
        result.failure = Some("Not deployed".into());
        return Ok(result);
    }
    let Ok(mut url) = Url::parse(&public_url) else {
        result.checked_at = Utc::now();
        result.failure = Some("Unavailable".into());
        return Ok(result);
    };
    if url.host_str().is_none() {
        result.checked_at = Utc::now();
        result.failure = Some("Unavailable".into());
        return Ok(result);
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);

    crate::ensure_crypto_provider();
    let client = match reqwest::Client::builder()
        .timeout(TUNNEL_CHECK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .http1_only()
        .tls_built_in_root_certs(true)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            result.failure = Some(classify_error(&error));
            result.checked_at = Utc::now();
            return Ok(result);
        }
    };
    let mut request = client.get(url);
    if !access_token.is_empty() {
        request = request.bearer_auth(&access_token);
    }
    let started = Instant::now();
    let response = request.send().await;
    result.duration = started.elapsed();
    result.checked_at = Utc::now();
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            result.failure = Some(classify_error(&error));
            return Ok(result);
        }
    };
    result.status_code = response.status().as_u16();
    if let Ok(bytes) = response.bytes().await {
        let _ = &bytes[..bytes.len().min(BODY_LIMIT)];
    }

    let latest = store.read()?;
    if latest.proxy_rules_revision != revision {
        result.failure = Some("Stale".into());
        result.status_code = 0;
        return Ok(result);
    }
    let secrets = SecretStore::new(store.root());
    let same = latest.proxy_rules.iter().any(|next| {
        if next.domain != result.domain || !next.tunnel {
            return false;
        }
        let next_url = resolve_tunnel_public_url(&latest, next);
        let next_digest = if next.tunnel_auth {
            secrets
                .tunnel_access_token(&next.resolved_tunnel_worker())
                .map(|token| rule_auth_digest(&token))
                .unwrap_or_default()
        } else {
            String::new()
        };
        tunnel_rule_fingerprint(next, &next_url, &next_digest) == fingerprint
    });
    if !same {
        result.failure = Some("Stale".into());
        result.status_code = 0;
    }
    Ok(result)
}

fn classify_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "Timeout".into();
    }
    if has_tls_error(error) {
        return "TLS".into();
    }
    "Unavailable".into()
}

fn has_tls_error(error: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        let text = current.to_string().to_lowercase();
        if text.contains("certificate")
            || text.contains("unknownissuer")
            || text.contains("notvalidforname")
            || text.contains("invalid peer certificate")
        {
            return true;
        }
        source = current.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use devcore::model::ProxyRule;
    use tempfile::TempDir;

    use super::*;

    fn store_with(domain: &str, port: i64) -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let mut state = store.read().unwrap();
        state.proxy_listener_port = 8443;
        state.proxy_rules = vec![ProxyRule {
            domain: domain.into(),
            local_port: port,
            ..ProxyRule::default()
        }];
        state.proxy_rules_revision = 7;
        store.write_state(&state).unwrap();
        (dir, store)
    }

    #[test]
    fn fingerprints_carry_generation_and_revision() {
        let (_dir, store) = store_with("app.devbase.localhost", 3000);
        let (fingerprint, revision) = proxy_fingerprint(&store, "APP.devbase.localhost").unwrap();
        assert_eq!(revision, 7);
        assert_eq!(fingerprint.len(), 64);
        assert_eq!(
            proxy_fingerprint(&store, "app.devbase.localhost").unwrap(),
            (fingerprint.clone(), 7)
        );
        assert!(proxy_fingerprint(&store, "missing.devbase.localhost").is_err());

        let mut state = store.read().unwrap();
        state.proxy_rules[0].local_port = 3001;
        store.write_state(&state).unwrap();
        assert_ne!(
            proxy_fingerprint(&store, "app.devbase.localhost")
                .unwrap()
                .0,
            fingerprint
        );
    }

    #[test]
    fn results_expire_after_thirty_seconds() {
        let now = Utc::now();
        let result = ProxyHttpResult {
            domain: "app.devbase.localhost".into(),
            fingerprint: "abc".into(),
            revision: 1,
            status_code: 200,
            duration: Duration::from_millis(4),
            checked_at: now - chrono::TimeDelta::seconds(29),
            failure: None,
        };
        assert!(result.is_current(now, 1, "abc"));
        assert!(!result.is_current(now, 2, "abc"));
        assert!(!result.is_current(now, 1, "other"));
        let old = ProxyHttpResult {
            checked_at: now - chrono::TimeDelta::seconds(31),
            ..result
        };
        assert!(!old.is_current(now, 1, "abc"));
    }

    #[test]
    fn tunnel_fingerprints_require_tunnel_mode() {
        let (_dir, store) = store_with("app.devbase.localhost", 3000);
        assert!(tunnel_fingerprint(&store, "app.devbase.localhost").is_err());
        let mut state = store.read().unwrap();
        state.proxy_rules[0].tunnel = true;
        state.proxy_rules[0].tunnel_url = "https://app.workers.dev".into();
        store.write_state(&state).unwrap();
        let (fingerprint, revision) = tunnel_fingerprint(&store, "app.devbase.localhost").unwrap();
        assert_eq!(revision, 7);
        assert_eq!(fingerprint.len(), 64);
    }

    #[test]
    fn stale_checks_fail_before_any_network_call() {
        let (_dir, store) = store_with("app.devbase.localhost", 3000);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(check_proxy_rule_http(
                &store,
                "app.devbase.localhost",
                6,
                "wrong",
            ))
            .unwrap_err();
        assert!(error.to_string().contains("proxy rule changed"));
        let error = runtime
            .block_on(check_tunnel_rule_http(
                &store,
                "app.devbase.localhost",
                6,
                "wrong",
            ))
            .unwrap_err();
        assert!(error.to_string().contains("tunnel rule changed"));
    }
}
