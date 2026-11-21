//! The HTTPS proxy workspace: the saved rule table and its editable draft,
//! the proxy and Cloudflare tunnel services, the local certificate authority
//! and the per-rule activation observations.
//!
//! Ported from `view_https_proxy.go`, `view_proxy_activation.go` and
//! `view_authority.go`. Every provider call does disk, process, network or
//! keychain work, so each one runs on a blocking thread and writes the store
//! when it answers. Render code reads the store; it never applies the proxy,
//! changes trust or checks a rule itself.
//!
//! The Cloudflare API token never enters this store: the screen picks a file
//! and the provider reads it straight into `proxy/cloudflare.key`. Nothing here
//! keeps or shows the token text.
//!
//! Tests install a provider of their own before the surface starts with
//! [`use_fake_provider`] and [`install_provider`]; production builds the
//! devproxy services on first use.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::Utc;
use devcore::{
    databases::{Databases, ListeningPort},
    engine::{Engine, ProcessRunnerAdapter},
    health::read_job_state,
    launchd::SystemRunner,
    model::{ProxyRule, State, local_domain, normalize_proxy_rules},
    state::Store,
};
use devproxy::{
    ca::{
        material::AuthorityInfo,
        service::{AuthorityService, AuthorityStatus, ProxyStopCheck},
        store::AuthorityStore,
        trust::SecurityTrust,
    },
    proxy::{
        activation::{
            ProxyHttpResult, check_proxy_rule_http, check_tunnel_rule_http, proxy_fingerprint,
            tunnel_fingerprint,
        },
        lifecycle::ProxyService,
        rules::preview_tunnel_url,
    },
    secrets::{SecretStore, validate_cloudflare_token},
    tunnel::{
        config::read_status as read_tunnel_status, lifecycle::TunnelJobs, service::TunnelService,
    },
};
use nsh_base::alert_dialog;
use nsh_core::prelude::*;

use crate::logs::tail;

/// The only domain suffix a rule may use.
pub use devproxy::proxy::rules::LOCAL_DOMAIN_SUFFIX;

/// `workers.dev (Auto)`, the tunnel mode that uses the account's subdomain.
pub const TUNNEL_MODE_WORKERS_DEV: &str = "workers.dev (Auto)";
/// `Custom domain`, the tunnel mode that maps a host in a Cloudflare zone.
pub const TUNNEL_MODE_CUSTOM_DOMAIN: &str = "Custom domain";

/// One editable rule. Ports stay text until Save, as in the Go draft, so the
/// field can hold what was typed and Save can reject it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleDraft {
    /// A stable row key for the reactive list; not part of the saved rule.
    pub id: u64,
    pub domain: String,
    pub local_port: String,
    pub upstream_host: String,
    pub tunnel: bool,
    pub tunnel_auth: bool,
    pub tunnel_worker: String,
    pub tunnel_mode: String,
    pub tunnel_domain: String,
    pub tunnel_url: String,
}

impl RuleDraft {
    /// A new rule exactly as the Go screen adds one.
    pub fn new() -> Self {
        RuleDraft {
            local_port: "3000".to_string(),
            tunnel_mode: TUNNEL_MODE_WORKERS_DEV.to_string(),
            ..RuleDraft::default()
        }
    }

    fn from_rule(id: u64, rule: &ProxyRule) -> Self {
        let mode = if rule.tunnel_domain.is_empty() {
            TUNNEL_MODE_WORKERS_DEV
        } else {
            TUNNEL_MODE_CUSTOM_DOMAIN
        };
        RuleDraft {
            id,
            domain: rule
                .domain
                .strip_suffix(LOCAL_DOMAIN_SUFFIX)
                .unwrap_or(&rule.domain)
                .to_string(),
            local_port: rule.local_port.to_string(),
            upstream_host: rule.upstream_host.clone(),
            tunnel: rule.tunnel,
            tunnel_auth: rule.tunnel_auth,
            tunnel_worker: rule.tunnel_worker.clone(),
            tunnel_mode: mode.to_string(),
            tunnel_domain: rule.tunnel_domain.clone(),
            tunnel_url: rule.tunnel_url.clone(),
        }
    }

    /// The custom domain the mode currently targets, empty for workers.dev.
    pub fn effective_tunnel_domain(&self) -> String {
        if self.tunnel_mode != TUNNEL_MODE_CUSTOM_DOMAIN {
            return String::new();
        }
        self.tunnel_domain.trim().to_lowercase()
    }

    /// The rule this draft saves, with ports left at zero when unparsable so
    /// the store's validator names them.
    pub fn to_rule(&self) -> ProxyRule {
        ProxyRule {
            domain: local_domain(&self.domain),
            local_port: self.local_port.trim().parse().unwrap_or(0),
            upstream_host: self.upstream_host.trim().to_string(),
            tunnel: self.tunnel,
            tunnel_auth: self.tunnel_auth,
            tunnel_worker: self.tunnel_worker.trim().to_lowercase(),
            tunnel_domain: self.effective_tunnel_domain(),
            tunnel_url: self.tunnel_url.trim().to_string(),
        }
    }
}

/// A deployed Worker's public address, keyed by script name.
#[derive(Debug, Clone, Default)]
pub struct Workers(BTreeMap<String, String>);

impl Workers {
    pub fn new(entries: BTreeMap<String, String>) -> Self {
        Workers(entries)
    }

    pub fn public_url(&self, worker: &str) -> Option<&str> {
        self.0.get(worker).map(String::as_str)
    }

    pub fn insert(&mut self, worker: impl Into<String>, url: impl Into<String>) {
        self.0.insert(worker.into(), url.into());
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// One read of the development home for these screens.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// The saved rule table, not the draft.
    pub rules: Vec<ProxyRule>,
    pub listener_port: i64,
    pub revision: u64,
    /// `Published`, `Pending` or `Unknown`.
    pub publication: String,
    /// `proxy`, `tunnel` and `tunnel:worker:<name>` observations.
    pub statuses: BTreeMap<String, String>,
    pub token_configured: bool,
    pub account_id: String,
    pub client_id: String,
    pub subdomain: String,
    pub zones: Vec<String>,
    /// Host names from the account's DNS records, for the domain suggestion.
    pub dns_names: Vec<String>,
    pub workers: Workers,
}

/// What `save_cloudflare` leaves behind.
#[derive(Debug, Clone, Default)]
pub struct SavedCloudflare {
    pub token_configured: bool,
    pub account_id: String,
}

/// One authority transition, named so a test records exactly what ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityAction {
    Prepare,
    TrustActive,
    RemoveActiveTrust,
    TrustPrepared {
        revision: String,
    },
    Activate {
        revision: String,
    },
    DiscardPrepared {
        revision: String,
    },
    Restore {
        id: String,
        fingerprint: String,
        revision: String,
    },
    TrustSaved {
        id: String,
        fingerprint: String,
    },
    RemoveSavedTrust {
        id: String,
        fingerprint: String,
    },
}

impl AuthorityAction {
    /// A short name for test records and progress lines.
    pub fn label(&self) -> String {
        match self {
            AuthorityAction::Prepare => "prepare".to_string(),
            AuthorityAction::TrustActive => "trust active".to_string(),
            AuthorityAction::RemoveActiveTrust => "remove active trust".to_string(),
            AuthorityAction::TrustPrepared { revision } => {
                format!("trust prepared {revision}")
            }
            AuthorityAction::Activate { revision } => format!("activate {revision}"),
            AuthorityAction::DiscardPrepared { revision } => {
                format!("discard prepared {revision}")
            }
            AuthorityAction::Restore {
                id,
                fingerprint,
                revision,
            } => format!("restore {id} {fingerprint} {revision}"),
            AuthorityAction::TrustSaved { id, fingerprint } => {
                format!("trust saved {id} {fingerprint}")
            }
            AuthorityAction::RemoveSavedTrust { id, fingerprint } => {
                format!("remove saved trust {id} {fingerprint}")
            }
        }
    }
}

/// The operations these screens need. The production implementation wraps
/// devproxy's services; a headless test implements it with fixtures and never
/// launches a process, touches the keychain or reaches Cloudflare.
pub trait Provider: Send + Sync {
    fn snapshot(&self) -> Result<Snapshot, String>;

    fn listening_ports(&self) -> Result<Vec<ListeningPort>, String>;

    fn save_rules(
        &self,
        expected_revision: u64,
        listener_port: i64,
        rules: Vec<ProxyRule>,
    ) -> Result<(), String>;

