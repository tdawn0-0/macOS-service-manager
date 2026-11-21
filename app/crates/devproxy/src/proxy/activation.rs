//! Explicit HTTPS and tunnel observations for one rule.
//!
//! Ported from `proxy_activation.go`. Results are never cached: a result is
//! current only while the revision and fingerprint still match and it is at
//! most 30 seconds old. Failures are fixed strings, never raw errors.

use std::io;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use devcore::state::{StateError, Store};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::secrets::SecretStore;
use crate::tunnel::config::read_status as read_tunnel_status;

use super::rules::{
    proxy_rule_fingerprint, resolve_tunnel_public_url, rule_auth_digest, tunnel_rule_fingerprint,
};

/// Actionable diagnosis category for a failed local HTTPS or tunnel probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeDiagnosis {
    UpstreamNotListening,
    CaUntrusted,
    TunnelDisconnected,
    TunnelConflict,
    ProxyStopped,
    Timeout,
    NotDeployed,
    Stale,
    Unavailable,
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnosis: Option<ProbeDiagnosis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
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

    /// Return the human-readable actionable hint for a failed probe, falling
    /// back to the diagnosis category or failure label when older fixtures only
    /// populate `failure`.
    pub fn diagnostic_hint(
        &self,
        local_port: Option<i64>,
        live_tunnel_status: Option<&str>,
    ) -> Option<String> {
        if let Some(hint) = self.hint.as_deref()
            && !hint.trim().is_empty()
        {
            return Some(hint.to_string());
        }
        if let Some(diagnosis) = self.diagnosis {
            return Some(default_hint_for_diagnosis(diagnosis, local_port));
        }
        let failure = self.failure.as_deref()?;
        default_hint_for_failure(failure, local_port, live_tunnel_status)
    }
}

/// Default actionable hint text for a structured [`ProbeDiagnosis`].
pub fn default_hint_for_diagnosis(diagnosis: ProbeDiagnosis, local_port: Option<i64>) -> String {
    match diagnosis {
        ProbeDiagnosis::UpstreamNotListening => match local_port {
            Some(port) if port > 0 => format!(
                "Upstream port :{port} is not listening. Start your app server in a terminal first."
            ),
            _ => "Upstream port is not listening. Start your app server in a terminal first."
                .to_string(),
        },
        ProbeDiagnosis::CaUntrusted => {
            "Local CA is not trusted yet. Trust the CA in Certificate Authority settings."
                .to_string()
        }
        ProbeDiagnosis::TunnelDisconnected => {
            "Cloudflare tunnel is not connected for this rule.".to_string()
        }
        ProbeDiagnosis::TunnelConflict => {
            "Cloudflare tunnel is not connected for this rule (another client is connected to this Worker)."
                .to_string()
        }
        ProbeDiagnosis::ProxyStopped => {
            "HTTPS proxy listener is not running. Start or apply the proxy first.".to_string()
        }
        ProbeDiagnosis::Timeout => {
            "The probe timed out waiting for a response. Check that the proxy and upstream server are responsive."
                .to_string()
        }
        ProbeDiagnosis::NotDeployed => {
            "Deploy Cloudflare Workers before checking the tunnel response.".to_string()
        }
        ProbeDiagnosis::Stale => {
            "The proxy rule changed while checking. Run the probe again.".to_string()
        }
        ProbeDiagnosis::Unavailable => {
            "The endpoint could not be reached. Check that the service is running.".to_string()
        }
    }
}