    fn apply_proxy(&self) -> Result<(), String>;
    fn stop_proxy(&self) -> Result<(), String>;
    fn restart_proxy(&self) -> Result<(), String>;

    fn proxy_result(&self, domain: &str) -> Result<ProxyHttpResult, String>;
    fn tunnel_result(&self, domain: &str) -> Result<ProxyHttpResult, String>;

    fn authority_status(&self) -> Result<AuthorityStatus, String>;
    fn authority_action(&self, action: AuthorityAction) -> Result<(), String>;

    /// Save the API token read from `token_path` (never the token text), the
    /// account ID and the client ID. An empty path keeps the saved token.
    fn save_cloudflare(
        &self,
        token_path: Option<&Path>,
        account_id: &str,
        client_id: &str,
    ) -> Result<SavedCloudflare, String>;
    fn refresh_cloudflare(&self) -> Result<(), String>;
    fn deploy_workers(&self) -> Result<(), String>;
    fn start_tunnel(&self) -> Result<(), String>;
    fn stop_tunnel(&self) -> Result<(), String>;
    fn restart_tunnel(&self) -> Result<(), String>;
    fn remove_cloudflare(&self, delete_workers: bool) -> Result<(), String>;

    fn ensure_tunnel_token(&self, domain: &str) -> Result<String, String>;
    fn rotate_tunnel_token(&self, domain: &str) -> Result<(), String>;

    fn logs(&self, job: &str) -> Result<String, String>;
}

// --- The production provider -------------------------------------------------

/// The development home, opened once.
struct Facade {
    store: Store,
}

impl Facade {
    fn new() -> Self {
        Facade {
            store: Store::new(devcore::state::default_root()),
        }
    }

    fn root(&self) -> &Path {
        self.store.root()
    }

    fn runner() -> Arc<dyn devcore::launchd::ProcessRunner> {
        Arc::new(SystemRunner::new())
    }

    fn authority(&self) -> AuthorityService {
        let engine = Engine::new(
            self.root(),
            Arc::new(ProcessRunnerAdapter::new(Self::runner())),
        );
        AuthorityService::new(
            AuthorityStore::new(self.root()),
            Arc::new(SecurityTrust::new(Self::runner())),
            Arc::new(StoppedProxy { engine }),
        )
    }

    fn proxy(&self) -> ProxyService {
        ProxyService::system(self.store.clone(), Arc::new(self.authority()))
    }

    fn jobs(&self) -> TunnelJobs {
        TunnelJobs::new(self.store.clone(), Self::runner())
    }

    fn tunnel(&self) -> TunnelService {
        self.jobs().service()
    }

    fn state(&self) -> Result<State, String> {
        self.store.read().map_err(|error| error.to_string())
    }

    /// The saved rule whose domain is `domain`, normalized the way the Go
    /// manager looks it up.
    fn rule(&self, domain: &str) -> Result<ProxyRule, String> {
        let domain = domain.trim().to_lowercase();
        let state = self.state()?;
        state
            .proxy_rules
            .into_iter()
            .find(|rule| rule.domain == domain)
            .ok_or_else(|| "proxy rule is missing".to_string())
    }
}

/// The Go `requireProxyStopped` gate: the user proxy job and the privileged
/// socket daemon must both be verifiably gone before a certificate swaps.
struct StoppedProxy {
    engine: Engine,
}

impl ProxyStopCheck for StoppedProxy {
    fn verify_stopped(&self) -> Result<(), String> {
        for id in ["proxy", "proxy-sockets"] {
            let (job, result) = read_job_state(&self.engine, id, &|| false);
            match result {
                Ok(()) if job.running && job.pid > 0 => {
                    return Err(format!(
                        "stop the HTTPS proxy before switching authorities: {} is still running",
                        self.engine.target(id)
                    ));
                }
                Ok(()) => {}
                Err(error) => {
                    let message = error.to_string();
                    if !message.contains("Could not find service")
                        && !message.contains("Could not find specified service")
                    {
                        return Err(format!(
                            "stop the HTTPS proxy before switching authorities: {message}"
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// A compact multi-threaded runtime shared by asynchronous devproxy operations
/// invoked from blocking worker threads, so concurrent probes and Cloudflare
/// calls do not serialize on a single current-thread driver.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a multi-thread runtime")
    });
    &RUNTIME
}

fn job_status(engine: &Engine, id: &str) -> (bool, String) {
    let (job, result) = read_job_state(engine, id, &|| false);
    match result {
        Ok(()) => (
            true,
            if job.running { "Running" } else { "Failed" }.to_string(),
        ),
        Err(error) => {
            let message = error.to_string();
            if message.contains("Could not find service")
                || message.contains("Could not find specified service")
            {
                (true, "Stopped".to_string())
            } else {
                (true, "Unknown".to_string())
            }
        }
    }
}

impl Provider for Facade {
    fn snapshot(&self) -> Result<Snapshot, String> {
        let state = self.state()?;
        let root = self.root().to_path_buf();
        let secrets = SecretStore::new(&root);
        let engine = Engine::new(&root, Arc::new(ProcessRunnerAdapter::new(Self::runner())));
        let mut statuses = BTreeMap::new();
        if secrets.has_cloudflare_token() {
            statuses.insert("tunnel:token".to_string(), "Configured".to_string());
        }

        let proxy_plist = engine.job_path("proxy").exists();
        if !state.proxy_rules.is_empty() || proxy_plist {
            let (_, status) = job_status(&engine, "proxy");
            statuses.insert("proxy".to_string(), status);
        }

        let any_tunnel_rule = state.proxy_rules.iter().any(|rule| rule.tunnel);
        let tunnel_plist = engine.job_path("tunnel").exists();
        if secrets.has_cloudflare_token()
            || !state.cloudflare_account_id.is_empty()
            || !state.tunnel_workers.is_empty()
            || any_tunnel_rule
            || tunnel_plist
        {
            let (_, status) = job_status(&engine, "tunnel");
            if status == "Running"
                && let Some(tunnel) = read_tunnel_status(&root)
            {
                for (name, worker_status) in tunnel.worker_states {
                    if !name.is_empty() && !worker_status.is_empty() {
                        statuses.insert(format!("tunnel:worker:{name}"), worker_status);
                    }
                }
            }
            statuses.insert("tunnel".to_string(), status);
        }

        let publication = devproxy::proxy::rules::publication_state(&self.store)
            .map(|value| value.to_string())
            .unwrap_or_else(|_| "Unknown".to_string());

        let mut workers = BTreeMap::new();
        for (name, worker) in &state.tunnel_workers {
            workers.insert(name.clone(), worker.public_url.clone());
        }

        Ok(Snapshot {
            rules: state.proxy_rules.clone(),
            listener_port: state.proxy_listener_port,
            revision: state.proxy_rules_revision,
            publication,
            statuses,
            token_configured: secrets.has_cloudflare_token(),
            account_id: state.cloudflare_account_id.clone(),
            client_id: state.tunnel_client_id.clone(),
            subdomain: state.cloudflare_subdomain.clone(),
            zones: state
                .cloudflare_zones
                .iter()
                .map(|zone| zone.name.clone())
                .collect(),
            dns_names: state
                .cloudflare_dns
                .iter()
                .map(|record| record.name.clone())
                .collect(),
            workers: Workers(workers),
        })
    }

    fn listening_ports(&self) -> Result<Vec<ListeningPort>, String> {
        Databases::new(self.store.clone())
            .listening_user_ports(&|| false)
            .map_err(|error| error.to_string())
    }

    fn save_rules(
        &self,
        expected_revision: u64,
        listener_port: i64,
        rules: Vec<ProxyRule>,
    ) -> Result<(), String> {
        let normalized =
            normalize_proxy_rules(listener_port, &rules).map_err(|error| error.to_string())?;
        self.store
            .with_state(|state| {
                if state.proxy_rules_revision != expected_revision {
                    return Err(devcore::state::StateError::invalid(
                        "proxy configuration changed; reload before saving",
                    ));
                }
                state.proxy_rules = normalized;
                state.proxy_listener_port = listener_port;
                state.proxy_rules_revision += 1;
                Ok(())
            })
            .map_err(|error| error.to_string())
    }

    fn apply_proxy(&self) -> Result<(), String> {
        self.proxy().apply().map_err(|error| error.to_string())
    }

    fn stop_proxy(&self) -> Result<(), String> {
        self.proxy().stop().map_err(|error| error.to_string())
    }

    fn restart_proxy(&self) -> Result<(), String> {
        self.proxy().restart().map_err(|error| error.to_string())
    }

    fn proxy_result(&self, domain: &str) -> Result<ProxyHttpResult, String> {
        let (fingerprint, revision) =
            proxy_fingerprint(&self.store, domain).map_err(|error| error.to_string())?;
        let result = runtime()
            .block_on(check_proxy_rule_http(
                &self.store,
                domain,
                revision,
                &fingerprint,
            ))
            .map_err(|error| error.to_string())?;
        match proxy_fingerprint(&self.store, domain) {
            Ok((latest_fingerprint, latest_revision))
                if latest_fingerprint == fingerprint && latest_revision == revision =>
            {
                Ok(result)
            }
            _ => Err("HTTPS rule changed; check again.".to_string()),
        }
    }

    fn tunnel_result(&self, domain: &str) -> Result<ProxyHttpResult, String> {
        let (fingerprint, revision) =
            tunnel_fingerprint(&self.store, domain).map_err(|error| error.to_string())?;
        let result = runtime()
            .block_on(check_tunnel_rule_http(
                &self.store,
                domain,
                revision,
                &fingerprint,
            ))
            .map_err(|error| error.to_string())?;
        match tunnel_fingerprint(&self.store, domain) {
            Ok((latest_fingerprint, latest_revision))
                if latest_fingerprint == fingerprint && latest_revision == revision =>
            {
                Ok(result)
            }
            _ => Err("Tunnel rule changed; check again.".to_string()),
        }
    }

    fn authority_status(&self) -> Result<AuthorityStatus, String> {
        self.authority().status().map_err(|error| error.to_string())
    }

    fn authority_action(&self, action: AuthorityAction) -> Result<(), String> {
        let service = self.authority();
        let result = match action {
            AuthorityAction::Prepare => service.prepare(),
            AuthorityAction::TrustActive => service.configure_active_trust(false),
            AuthorityAction::RemoveActiveTrust => service.configure_active_trust(true),
            AuthorityAction::TrustPrepared { revision } => service.trust_prepared(&revision),
            AuthorityAction::Activate { revision } => service.activate(&revision),
            AuthorityAction::DiscardPrepared { revision } => service.discard_prepared(&revision),
            AuthorityAction::Restore {
                id,
                fingerprint,
                revision,
            } => service.restore(&id, &fingerprint, &revision),
            AuthorityAction::TrustSaved { id, fingerprint } => {
                service.configure_saved_trust(&id, &fingerprint, false)
            }
            AuthorityAction::RemoveSavedTrust { id, fingerprint } => {
                service.remove_archived_trust(&id, &fingerprint)
            }
        };
        result.map_err(|error| error.to_string())
    }

    fn save_cloudflare(
        &self,
        token_path: Option<&Path>,
        account_id: &str,
        client_id: &str,
    ) -> Result<SavedCloudflare, String> {
        let token = match token_path {
            Some(path) => Some(read_token_file(path)?),
            None => None,
        };
        self.jobs()
            .service()
            .save_settings(token.as_deref().unwrap_or_default(), account_id, client_id)
            .map_err(|error| error.to_string())?;
        let state = self.state()?;
        Ok(SavedCloudflare {
            token_configured: SecretStore::new(self.root()).has_cloudflare_token(),
            account_id: state.cloudflare_account_id,
        })
    }

    fn refresh_cloudflare(&self) -> Result<(), String> {
        runtime()
            .block_on(self.tunnel().refresh_metadata())
            .map_err(|error| error.to_string())
    }

    fn deploy_workers(&self) -> Result<(), String> {
        runtime()
            .block_on(self.tunnel().sync_tunnels())
            .map_err(|error| error.to_string())
    }

    fn start_tunnel(&self) -> Result<(), String> {
        runtime()
            .block_on(self.jobs().apply())
            .map_err(|error| error.to_string())
    }

    fn stop_tunnel(&self) -> Result<(), String> {
        self.jobs().stop().map_err(|error| error.to_string())
    }

    fn restart_tunnel(&self) -> Result<(), String> {
        self.jobs().restart().map_err(|error| error.to_string())
    }

    fn remove_cloudflare(&self, delete_workers: bool) -> Result<(), String> {
        runtime()
            .block_on(self.tunnel().remove_settings(delete_workers))
            .map_err(|error| error.to_string())
    }

    fn ensure_tunnel_token(&self, domain: &str) -> Result<String, String> {
        let rule = self.rule(domain)?;
        if !rule.tunnel {
            return Err("tunnel is not enabled for this rule".to_string());
        }
        SecretStore::new(self.root())
            .ensure_tunnel_access_token(&rule.resolved_tunnel_worker())
            .map_err(|error| error.to_string())
    }

    fn rotate_tunnel_token(&self, domain: &str) -> Result<(), String> {
        let rule = self.rule(domain)?;
        if !rule.tunnel {
            return Err("tunnel is not enabled for this rule".to_string());
        }
        SecretStore::new(self.root())
            .rotate_tunnel_access_token(&rule.resolved_tunnel_worker())
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn logs(&self, job: &str) -> Result<String, String> {
        let mut out = String::new();
        for suffix in [".log", ".err.log"] {
            let path = self.root().join("logs").join(format!("{job}{suffix}"));
            match tail(&path) {
                Ok(snapshot) => {
                    if snapshot.lines.is_empty() {
                        continue;
                    }
                    out.push_str(&format!("{job}{suffix}"));
                    if snapshot.truncated {
                        out.push_str(" (last 200 lines / 64 KiB)");
                    }
                    out.push_str(":\n");
                    out.push_str(&snapshot.lines.join("\n"));
                    out.push_str("\n\n");
                }
                Err(error) if error.contains("No such file") => continue,
                Err(error) => return Err(error),
            }
        }
        if out.is_empty() {
            return Ok("No logs yet. Start the service to capture its output.".to_string());
        }
        Ok(out.trim_end().to_string())
    }
}

/// Read a Cloudflare token file without echoing it. The cap matches the Go
/// reader: a real token is far smaller.
fn read_token_file(path: &Path) -> Result<String, String> {
    use std::io::Read;

    let file = std::fs::File::open(path)
        .map_err(|error| format!("read Cloudflare token file: {error}"))?;
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read Cloudflare token file: {error}"))?;
    if bytes.len() > 4096 {
        return Err("Cloudflare token file is too large".to_string());
    }
    let token = String::from_utf8(bytes)
        .map_err(|_| "Cloudflare token file is not UTF-8 text".to_string())?;
    validate_cloudflare_token(&token)
        .map(|token| token.trim().to_string())
        .map_err(|error| error.to_string())
}

// --- Providers ---------------------------------------------------------------

static FAKE: LazyLock<Mutex<Option<Arc<dyn Provider>>>> = LazyLock::new(|| Mutex::new(None));
static FAKE_MODE: AtomicBool = AtomicBool::new(false);

fn fake() -> &'static Mutex<Option<Arc<dyn Provider>>> {
    &FAKE
}

static FACADE: LazyLock<Facade> = LazyLock::new(Facade::new);

fn provider_of() -> Option<Arc<dyn Provider>> {
    if let Some(provider) = fake()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return Some(provider);
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return None;
    }
    Some(Arc::new(Facade {
        store: FACADE.store.clone(),
    }) as Arc<dyn Provider>)
}

/// Makes the surface use a test provider instead of devproxy. Call it before
/// starting the headless surface, then [`install_provider`]. Tests only.
pub fn use_fake_provider() {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *fake()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Gives the surface a provider of the test's own. Tests only.
pub fn install_provider(provider: Arc<dyn Provider>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *fake()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

// --- State -------------------------------------------------------------------

/// The saved snapshot and workspace loading state.
#[derive(Debug, Clone, Default)]
struct Workspace {
    snapshot: Snapshot,
    loading: bool,
    loaded: bool,
    load_error: Option<String>,
    /// Bumped for every read, so an older answer never replaces a newer one.
    sequence: u64,
}

/// The editable proxy rules table, kept separate from transient notices,
/// port scans, and tunnel inputs so unrelated updates never thrash rule rows.
#[derive(Debug, Clone, Default)]
struct RulesDraft {
    rules: Vec<RuleDraft>,
    next_rule_id: u64,
    revision: u64,
    loaded: bool,
}

/// The editable HTTPS listener port field.
#[derive(Debug, Clone)]
struct ListenerDraft {
    port: String,
}

impl Default for ListenerDraft {
    fn default() -> Self {
        ListenerDraft {
            port: "443".to_string(),
        }
    }
}

/// The tunnel card's unsaved fields. The token is only ever a file path.
#[derive(Debug, Clone, Default)]
struct TunnelDraft {
    account_id: String,
    client_id: String,
    token_file: Option<PathBuf>,
    loaded: bool,
}

/// Listening local port discovery state, isolated from notices and rule inputs
/// so background scans never collapse open menus or disturb form focus.
#[derive(Debug, Clone, Default)]
struct PortsState {
    ports: Vec<ListeningPort>,
    loaded: bool,
    loading: bool,
    error: Option<String>,
}

/// Per-rule activation HTTP and tunnel probe state.
#[derive(Debug, Clone, Default)]
struct ChecksState {
    proxy_checks: BTreeMap<String, ProxyHttpResult>,
    proxy_checking: BTreeSet<String>,
    proxy_generation: u64,
    tunnel_checks: BTreeMap<String, ProxyHttpResult>,
    tunnel_checking: BTreeSet<String>,
    tunnel_generation: u64,
}

/// Local certificate authority page state.
#[derive(Debug, Clone, Default)]
struct AuthorityState {
    authority: Option<AuthorityStatus>,
    open: bool,
}

/// In-flight operation keys.
#[derive(Debug, Clone, Default)]
struct BusyState {
    busy: BTreeSet<String>,
}

/// Action errors, tracked transient toast id, and inline log snapshots.
#[derive(Debug, Clone, Default)]
struct NoticesState {
    action_error: Option<String>,
    notice_toast: Option<u64>,
    logs: Option<(String, String)>,
}

fn workspace() -> Signal<Workspace> {
    global::<Workspace>()
}

fn rules_draft() -> Signal<RulesDraft> {
    global::<RulesDraft>()
}

fn listener_draft() -> Signal<ListenerDraft> {
    global::<ListenerDraft>()
}

fn tunnel_draft() -> Signal<TunnelDraft> {
    global::<TunnelDraft>()
}

fn ports_state() -> Signal<PortsState> {
    global::<PortsState>()
}

fn checks_state() -> Signal<ChecksState> {
    global::<ChecksState>()
}

fn authority_state() -> Signal<AuthorityState> {
    global::<AuthorityState>()
}

fn busy_state() -> Signal<BusyState> {
    global::<BusyState>()
}

fn notices_state() -> Signal<NoticesState> {
    global::<NoticesState>()
}

/// Runs store work outside the view that asked for it: a row or pane the
/// store change rebuilds must not cancel the task that is changing it.
fn task(future: impl Future<Output = ()> + 'static) {
    Owner::detached().run(move || {
        spawn(future);
    });
}

/// Runs one provider call on a blocking thread and flattens the thread's own
/// failure into the verbatim error channel.
async fn fetch<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    match blocking(f).await {
        Ok(answer) => answer,
        Err(error) => Err(format!("the task did not answer: {error}")),
    }
}

// --- Drafts ------------------------------------------------------------------

fn load_rules_draft(snapshot: &Snapshot) {
    let rules: Vec<RuleDraft> = snapshot
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| RuleDraft::from_rule(index as u64, rule))
        .collect();
    let next_rule_id = rules.len() as u64;
    let port = if snapshot.listener_port == 0 {
        443
    } else {
        snapshot.listener_port
    }
    .to_string();
    rules_draft().set(RulesDraft {
        rules,
        next_rule_id,
        revision: snapshot.revision,
        loaded: true,
    });
    if untracked(|| listener_draft().with(|state| state.port != port)) {
        listener_draft().set(ListenerDraft { port });
    }
}

fn load_tunnel_draft(snapshot: &Snapshot) {
    let already_matches = untracked(|| {
        tunnel_draft().with(|tunnel| {
            tunnel.loaded
                && tunnel.account_id == snapshot.account_id
                && tunnel.client_id == snapshot.client_id
                && tunnel.token_file.is_none()
        })
    });
    if !already_matches {
        tunnel_draft().set(TunnelDraft {
            account_id: snapshot.account_id.clone(),
            client_id: snapshot.client_id.clone(),
            token_file: None,
            loaded: true,
        });
    }
}

/// Whether the rule draft differs from the saved table.
fn is_rules_draft_dirty(draft: &RulesDraft, listener_port: &str, snapshot: &Snapshot) -> bool {
    if !draft.loaded {
        return false;
    }
    let saved_port = if snapshot.listener_port == 0 {
        443
    } else {
        snapshot.listener_port
    };
    if listener_port.trim() != saved_port.to_string() || draft.rules.len() != snapshot.rules.len() {
        return true;
    }
    draft
        .rules
        .iter()
        .zip(snapshot.rules.iter())
        .any(|(row, rule)| {
            local_domain(&row.domain) != rule.domain
                || row.local_port.trim() != rule.local_port.to_string()
                || row.upstream_host.trim() != rule.upstream_host
                || row.tunnel != rule.tunnel
                || row.tunnel_auth != rule.tunnel_auth
                || row.tunnel_worker != rule.tunnel_worker
                || row.effective_tunnel_domain() != rule.tunnel_domain
        })
}

fn is_tunnel_draft_dirty(tunnel: &TunnelDraft, snapshot: &Snapshot) -> bool {
    if !tunnel.loaded {
        return false;
    }
    tunnel.account_id != snapshot.account_id
        || tunnel.client_id != snapshot.client_id
        || tunnel.token_file.is_some()
}

fn sync_drafts_with_snapshot(snapshot: &Snapshot) {
    let should_load_rules = untracked(|| {
        rules_draft().with(|draft| {
            if !draft.loaded {
                return true;
            }
            if draft.revision == snapshot.revision {
                return false;
            }
            listener_draft().with(|listener| !is_rules_draft_dirty(draft, &listener.port, snapshot))
        })
    });
    if should_load_rules {
        load_rules_draft(snapshot);
    }

    let should_load_tunnel = untracked(|| {
        tunnel_draft().with(|tunnel| !tunnel.loaded || !is_tunnel_draft_dirty(tunnel, snapshot))
    });
    if should_load_tunnel {
        load_tunnel_draft(snapshot);
    }
}

fn dismiss_notice_toast() {
    let id = untracked(|| notices_state().with(|state| state.notice_toast));
    if let Some(id) = id {
        notices_state().update(|state| {
            if state.notice_toast == Some(id) {
                state.notice_toast = None;
            }
        });
        crate::toast::dismiss(id);
    }
}

fn clear_feedback() {
    dismiss_notice_toast();
    let has_error = untracked(|| notices_state().with(|state| state.action_error.is_some()));
    if has_error {
        notices_state().update(|state| {
            state.action_error = None;
        });
    }
}

// --- Reads -------------------------------------------------------------------

/// Reads the development home the first time the screen appears.
pub(crate) fn load() {
    let (loaded, loading) = workspace().with(|state| (state.loaded, state.loading));
    if !loaded && !loading {
        refresh();
    }
    if ports_state().with(|state| !state.loaded && !state.loading) {
        refresh_ports();
    }
}

/// Reads the development home and the authority again.
pub(crate) fn refresh() {
    let Some(provider) = provider_of() else {
        workspace().update(|state| {
            state.loading = false;
            state.load_error = Some("no HTTPS proxy provider is available".to_string());
        });
        return;
    };
    let sequence = untracked(|| workspace().with(|state| state.sequence + 1));
    workspace().update(|state| {
        state.sequence = sequence;
        state.loading = true;
        state.load_error = None;
    });
    task(async move {
        let answer = blocking(move || (provider.snapshot(), provider.authority_status())).await;
        let (snapshot, authority) = match answer {
            Ok(answer) => answer,
            Err(error) => (
                Err(format!("the HTTPS proxy workspace did not answer: {error}")),
                Err(String::new()),
            ),
        };
        let mut stale = false;
        let mut next_snapshot = None;
        workspace().update(|state| {
            if state.sequence != sequence {
                stale = true;
                return;
            }
            state.loading = false;
            state.loaded = true;
            match snapshot {
                Ok(snapshot) => {
                    state.load_error = None;
                    state.snapshot = snapshot.clone();
                    next_snapshot = Some(snapshot);
                }
                Err(error) => state.load_error = Some(error),
            }
        });
        if stale {
            return;
        }
        if let Some(snapshot) = next_snapshot {
            sync_drafts_with_snapshot(&snapshot);
        }
        if let Ok(status) = authority {
            store_authority(status);
        }
    });
}

pub(crate) fn load_error() -> Option<String> {
    workspace().with(|state| state.load_error.clone())
}

/// The saved rule table.
pub(crate) fn saved_rules() -> Vec<ProxyRule> {
    workspace().with(|state| state.snapshot.rules.clone())
}

/// The listener the saved table publishes on (`443` when unset).
pub(crate) fn listener_port() -> i64 {
    workspace().with(|state| {
        if state.snapshot.listener_port == 0 {
            443
        } else {
            state.snapshot.listener_port
        }
    })
}

pub(crate) fn publication() -> String {
    workspace().with(|state| {
        if state.snapshot.publication.is_empty() {
            "Unknown".to_string()
        } else {
            state.snapshot.publication.clone()
        }
    })
}

fn status_of(state: &Workspace, key: &str) -> Option<String> {
    state.snapshot.statuses.get(key).cloned()
}

/// The proxy service's status, as the Go card defaults it.
pub(crate) fn proxy_status() -> String {
    workspace().with(|state| {
        status_of(state, "proxy").unwrap_or_else(|| {
            if state.snapshot.rules.is_empty() {
                "Stopped".to_string()
            } else {
                "Unknown".to_string()
            }
        })
    })
}

pub(crate) fn proxy_on() -> bool {
    proxy_status() == "Running"
}

pub(crate) fn tunnel_status() -> String {
    workspace().with(|state| status_of(state, "tunnel").unwrap_or_else(|| "Stopped".to_string()))
}

pub(crate) fn tunnel_on() -> bool {
    tunnel_status() == "Running"
}

pub(crate) fn token_configured() -> bool {
    workspace().with(|state| state.snapshot.token_configured)
}

pub(crate) fn has_tunnel_rules() -> bool {
    workspace().with(|state| state.snapshot.rules.iter().any(|rule| rule.tunnel))
}

/// How many Workers the saved Cloudflare configuration deployed.
pub(crate) fn worker_count() -> usize {
    workspace().with(|state| state.snapshot.workers.len())
}

/// The tunnel's per-observation detail (`tunnel:detail`), when it has one.
pub(crate) fn tunnel_detail() -> String {
    workspace().with(|state| {
        state
            .snapshot
            .statuses
            .get("tunnel:detail")
            .cloned()
            .unwrap_or_default()
    })
}

// --- Draft accessors ---------------------------------------------------------

pub(crate) fn rules() -> Vec<RuleDraft> {
    rules_draft().with(|state| state.rules.clone())
}

pub(crate) fn listener_port_field() -> String {
    listener_draft().with(|state| state.port.clone())
}

pub(crate) fn set_listener_port(next: String) {
    listener_draft().update(|state| state.port = next);
    clear_feedback();
}

pub(crate) fn add_rule() {
    rules_draft().update(|state| {
        let id = state.next_rule_id;
        state.next_rule_id += 1;
        let mut row = RuleDraft::new();
        row.id = id;
        state.rules.push(row);
    });
    clear_feedback();
}

pub(crate) fn remove_rule(index: usize) {
    rules_draft().update(|state| {
        if index < state.rules.len() {
            state.rules.remove(index);
        }
    });
    clear_feedback();
}

pub(crate) fn set_domain(index: usize, next: String) {
    update_rule(index, |row| row.domain = next);
}

pub(crate) fn set_upstream(index: usize, next: String) {
    update_rule(index, |row| row.upstream_host = next);
}

pub(crate) fn set_local_port(index: usize, next: String) {
    update_rule(index, |row| row.local_port = next);
}

pub(crate) fn set_tunnel(index: usize, on: bool) {
    update_rule(index, |row| row.tunnel = on);
}

pub(crate) fn set_tunnel_auth(index: usize, on: bool) {
    update_rule(index, |row| row.tunnel_auth = on);
}

pub(crate) fn set_tunnel_mode(index: usize, mode: &str) {
    let mode = mode.to_string();
    update_rule(index, move |row| {
        row.tunnel_mode = mode;
        if row.tunnel_mode != TUNNEL_MODE_CUSTOM_DOMAIN {
            row.tunnel_domain.clear();
        }
    });
}

pub(crate) fn set_tunnel_domain(index: usize, next: String) {
    update_rule(index, |row| row.tunnel_domain = next);
}

fn update_rule(index: usize, change: impl FnOnce(&mut RuleDraft)) {
    rules_draft().update(|state| {
        if let Some(row) = state.rules.get_mut(index) {
            change(row);
        }
    });
    clear_feedback();
}

pub(crate) fn dirty() -> bool {
    workspace().with(|ws| {
        rules_draft().with(|draft| {
            listener_draft()
                .with(|listener| is_rules_draft_dirty(draft, &listener.port, &ws.snapshot))
        })
    })
}

pub(crate) fn tunnel_dirty() -> bool {
    workspace().with(|ws| tunnel_draft().with(|tunnel| is_tunnel_draft_dirty(tunnel, &ws.snapshot)))
}

/// Throw the rule edits away and show the saved table.
pub(crate) fn discard() {
    let snapshot = untracked(|| workspace().with(|state| state.snapshot.clone()));
    load_rules_draft(&snapshot);
    clear_feedback();
}

/// The review URL of a rule that is clean enough to open, exactly as the Go
/// card builds it.
pub(crate) fn rule_url(index: usize) -> String {
    let port = workspace().with(|state| state.snapshot.listener_port);
    let domain = rules_draft().with(|state| {
        state
            .rules
            .get(index)
            .map(RuleDraft::to_rule)
            .map(|rule| rule.domain)
            .unwrap_or_default()
    });
    let mut url = format!("https://{domain}");
    if port != 0 && port != 443 {
        url = format!("{url}:{port}");
    }
    url
}

/// The preview of a draft rule's public tunnel URL.
pub(crate) fn tunnel_preview(index: usize) -> String {
    let rule = rules_draft().with(|state| {
        let row = state.rules.get(index)?;
        if !row.tunnel {
            return None;
        }
        Some(row.to_rule())
    });
    let Some(rule) = rule else {
        return String::new();
    };
    workspace().with(|state| preview_tunnel_url(&rule, &state.snapshot.subdomain))
}

/// The Worker name a draft rule resolves to.
pub(crate) fn tunnel_worker(index: usize) -> String {
    rules_draft().with(|state| {
        state
            .rules
            .get(index)
            .map(RuleDraft::to_rule)
            .map(|rule| rule.resolved_tunnel_worker())
            .unwrap_or_default()
    })
}

pub(crate) fn subdomain() -> String {
    workspace().with(|state| state.snapshot.subdomain.clone())
}

pub(crate) fn zones() -> Vec<String> {
    workspace().with(|state| state.snapshot.zones.clone())
}

/// The Go `cloudflareHostSuggestions`: existing DNS names plus each zone and
/// the rule's default Worker name under it, deduplicated and sorted.
pub(crate) fn host_suggestions(index: usize) -> Vec<String> {
    let local = rules_draft().with(|state| {
        state
            .rules
            .get(index)
            .map(RuleDraft::to_rule)
            .map(|rule| rule.domain)
            .unwrap_or_default()
    });
    let (dns, zones) = workspace().with(|state| {
        (
            state.snapshot.dns_names.clone(),
            state.snapshot.zones.clone(),
        )
    });
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut add = |host: &str| {
        let host = host.trim().to_lowercase();
        if host.is_empty() || host.starts_with("*.") || !host.contains('.') {
            return;
        }
        if seen.insert(host.clone()) {
            out.push(host);
        }
    };
    for name in dns {
        add(&name);
    }
    let worker = devcore::model::default_tunnel_worker_name(&local);
    let slug = worker.strip_prefix("devbase-").unwrap_or(&worker);
    for zone in zones {
        if !slug.is_empty() {
            add(&format!("{slug}.{zone}"));
        }
        add(&zone);
    }
    out.sort();
    out
}

// --- Listening ports ---------------------------------------------------------

pub(crate) fn ports() -> Vec<ListeningPort> {
    ports_state().with(|state| state.ports.clone())
}

pub(crate) fn ports_loading() -> bool {
    ports_state().with(|state| state.loading)
}

pub(crate) fn ports_error() -> Option<String> {
    ports_state().with(|state| state.error.clone())
}

pub(crate) fn refresh_ports() {
    if untracked(|| ports_state().with(|state| state.loading)) {
        return;
    }
    let Some(provider) = provider_of() else {
        return;
    };
    ports_state().update(|state| {
        state.loading = true;
        state.error = None;
    });
    task(async move {
        let answer = fetch(move || provider.listening_ports()).await;
        ports_state().update(|state| {
            state.loading = false;
            state.loaded = true;
            match answer {
                Ok(ports) => {
                    state.ports = ports;
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
        });
    });
}

// --- Errors and busy ---------------------------------------------------------

pub(crate) fn busy() -> bool {
    busy_state().with(|state| !state.busy.is_empty())
}

pub(crate) fn action_error() -> Option<String> {
    notices_state().with(|state| state.action_error.clone())
}

pub(crate) fn logs() -> Option<(String, String)> {
    notices_state().with(|state| state.logs.clone())
}

pub(crate) fn dismiss_logs() {
    notices_state().update(|state| state.logs = None);
}

fn begin(key: &str) -> bool {
    if untracked(busy) {
        return false;
    }
    busy_state().update(|state| {
        state.busy.insert(key.to_string());
    });
    dismiss_notice_toast();
    let has_transient = untracked(|| {
        notices_state().with(|state| state.action_error.is_some() || state.logs.is_some())
    });
    if has_transient {
        notices_state().update(|state| {
            state.action_error = None;
            state.logs = None;
        });
    }
    true
}

fn finish(key: &str) {
    busy_state().update(|state| {
        state.busy.remove(key);
    });
}

fn fail(message: impl Into<String>) {
    dismiss_notice_toast();
    notices_state().update(|state| state.action_error = Some(message.into()));
}

fn succeed(message: impl Into<String>) {
    dismiss_notice_toast();
    let id = crate::toast::push_tracked(crate::toast::Tone::Success, message);
    notices_state().update(|state| {
        state.action_error = None;
        state.notice_toast = id;
    });
}

/// The rule table as the screen would save it; the exact Go error when a port
/// field is not a number.
fn build_rules() -> Result<(i64, Vec<ProxyRule>), String> {
    let listener: i64 = untracked(|| listener_draft().with(|state| state.port.clone()))
        .trim()
        .parse()
        .map_err(|_| "Enter valid numeric ports".to_string())?;
    untracked(|| {
        rules_draft().with(|state| {
            let mut rules = Vec::with_capacity(state.rules.len());
            for row in &state.rules {
                let port: i64 = row
                    .local_port
                    .trim()
                    .parse()
                    .map_err(|_| "Enter valid numeric ports".to_string())?;
                let mut rule = row.to_rule();
                rule.local_port = port;
                rules.push(rule);
            }
            Ok((listener, rules))
        })
    })
}

fn revision() -> u64 {
    untracked(|| rules_draft().with(|state| state.revision))
}

// --- Saving and applying -----------------------------------------------------

/// `Apply proxy`: publish the saved table. The screen shows this button only
/// while the draft is clean.
pub(crate) fn apply() {
    if dirty() || !begin("apply") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("apply");
        fail("no HTTPS proxy provider is available");
        return;
    };
    task(async move {
        let answer = fetch(move || provider.apply_proxy()).await;
        finish("apply");
        match answer {
            Ok(()) => {
                succeed("Proxy configuration published. Check DNS, TLS and upstream separately.")
            }
            Err(error) => fail(error),
        }
        refresh();
    });
}

/// `Save HTTPS rules`: validate, save at the draft's revision, then sync
/// Cloudflare when a token is configured.
pub(crate) fn save() {
    if !begin("save") {
        return;
    }
    let Ok((listener, rules)) = build_rules() else {
        finish("save");
        fail("Enter valid numeric ports");
        return;
    };
    let expected = revision();
    let Some(provider) = provider_of() else {
        finish("save");
        fail("no HTTPS proxy provider is available");
        return;
    };
    task(async move {
        let saved = fetch({
            let rules = rules.clone();
            let provider = Arc::clone(&provider);
            move || provider.save_rules(expected, listener, rules)
        })
        .await;
        match saved {
            Ok(()) => {}
            Err(error) => {
                finish("save");
                fail(error);
                return;
            }
        }
        let if_token = fetch({
            let provider = Arc::clone(&provider);
            move || {
                provider
                    .snapshot()
                    .map(|snapshot| snapshot.token_configured)
            }
        })
        .await
        .unwrap_or(false);
        if if_token {
            let synced = fetch({
                let provider = Arc::clone(&provider);
                move || provider.deploy_workers()
            })
            .await;
            finish("save");
            match synced {
                Ok(()) => succeed("Saved HTTPS rules and synced Cloudflare Workers."),
                Err(error) => fail(format!(
                    "saved HTTPS rules; Cloudflare sync failed: {error}"
                )),
            }
        } else {
            finish("save");
            succeed("Saved HTTPS rules.");
        }
        refresh();
    });
}

/// `Save & apply`: save, sync when a token is configured, apply the proxy,
/// and keep the tunnel where it was running.
pub(crate) fn save_and_apply() {
    if !begin("save") {
        return;
    }
    let Ok((listener, rules)) = build_rules() else {
        finish("save");
        fail("Enter valid numeric ports");
        return;
    };
    let expected = revision();
    let tunnel_was_running = tunnel_on();
    let Some(provider) = provider_of() else {
        finish("save");
        fail("no HTTPS proxy provider is available");
        return;
    };
    task(async move {
        let saved = fetch({
            let rules = rules.clone();
            let provider = Arc::clone(&provider);
            move || provider.save_rules(expected, listener, rules)
        })
        .await;
        if let Err(error) = saved {
            finish("save");
            fail(error);
            return;
        }
        let token_configured = fetch({
            let provider = Arc::clone(&provider);
            move || {
                provider
                    .snapshot()
                    .map(|snapshot| snapshot.token_configured)
            }
        })
        .await
        .unwrap_or(false);
        let mut synced = false;
        if token_configured {
            match fetch({
                let provider = Arc::clone(&provider);
                move || provider.deploy_workers()
            })
            .await
            {
                Ok(()) => synced = true,
                Err(error) => {
                    finish("save");
                    fail(format!(
                        "saved HTTPS rules; Cloudflare sync failed: {error}"
                    ));
                    refresh();
                    return;
                }
            }
        }
        if let Err(error) = fetch({
            let provider = Arc::clone(&provider);
            move || provider.apply_proxy()
        })
        .await
        {
            finish("save");
            fail(error);
            refresh();
            return;
        }
        if tunnel_was_running {
            let has_tunnel = rules.iter().any(|rule| rule.tunnel);
            let answer = fetch({
                let provider = Arc::clone(&provider);
                move || {
                    if has_tunnel {
                        provider.start_tunnel()
                    } else {
                        provider.stop_tunnel()
                    }
                }
            })
            .await;
            if let Err(error) = answer {
                finish("save");
                fail(error);
                refresh();
                return;
            }
        }
        finish("save");
        if synced {
            succeed("Saved, synced Cloudflare Workers, and applied proxy configuration.");
        } else {
            succeed("Saved and applied proxy configuration.");
        }
        refresh();
    });
}

// --- The proxy service -------------------------------------------------------

/// The Proxy card's switch: apply what is saved, or stop the job.
pub(crate) fn set_proxy_enabled(on: bool) {
    if dirty() || !begin("proxy") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("proxy");
        fail("no HTTPS proxy provider is available");
        return;
    };
    task(async move {
        let answer = fetch(move || {
            if on {
                provider.apply_proxy()
            } else {
                provider.stop_proxy()
            }
        })
        .await;
        finish("proxy");
        match answer {
            Ok(()) if on => {
                succeed("Proxy configuration published. Check DNS, TLS and upstream separately.")
            }
            Ok(()) => succeed("Stopped HTTPS proxy"),
            Err(error) => fail(error),
        }
        refresh();
    });
}

pub(crate) fn restart_proxy() {
    if dirty() || !begin("proxy") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("proxy");
        fail("no HTTPS proxy provider is available");
        return;
    };
    task(async move {
        let answer = fetch(move || provider.restart_proxy()).await;
        finish("proxy");
        match answer {
            Ok(()) => succeed("Restart HTTPS proxy"),
            Err(error) => fail(error),
        }
        refresh();
    });
}

pub(crate) fn read_logs(job: &str) {
    if !begin("logs") {
        return;
    }
    let title = if job == "proxy" {
        "HTTPS proxy log"
    } else {
        "Cloudflare tunnel log"
    };
    let job = job.to_string();
    let Some(provider) = provider_of() else {
        finish("logs");
        fail("no HTTPS proxy provider is available");
        return;
    };
    task(async move {
        let answer = fetch(move || provider.logs(&job)).await;
        finish("logs");
        match answer {
            Ok(text) => notices_state().update(|state| {
                state.logs = Some((title.to_string(), text));
            }),
            Err(error) => fail(error),
        }
    });
}

// --- The Cloudflare tunnel ---------------------------------------------------

pub(crate) fn account_id_field() -> String {
    tunnel_draft().with(|state| state.account_id.clone())
}

pub(crate) fn client_id_field() -> String {
    tunnel_draft().with(|state| state.client_id.clone())
}

pub(crate) fn set_account_id(next: String) {
    tunnel_draft().update(|state| state.account_id = next);
    clear_feedback();
}

pub(crate) fn set_client_id(next: String) {
    tunnel_draft().update(|state| state.client_id = next);
    clear_feedback();
}

/// `Choose token file…`: remember only the path. The provider reads the token
/// into its 0600 file; the store never sees the text.
pub(crate) fn choose_token_file(path: PathBuf) {
    tunnel_draft().update(|state| state.token_file = Some(path));
    clear_feedback();
}

pub(crate) fn choose_token_file_dialog() {
    task(async move {
        let picked = pick_paths(
            nsh_core::file_dialog::OpenDialog::files().title("Choose a file with the API token"),
        )
        .await;
        match picked {
            Ok(Some(paths)) => {
                if let Some(path) = paths.into_iter().next() {
                    choose_token_file(path);
                }
            }
            Ok(None) => {}
            Err(error) => fail(error),
        }
    });
}

pub(crate) fn discard_tunnel() {
    let snapshot = untracked(|| workspace().with(|state| state.snapshot.clone()));
    load_tunnel_draft(&snapshot);
    clear_feedback();
}

/// `Save Cloudflare settings`: write the token file, account and client, then
/// discover metadata when the token is new or the account is still unknown.
pub(crate) fn save_cloudflare() {
    if !begin("tunnel") {
        return;
    }
    let (token_file, account, client) = untracked(|| {
        tunnel_draft().with(|state| {
            (
                state.token_file.clone(),
                state.account_id.clone(),
                state.client_id.clone(),
            )
        })
    });
    let supplied_token = token_file.is_some();
    let Some(provider) = provider_of() else {
        finish("tunnel");
        fail("no Cloudflare provider is available");
        return;
    };
    task(async move {
        let saved = fetch({
            let provider = Arc::clone(&provider);
            let token_file = token_file.clone();
            let account = account.clone();
            move || provider.save_cloudflare(token_file.as_deref(), &account, &client)
        })
        .await;
        let saved = match saved {
            Ok(saved) => saved,
            Err(error) => {
                finish("tunnel");
                fail(error);
                return;
            }
        };
        tunnel_draft().update(|state| {
            if state.token_file == token_file {
                state.token_file = None;
            }
            if state.account_id == account {
                state.account_id = saved.account_id.clone();
            }
        });
        let discover = supplied_token || (saved.token_configured && saved.account_id.is_empty());
        if discover {
            let answer = fetch({
                let provider = Arc::clone(&provider);
                move || provider.refresh_cloudflare()
            })
            .await;
            finish("tunnel");
            match answer {
                Ok(()) => succeed("Saved Cloudflare settings and refreshed zones."),
                Err(error) => succeed(format!(
                    "Saved Cloudflare settings (metadata check failed: {error})."
                )),
            }
        } else {
            finish("tunnel");
            succeed("Saved Cloudflare settings.");
        }
        refresh();
    });
}

pub(crate) fn refresh_cloudflare() {
    if !begin("tunnel") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("tunnel");
        fail("no Cloudflare provider is available");
        return;
    };
    task(async move {
        let answer = fetch(move || provider.refresh_cloudflare()).await;
        finish("tunnel");
        match answer {
            Ok(()) => succeed("Refreshed Cloudflare zones and DNS records."),
            Err(error) => fail(error),
        }
        refresh();
    });
}

pub(crate) fn deploy_workers() {
    if dirty() || tunnel_dirty() || !has_tunnel_rules() || !token_configured() || !begin("tunnel") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("tunnel");
        fail("no Cloudflare provider is available");
        return;
    };
    task(async move {
        let answer = fetch(move || provider.deploy_workers()).await;
        finish("tunnel");
        match answer {
            Ok(()) => succeed("Deployed Cloudflare Workers."),
            Err(error) => fail(error),
        }
        refresh();
    });
}

pub(crate) fn set_tunnel_enabled(on: bool) {
    if tunnel_dirty() || !begin("tunnel") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("tunnel");
        fail("no Cloudflare provider is available");
        return;
    };
    task(async move {
        let answer = fetch(move || {
            if on {
                provider.start_tunnel()
            } else {
                provider.stop_tunnel()
            }
        })
        .await;
        finish("tunnel");
        match answer {
            Ok(()) if on => succeed("Cloudflare tunnel started."),
            Ok(()) => succeed("Stopped Cloudflare tunnel"),
            Err(error) => fail(error),
        }
        refresh();
    });
}