fn default_hint_for_failure(
    failure: &str,
    local_port: Option<i64>,
    live_tunnel_status: Option<&str>,
) -> Option<String> {
    let lower = failure.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }
    if lower == "tls" || lower.contains("certificate") || lower.contains("ca ") {
        return Some(default_hint_for_diagnosis(
            ProbeDiagnosis::CaUntrusted,
            local_port,
        ));
    }
    if lower.contains("connection refused") || lower.contains("upstream") {
        return Some(default_hint_for_diagnosis(
            ProbeDiagnosis::UpstreamNotListening,
            local_port,
        ));
    }
    if lower.contains("conflict") {
        return Some(default_hint_for_diagnosis(
            ProbeDiagnosis::TunnelConflict,
            local_port,
        ));
    }
    if lower.contains("disconnected") || lower.contains("offline") {
        return Some(default_hint_for_diagnosis(
            ProbeDiagnosis::TunnelDisconnected,
            local_port,
        ));
    }
    if lower.contains("proxy stopped") {
        return Some(default_hint_for_diagnosis(
            ProbeDiagnosis::ProxyStopped,
            local_port,
        ));
    }
    if lower.contains("timeout") {
        return Some(default_hint_for_diagnosis(
            ProbeDiagnosis::Timeout,
            local_port,
        ));
    }
    if lower.contains("not deployed") {
        return Some(default_hint_for_diagnosis(
            ProbeDiagnosis::NotDeployed,
            local_port,
        ));
    }
    if lower == "stale" {
        return Some(default_hint_for_diagnosis(
            ProbeDiagnosis::Stale,
            local_port,
        ));
    }
    if let Some(status) = live_tunnel_status {
        if status.eq_ignore_ascii_case("Conflict") {
            return Some(default_hint_for_diagnosis(
                ProbeDiagnosis::TunnelConflict,
                local_port,
            ));
        }
        if !status.eq_ignore_ascii_case("Connected") {
            return Some(default_hint_for_diagnosis(
                ProbeDiagnosis::TunnelDisconnected,
                local_port,
            ));
        }
    }
    None
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
        diagnosis: None,
        hint: None,
    };
    let state = store.read()?;
    let Some(rule) = state
        .proxy_rules
        .iter()
        .find(|rule| rule.domain == result.domain)
    else {
        apply_failure(
            &mut result,
            "Stale",
            ProbeDiagnosis::Stale,
            default_hint_for_diagnosis(ProbeDiagnosis::Stale, None),
        );
        return Err(StateError::invalid("proxy rule changed; check again"));
    };
    if state.proxy_rules_revision != revision
        || proxy_rule_fingerprint(state.proxy_listener_port, rule) != fingerprint
    {
        apply_failure(
            &mut result,
            "Stale",
            ProbeDiagnosis::Stale,
            default_hint_for_diagnosis(ProbeDiagnosis::Stale, Some(rule.local_port)),
        );
        return Err(StateError::invalid("proxy rule changed; check again"));
    }

    crate::ensure_crypto_provider();
    let listener_port = state.proxy_listener_port;
    let local_port = rule.local_port;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], listener_port as u16));
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
            let (failure, diagnosis, hint) =
                classify_reqwest_error(&error, false, listener_port, local_port, None);
            apply_failure(&mut result, failure, diagnosis, hint);
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
            let (failure, diagnosis, hint) =
                classify_reqwest_error(&error, false, listener_port, local_port, None);
            apply_failure(&mut result, failure, diagnosis, hint);
            return Ok(result);
        }
    };
    result.status_code = response.status().as_u16();
    // Drain a bounded amount so the listener sees a complete request and inspect
    // Devbase's fixed error pages without retaining any response body.
    let body_snippet = match response.bytes().await {
        Ok(bytes) => {
            let slice = &bytes[..bytes.len().min(BODY_LIMIT)];
            String::from_utf8_lossy(slice).into_owned()
        }
        Err(_) => String::new(),
    };
    if let Some((failure, diagnosis, hint)) =
        diagnose_proxy_response(result.status_code, &body_snippet, local_port)
    {
        apply_failure(&mut result, failure, diagnosis, hint);
    }

    let latest = store.read()?;
    if latest.proxy_rules_revision != revision {
        apply_failure(
            &mut result,
            "Stale",
            ProbeDiagnosis::Stale,
            default_hint_for_diagnosis(ProbeDiagnosis::Stale, Some(local_port)),
        );
        result.status_code = 0;
        return Ok(result);
    }
    let same = latest.proxy_rules.iter().any(|next| {
        next.domain == result.domain
            && proxy_rule_fingerprint(latest.proxy_listener_port, next) == fingerprint
    });
    if !same {
        apply_failure(
            &mut result,
            "Stale",
            ProbeDiagnosis::Stale,
            default_hint_for_diagnosis(ProbeDiagnosis::Stale, Some(local_port)),
        );
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
        diagnosis: None,
        hint: None,
    };
    let state = store.read()?;
    let secrets = SecretStore::new(store.root());
    let Some(rule) = state
        .proxy_rules
        .iter()
        .find(|rule| rule.domain == result.domain)
    else {
        apply_failure(
            &mut result,
            "Stale",
            ProbeDiagnosis::Stale,
            default_hint_for_diagnosis(ProbeDiagnosis::Stale, None),
        );
        return Err(StateError::invalid("tunnel rule changed; check again"));
    };
    if !rule.tunnel || state.proxy_rules_revision != revision {
        apply_failure(
            &mut result,
            "Stale",
            ProbeDiagnosis::Stale,
            default_hint_for_diagnosis(ProbeDiagnosis::Stale, Some(rule.local_port)),
        );
        return Err(StateError::invalid("tunnel rule changed; check again"));
    }
    let worker_name = rule.resolved_tunnel_worker();
    let local_port = rule.local_port;
    let listener_port = state.proxy_listener_port;
    let worker_state = read_tunnel_status(store.root())
        .and_then(|status| status.worker_states.get(&worker_name).cloned());
    let public_url = resolve_tunnel_public_url(&state, rule);
    let mut access_token = String::new();
    if rule.tunnel_auth
        && let Ok(token) = secrets.tunnel_access_token(&worker_name)
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
        apply_failure(
            &mut result,
            "Stale",
            ProbeDiagnosis::Stale,
            default_hint_for_diagnosis(ProbeDiagnosis::Stale, Some(local_port)),
        );
        return Err(StateError::invalid("tunnel rule changed; check again"));
    }
    if public_url.is_empty() {
        result.checked_at = Utc::now();
        apply_failure(
            &mut result,
            "Not deployed",
            ProbeDiagnosis::NotDeployed,
            default_hint_for_diagnosis(ProbeDiagnosis::NotDeployed, Some(local_port)),
        );
        return Ok(result);
    }
    let Ok(mut url) = Url::parse(&public_url) else {
        result.checked_at = Utc::now();
        apply_failure(
            &mut result,
            "Unavailable",
            ProbeDiagnosis::Unavailable,
            "Tunnel public URL is invalid. Re-deploy Cloudflare Workers.".to_string(),
        );
        return Ok(result);
    };
    if url.host_str().is_none() {
        result.checked_at = Utc::now();
        apply_failure(
            &mut result,
            "Unavailable",
            ProbeDiagnosis::Unavailable,
            "Tunnel public URL is missing a host. Re-deploy Cloudflare Workers.".to_string(),
        );
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
            let (failure, diagnosis, hint) = classify_reqwest_error(
                &error,
                true,
                listener_port,
                local_port,
                worker_state.as_deref(),
            );
            apply_failure(&mut result, failure, diagnosis, hint);
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
            let (failure, diagnosis, hint) = classify_reqwest_error(
                &error,
                true,
                listener_port,
                local_port,
                worker_state.as_deref(),
            );
            apply_failure(&mut result, failure, diagnosis, hint);
            return Ok(result);
        }
    };
    result.status_code = response.status().as_u16();
    let body_snippet = match response.bytes().await {
        Ok(bytes) => {
            let slice = &bytes[..bytes.len().min(BODY_LIMIT)];
            String::from_utf8_lossy(slice).into_owned()
        }
        Err(_) => String::new(),
    };
    if let Some((failure, diagnosis, hint)) = diagnose_tunnel_response(
        result.status_code,
        &body_snippet,
        local_port,
        worker_state.as_deref(),
    ) {
        apply_failure(&mut result, failure, diagnosis, hint);
    }

    let latest = store.read()?;
    if latest.proxy_rules_revision != revision {
        apply_failure(
            &mut result,
            "Stale",
            ProbeDiagnosis::Stale,
            default_hint_for_diagnosis(ProbeDiagnosis::Stale, Some(local_port)),
        );
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
        apply_failure(
            &mut result,
            "Stale",
            ProbeDiagnosis::Stale,
            default_hint_for_diagnosis(ProbeDiagnosis::Stale, Some(local_port)),
        );
        result.status_code = 0;
    }
    Ok(result)
}