pub(crate) fn restart_tunnel() {
    if !begin("tunnel") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("tunnel");
        fail("no Cloudflare provider is available");
        return;
    };
    task(async move {
        let answer = fetch(move || provider.restart_tunnel()).await;
        finish("tunnel");
        match answer {
            Ok(()) => succeed("Restarted Cloudflare tunnel."),
            Err(error) => fail(error),
        }
        refresh();
    });
}

/// Asks the Go question, then removes the token and Cloudflare metadata.
pub(crate) fn request_remove_cloudflare() {
    if !token_configured() {
        return;
    }
    let answer = alert_dialog()
        .title("Remove Cloudflare API token?")
        .description(
            "Stops the tunnel if running and clears saved Cloudflare account metadata from this Mac.",
        )
        .cancel("cancel", "Cancel")
        .destructive("confirm", "Remove Token")
        .show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if !chosen {
            return;
        }
        if !begin("tunnel") {
            return;
        }
        let Some(provider) = provider_of() else {
            finish("tunnel");
            fail("no Cloudflare provider is available");
            return;
        };
        let result = fetch(move || provider.remove_cloudflare(false)).await;
        finish("tunnel");
        match result {
            Ok(()) => succeed("Removed Cloudflare token and account metadata."),
            Err(error) => fail(error),
        }
        refresh();
    });
}

/// Copies the saved rule's access token without putting it in a signal.
pub(crate) fn copy_tunnel_token(domain: String) {
    if !begin("token") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("token");
        fail("no Cloudflare provider is available");
        return;
    };
    task(async move {
        let request = domain.clone();
        let answer = fetch(move || provider.ensure_tunnel_token(&request)).await;
        finish("token");
        match answer {
            Ok(token) => {
                set_clipboard(token);
                succeed(format!("Copied tunnel access token for {domain}"));
            }
            Err(error) => fail(error),
        }
    });
}

pub(crate) fn rotate_tunnel_token(domain: String) {
    if !begin("token") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("token");
        fail("no Cloudflare provider is available");
        return;
    };
    task(async move {
        let request = domain.clone();
        let answer = fetch(move || provider.rotate_tunnel_token(&request)).await;
        finish("token");
        match answer {
            Ok(()) => succeed(format!("Rotated tunnel access token for {domain}")),
            Err(error) => fail(error),
        }
        refresh();
    });
}

// --- Activation --------------------------------------------------------------

pub(crate) fn proxy_check(domain: &str) -> Option<ProxyHttpResult> {
    checks_state().with(|state| state.proxy_checks.get(domain).cloned())
}

pub(crate) fn tunnel_check(domain: &str) -> Option<ProxyHttpResult> {
    checks_state().with(|state| state.tunnel_checks.get(domain).cloned())
}