fn apply_failure(
    result: &mut ProxyHttpResult,
    failure: impl Into<String>,
    diagnosis: ProbeDiagnosis,
    hint: String,
) {
    result.failure = Some(failure.into());
    result.diagnosis = Some(diagnosis);
    result.hint = Some(hint);
}

/// Classify a Devbase local proxy HTTP error response (502/504 failure pages).
pub fn diagnose_proxy_response(
    status_code: u16,
    body: &str,
    local_port: i64,
) -> Option<(String, ProbeDiagnosis, String)> {
    if status_code == 502 && body.contains("Local service is unavailable") {
        return Some((
            "Connection refused".into(),
            ProbeDiagnosis::UpstreamNotListening,
            default_hint_for_diagnosis(ProbeDiagnosis::UpstreamNotListening, Some(local_port)),
        ));
    }
    if status_code == 502 && body.contains("Upstream certificate is not trusted") {
        return Some((
            "TLS".into(),
            ProbeDiagnosis::CaUntrusted,
            "Upstream HTTPS certificate is not trusted. Check the upstream server's certificate."
                .into(),
        ));
    }
    if status_code == 504 && body.contains("Local service timed out") {
        return Some((
            "Timeout".into(),
            ProbeDiagnosis::Timeout,
            format!(
                "Upstream port :{local_port} did not respond before the proxy timeout. Check your app server logs."
            ),
        ));
    }
    None
}