pub(crate) fn checking_proxy(domain: &str) -> bool {
    checks_state().with(|state| state.proxy_checking.contains(domain))
}

pub(crate) fn checking_tunnel(domain: &str) -> bool {
    checks_state().with(|state| state.tunnel_checking.contains(domain))
}

/// The Go `Current` test: same proxy revision and at most 30 seconds old.
pub(crate) fn check_is_current(result: &ProxyHttpResult) -> bool {
    workspace()
        .with(|state| result.is_current(Utc::now(), state.snapshot.revision, &result.fingerprint))
}

/// Seconds since the observation, rounded down.
pub(crate) fn check_age(result: &ProxyHttpResult) -> i64 {
    (Utc::now() - result.checked_at).num_seconds().max(0)
}

pub(crate) fn check_proxy(domain: String) {
    if untracked(dirty) {
        return;
    }
    let Some(generation) = untracked(|| {
        checks_state().with(|state| {
            if state.proxy_checking.contains(&domain) {
                return None;
            }
            Some(state.proxy_generation + 1)
        })
    }) else {
        return;
    };
    let Some(provider) = provider_of() else {
        return;
    };
    checks_state().update(|state| {
        state.proxy_generation = generation;
        state.proxy_checking.insert(domain.clone());
        state.proxy_checks.remove(&domain);
    });
    if untracked(|| notices_state().with(|state| state.action_error.is_some())) {
        notices_state().update(|state| state.action_error = None);
    }
    task(async move {
        let request = domain.clone();
        let answer = fetch(move || provider.proxy_result(&request)).await;
        let mut error_out = None;
        checks_state().update(|state| {
            if state.proxy_generation != generation {
                return;
            }
            state.proxy_checking.remove(&domain);
            match answer {
                Ok(result) => {
                    state.proxy_checks.insert(domain, result);
                }
                Err(error) => error_out = Some(error),
            }
        });
        if let Some(error) = error_out {
            fail(error);
        }
    });
}