/// Classify a Cloudflare tunnel HTTP error response (502/504 worker or agent failure pages).
pub fn diagnose_tunnel_response(
    status_code: u16,
    body: &str,
    local_port: i64,
    worker_state: Option<&str>,
) -> Option<(String, ProbeDiagnosis, String)> {
    if worker_state == Some("Conflict") && status_code >= 500 {
        return Some((
            "Tunnel conflict".into(),
            ProbeDiagnosis::TunnelConflict,
            default_hint_for_diagnosis(ProbeDiagnosis::TunnelConflict, Some(local_port)),
        ));
    }
    if status_code == 502
        && (body.contains("Devbase tunnel offline") || body.contains("Tunnel client disconnected"))
    {
        return Some((
            "Tunnel disconnected".into(),
            ProbeDiagnosis::TunnelDisconnected,
            default_hint_for_diagnosis(ProbeDiagnosis::TunnelDisconnected, Some(local_port)),
        ));
    }
    if status_code == 502 && body.contains("Local service is unavailable") {
        return Some((
            "Connection refused".into(),
            ProbeDiagnosis::UpstreamNotListening,
            default_hint_for_diagnosis(ProbeDiagnosis::UpstreamNotListening, Some(local_port)),
        ));
    }
    if status_code == 504
        && (body.contains("Local service timed out")
            || body.contains("Gateway timeout waiting for local upstream"))
    {
        return Some((
            "Timeout".into(),
            ProbeDiagnosis::Timeout,
            format!(
                "Tunnel timed out waiting for upstream port :{local_port}. Check your app server logs."
            ),
        ));
    }
    None
}

/// Classify a transport/TLS/connection error into a fixed failure label, structured
/// [`ProbeDiagnosis`], and actionable human-readable hint.
pub fn diagnose_transport_error(
    error_text: &str,
    is_timeout: bool,
    is_connection_refused: bool,
    is_tunnel: bool,
    listener_port: i64,
    local_port: i64,
    worker_state: Option<&str>,
) -> (String, ProbeDiagnosis, String) {
    if is_timeout {
        let hint = if is_tunnel {
            "Cloudflare tunnel probe timed out. Check that the tunnel is running and your app server is responsive."
                .to_string()
        } else {
            format!(
                "HTTPS proxy listener on :{listener_port} timed out. Check that the proxy and upstream port :{local_port} are responsive."
            )
        };
        return ("Timeout".into(), ProbeDiagnosis::Timeout, hint);
    }
    if has_tls_text(error_text) {
        let hint = if is_tunnel {
            "TLS handshake failed while reaching the Cloudflare tunnel URL.".to_string()
        } else {
            default_hint_for_diagnosis(ProbeDiagnosis::CaUntrusted, Some(local_port))
        };
        return ("TLS".into(), ProbeDiagnosis::CaUntrusted, hint);
    }
    if is_connection_refused {
        if is_tunnel {
            if worker_state == Some("Conflict") {
                return (
                    "Tunnel conflict".into(),
                    ProbeDiagnosis::TunnelConflict,
                    default_hint_for_diagnosis(ProbeDiagnosis::TunnelConflict, Some(local_port)),
                );
            }
            return (
                "Tunnel disconnected".into(),
                ProbeDiagnosis::TunnelDisconnected,
                default_hint_for_diagnosis(ProbeDiagnosis::TunnelDisconnected, Some(local_port)),
            );
        }
        return (
            "Proxy stopped".into(),
            ProbeDiagnosis::ProxyStopped,
            format!(
                "HTTPS proxy listener on :{listener_port} is not running. Start or apply the proxy first."
            ),
        );
    }
    if is_tunnel {
        if worker_state == Some("Conflict") {
            return (
                "Tunnel conflict".into(),
                ProbeDiagnosis::TunnelConflict,
                default_hint_for_diagnosis(ProbeDiagnosis::TunnelConflict, Some(local_port)),
            );
        }
        if worker_state.is_some_and(|status| !status.eq_ignore_ascii_case("Connected")) {
            return (
                "Tunnel disconnected".into(),
                ProbeDiagnosis::TunnelDisconnected,
                default_hint_for_diagnosis(ProbeDiagnosis::TunnelDisconnected, Some(local_port)),
            );
        }
        return (
            "Unavailable".into(),
            ProbeDiagnosis::Unavailable,
            "Could not reach the Cloudflare tunnel URL. Check your network and tunnel status."
                .to_string(),
        );
    }
    (
        "Unavailable".into(),
        ProbeDiagnosis::Unavailable,
        format!(
            "Could not reach the local HTTPS proxy on :{listener_port}. Start or apply the proxy first."
        ),
    )
}

fn classify_reqwest_error(
    error: &reqwest::Error,
    is_tunnel: bool,
    listener_port: i64,
    local_port: i64,
    worker_state: Option<&str>,
) -> (String, ProbeDiagnosis, String) {
    let combined = collect_error_chain(error);
    let refused =
        is_connection_refused(error) || combined.to_lowercase().contains("connection refused");
    diagnose_transport_error(
        &combined,
        error.is_timeout(),
        refused,
        is_tunnel,
        listener_port,
        local_port,
        worker_state,
    )
}

fn collect_error_chain(error: &reqwest::Error) -> String {
    let mut out = String::new();
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&current.to_string());
        source = current.source();
    }
    out
}

fn has_tls_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("certificate")
        || lower.contains("unknownissuer")
        || lower.contains("notvalidforname")
        || lower.contains("invalid peer certificate")
        || lower.contains("tls handshake")
}