pub(crate) fn check_tunnel(domain: String) {
    if untracked(dirty) {
        return;
    }
    let Some(generation) = untracked(|| {
        checks_state().with(|state| {
            if state.tunnel_checking.contains(&domain) {
                return None;
            }
            Some(state.tunnel_generation + 1)
        })
    }) else {
        return;
    };
    let Some(provider) = provider_of() else {
        return;
    };
    checks_state().update(|state| {
        state.tunnel_generation = generation;
        state.tunnel_checking.insert(domain.clone());
        state.tunnel_checks.remove(&domain);
    });
    if untracked(|| notices_state().with(|state| state.action_error.is_some())) {
        notices_state().update(|state| state.action_error = None);
    }
    task(async move {
        let request = domain.clone();
        let answer = fetch(move || provider.tunnel_result(&request)).await;
        let mut error_out = None;
        checks_state().update(|state| {
            if state.tunnel_generation != generation {
                return;
            }
            state.tunnel_checking.remove(&domain);
            match answer {
                Ok(result) => {
                    state.tunnel_checks.insert(domain, result);
                }
                Err(error) => error_out = Some(error),
            }
        });
        if let Some(error) = error_out {
            fail(error);
        }
    });
}

/// The live Worker state of the saved tunneled rule, when the tunnel runs.
pub(crate) fn live_tunnel(domain: &str) -> Option<String> {
    workspace().with(|state| {
        let rule = state
            .snapshot
            .rules
            .iter()
            .find(|rule| rule.domain == domain.trim().to_lowercase())?;
        if !rule.tunnel {
            return None;
        }
        if status_of(state, "tunnel").as_deref() != Some("Running") {
            return None;
        }
        let key = format!("tunnel:worker:{}", rule.resolved_tunnel_worker());
        status_of(state, &key)
    })
}