fn is_connection_refused(error: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if let Some(io_error) = current.downcast_ref::<io::Error>()
            && io_error.kind() == io::ErrorKind::ConnectionRefused
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
            diagnosis: None,
            hint: None,
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
    fn serde_preserves_backwards_compatibility_with_legacy_payloads() {
        let legacy_json = r#"{
            "domain": "app.devbase.localhost",
            "fingerprint": "abc",
            "revision": 7,
            "statusCode": 200,
            "duration": 15,
            "checkedAt": "2026-01-01T00:00:00Z"
        }"#;
        let parsed: ProxyHttpResult = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed.status_code, 200);
        assert_eq!(parsed.failure, None);
        assert_eq!(parsed.diagnosis, None);
        assert_eq!(parsed.hint, None);

        let reencoded = serde_json::to_string(&parsed).unwrap();
        assert!(!reencoded.contains("diagnosis"));
        assert!(!reencoded.contains("hint"));
    }

    #[test]
    fn probe_diagnostics_categorize_upstream_ca_tunnel_stopped_and_timeout() {
        // Upstream port not listening (502 failure page from local proxy or tunnel).
        let (failure, diag, hint) =
            diagnose_proxy_response(502, "<h1>Local service is unavailable</h1>", 3000).unwrap();
        assert_eq!(failure, "Connection refused");
        assert_eq!(diag, ProbeDiagnosis::UpstreamNotListening);
        assert_eq!(
            hint,
            "Upstream port :3000 is not listening. Start your app server in a terminal first."
        );

        // Local CA untrusted / TLS handshake error.
        let (failure, diag, hint) = diagnose_transport_error(
            "invalid peer certificate: UnknownIssuer",
            false,
            false,
            false,
            443,
            3000,
            None,
        );
        assert_eq!(failure, "TLS");
        assert_eq!(diag, ProbeDiagnosis::CaUntrusted);
        assert_eq!(
            hint,
            "Local CA is not trusted yet. Trust the CA in Certificate Authority settings."
        );

        // Tunnel offline / disconnected.
        let (failure, diag, hint) =
            diagnose_tunnel_response(502, "Devbase tunnel offline", 3000, Some("Connecting"))
                .unwrap();
        assert_eq!(failure, "Tunnel disconnected");
        assert_eq!(diag, ProbeDiagnosis::TunnelDisconnected);
        assert_eq!(hint, "Cloudflare tunnel is not connected for this rule.");

        // Tunnel conflict.
        let (failure, diag, hint) =
            diagnose_tunnel_response(502, "Devbase tunnel offline", 3000, Some("Conflict"))
                .unwrap();
        assert_eq!(failure, "Tunnel conflict");
        assert_eq!(diag, ProbeDiagnosis::TunnelConflict);
        assert!(hint.contains("Cloudflare tunnel is not connected for this rule"));

        // Local proxy listener stopped.
        let (failure, diag, hint) = diagnose_transport_error(
            "tcp connect error: Connection refused",
            false,
            true,
            false,
            8443,
            3000,
            None,
        );
        assert_eq!(failure, "Proxy stopped");
        assert_eq!(diag, ProbeDiagnosis::ProxyStopped);
        assert!(hint.contains(":8443"));

        // Timeout.
        let (failure, diag, hint) =
            diagnose_transport_error("operation timed out", true, false, false, 443, 3000, None);
        assert_eq!(failure, "Timeout");
        assert_eq!(diag, ProbeDiagnosis::Timeout);
        assert!(hint.contains("timed out"));

        // User app's own 500/404 is not misclassified as a Devbase infrastructure error.
        assert!(diagnose_proxy_response(500, "Internal Server Error", 3000).is_none());
        assert!(diagnose_proxy_response(404, "Not Found", 3000).is_none());
    }

    #[test]
    fn diagnostic_hint_falls_back_from_legacy_failure_strings() {
        let tls = ProxyHttpResult {
            domain: "app.devbase.localhost".into(),
            fingerprint: "fp".into(),
            revision: 1,
            status_code: 0,
            duration: Duration::from_millis(5),
            checked_at: Utc::now(),
            failure: Some("TLS".into()),
            diagnosis: None,
            hint: None,
        };
        assert_eq!(
            tls.diagnostic_hint(Some(3000), None).as_deref(),
            Some("Local CA is not trusted yet. Trust the CA in Certificate Authority settings.")
        );

        let refused = ProxyHttpResult {
            failure: Some("Connection refused".into()),
            ..tls.clone()
        };
        assert_eq!(
            refused.diagnostic_hint(Some(5173), None).as_deref(),
            Some(
                "Upstream port :5173 is not listening. Start your app server in a terminal first."
            )
        );

        let tunnel_disconnected = ProxyHttpResult {
            failure: Some("Unavailable".into()),
            ..tls
        };
        assert_eq!(
            tunnel_disconnected
                .diagnostic_hint(Some(3000), Some("Connecting"))
                .as_deref(),
            Some("Cloudflare tunnel is not connected for this rule.")
        );
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