/// Whether the saved rule already has a public URL to check.
pub(crate) fn tunnel_url_available(domain: &str) -> bool {
    workspace().with(|state| {
        let Some(rule) = state
            .snapshot
            .rules
            .iter()
            .find(|rule| rule.domain == domain.trim().to_lowercase())
        else {
            return false;
        };
        if !rule.tunnel {
            return false;
        }
        if state
            .snapshot
            .workers
            .public_url(&rule.resolved_tunnel_worker())
            .is_some_and(|url| !url.is_empty())
        {
            return true;
        }
        !preview_tunnel_url(rule, &state.snapshot.subdomain).is_empty()
    })
}

// --- Authority ---------------------------------------------------------------

pub(crate) fn authority_open() -> bool {
    authority_state().with(|state| state.open)
}

pub(crate) fn open_authority() {
    authority_state().update(|state| state.open = true);
    clear_feedback();
    refresh_authority();
}

pub(crate) fn close_authority() {
    authority_state().update(|state| state.open = false);
    clear_feedback();
}

pub(crate) fn authority() -> Option<AuthorityStatus> {
    authority_state().with(|state| state.authority.clone())
}

fn store_authority(status: AuthorityStatus) {
    authority_state().update(|state| state.authority = Some(status));
}

pub(crate) fn refresh_authority() {
    let Some(provider) = provider_of() else {
        return;
    };
    let sequence = untracked(|| workspace().with(|state| state.sequence));
    task(async move {
        let answer = fetch(move || provider.authority_status()).await;
        if untracked(|| workspace().with(|state| state.sequence)) != sequence {
            return;
        }
        if let Ok(status) = answer {
            store_authority(status);
        }
    });
}

fn authority_action(action: AuthorityAction) {
    if !begin("authority") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("authority");
        fail("no certificate provider is available");
        return;
    };
    task(async move {
        let result = fetch({
            let action = action.clone();
            let provider = Arc::clone(&provider);
            move || provider.authority_action(action)
        })
        .await;
        let status = fetch(move || provider.authority_status()).await;
        finish("authority");
        if let Ok(status) = status {
            store_authority(status);
        }
        match result {
            Ok(()) => succeed("Local certificate status refreshed."),
            Err(error) => fail(format!(
                "Local certificate action stopped. Review the current status.\nError: {error}"
            )),
        }
    });
}

pub(crate) fn prepare_authority() {
    authority_action(AuthorityAction::Prepare);
}

pub(crate) fn trust_active() {
    authority_action(AuthorityAction::TrustActive);
}

pub(crate) fn remove_active_trust() {
    authority_action(AuthorityAction::RemoveActiveTrust);
}

pub(crate) fn trust_prepared() {
    let Some(status) = authority() else {
        return;
    };
    authority_action(AuthorityAction::TrustPrepared {
        revision: status.revision.clone(),
    });
}

pub(crate) fn activate_authority() {
    let Some(status) = authority() else {
        return;
    };
    authority_action(AuthorityAction::Activate {
        revision: status.revision.clone(),
    });
}

pub(crate) fn discard_prepared() {
    let Some(status) = authority() else {
        return;
    };
    authority_action(AuthorityAction::DiscardPrepared {
        revision: status.revision.clone(),
    });
}

pub(crate) fn restore_authority(id: String, fingerprint: String) {
    let Some(status) = authority() else {
        return;
    };
    authority_action(AuthorityAction::Restore {
        id,
        fingerprint,
        revision: status.revision,
    });
}

pub(crate) fn trust_saved(id: String, fingerprint: String) {
    authority_action(AuthorityAction::TrustSaved { id, fingerprint });
}

pub(crate) fn remove_saved_trust(id: String, fingerprint: String) {
    authority_action(AuthorityAction::RemoveSavedTrust { id, fingerprint });
}

pub(crate) fn authority_apply_proxy() {
    if !begin("authority") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("authority");
        fail("no HTTPS proxy provider is available");
        return;
    };
    task(async move {
        let result = fetch({
            let provider = Arc::clone(&provider);
            move || provider.apply_proxy()
        })
        .await;
        let status = fetch(move || provider.authority_status()).await;
        finish("authority");
        if let Ok(status) = status {
            store_authority(status);
        }
        match result {
            Ok(()) => succeed("Local certificate status refreshed."),
            Err(error) => fail(format!(
                "Local certificate action stopped. Review the current status.\nError: {error}"
            )),
        }
        refresh();
    });
}

pub(crate) fn authority_stop_proxy() {
    if !begin("authority") {
        return;
    }
    let Some(provider) = provider_of() else {
        finish("authority");
        fail("no HTTPS proxy provider is available");
        return;
    };
    task(async move {
        let result = fetch({
            let provider = Arc::clone(&provider);
            move || provider.stop_proxy()
        })
        .await;
        let status = fetch(move || provider.authority_status()).await;
        finish("authority");
        if let Ok(status) = status {
            store_authority(status);
        }
        match result {
            Ok(()) => succeed("Local certificate status refreshed."),
            Err(error) => fail(format!(
                "Local certificate action stopped. Review the current status.\nError: {error}"
            )),
        }
        refresh();
    });
}

// --- Formatting --------------------------------------------------------------

/// `n` and its noun, singular when one.
pub(crate) fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The Go `authorityFingerprint`: a 64-character SHA-256 split in two lines.
pub(crate) fn split_fingerprint(value: &str) -> String {
    if value.len() == 64 {
        format!("{}\n{}", &value[..32], &value[32..])
    } else {
        value.to_string()
    }
}

/// `YYYY-MM-DD → YYYY-MM-DD`, as the authority card writes validity.
pub(crate) fn validity(info: &AuthorityInfo) -> String {
    let start = chrono::DateTime::from_timestamp(info.not_before, 0)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default();
    let end = chrono::DateTime::from_timestamp(info.not_after, 0)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default();
    format!("{start} → {end}")
}

/// The short id an archive card titles itself with.
pub(crate) fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}
