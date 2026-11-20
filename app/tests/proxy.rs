//! The HTTPS proxy workspace, headless: the saved rule table, the draft's
//! validation, the proxy switch's busy and error states, the Cloudflare token
//! file flow, the certificate action sequence and the activation panel. A fake
//! provider stands in for devproxy: no test starts a launchd job, touches the
//! keychain, reaches Cloudflare or opens a socket.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use chrono::Utc;
use devcore::model::{ProxyRule, normalize_proxy_rules};
use nsh_app_devbase::testing::{
    AuthorityAction, AuthorityInfo, AuthorityStatus, ProxyHttpResult, ProxyListeningPort,
    ProxyProvider, ProxyRuleDraft, ProxySavedCloudflare, ProxySnapshot, ProxyWorkers, TrustState,
    install_proxy_provider, use_fake_proxy,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

/// The proxy tests share one fake-provider registry, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fingerprint(seed: char) -> String {
    std::iter::repeat_n(seed, 64).collect()
}

fn info(id: &str, status: &str, trust: TrustState, usable: bool, seed: char) -> AuthorityInfo {
    AuthorityInfo {
        id: id.to_string(),
        name: "Devbase Local Development CA".to_string(),
        status: status.to_string(),
        detail: String::new(),
        fingerprint: if id.is_empty() {
            String::new()
        } else {
            fingerprint(seed)
        },
        trust,
        certificate_path: PathBuf::from(format!("/tmp/devbase-ca/{id}.pem")),
        not_before: 1_700_000_000,
        not_after: 1_800_000_000,
        usable,
    }
}

fn not_created() -> AuthorityStatus {
    AuthorityStatus {
        checked_at: Utc::now(),
        active: info("", "Not created", TrustState::NotChecked, false, 'a'),
        prepared: None,
        previous: None,
        archives: Vec::new(),
        prepared_is_active: false,
        revision: "rev-1".to_string(),
    }
}

fn rule(domain: &str, port: i64, tunnel: bool) -> ProxyRule {
    ProxyRule {
        domain: domain.to_string(),
        local_port: port,
        upstream_host: String::new(),
        tunnel,
        tunnel_domain: if tunnel {
            "www.example.com".to_string()
        } else {
            String::new()
        },
        tunnel_url: if tunnel {
            "https://www.example.com".to_string()
        } else {
            String::new()
        },
        ..ProxyRule::default()
    }
}

fn snapshot(rules: Vec<ProxyRule>, listener_port: i64) -> ProxySnapshot {
    let mut statuses = BTreeMap::new();
    if !rules.is_empty() {
        statuses.insert("proxy".to_string(), "Stopped".to_string());
    }
    ProxySnapshot {
        rules,
        listener_port,
        revision: 7,
        publication: "Published".to_string(),
        statuses,
        ..ProxySnapshot::default()
    }
}

struct Fake {
    snapshot: Mutex<ProxySnapshot>,
    authority: Mutex<AuthorityStatus>,
    calls: Mutex<Vec<String>>,
    apply_delay_ms: u64,
    apply_error: Mutex<Option<String>>,
    proxy_failure: Option<String>,
    tunnel_failure: Option<String>,
    token_seen: Mutex<Option<String>>,
}

impl Fake {
    fn new(snapshot: ProxySnapshot) -> Arc<Self> {
        Arc::new(Fake {
            snapshot: Mutex::new(snapshot),
            authority: Mutex::new(not_created()),
            calls: Mutex::new(Vec::new()),
            apply_delay_ms: 0,
            apply_error: Mutex::new(None),
            proxy_failure: None,
            tunnel_failure: None,
            token_seen: Mutex::new(None),
        })
    }

    fn slow(mut self: Arc<Self>, millis: u64) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("a unique fake")
            .apply_delay_ms = millis;
        self
    }

    fn with_proxy_failure(mut self: Arc<Self>, failure: &str) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("a unique fake")
            .proxy_failure = Some(failure.to_string());
        self
    }

    fn with_tunnel_failure(mut self: Arc<Self>, failure: &str) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("a unique fake")
            .tunnel_failure = Some(failure.to_string());
        self
    }

    fn with_authority(self: Arc<Self>, status: AuthorityStatus) -> Arc<Self> {
        *self
            .authority
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = status;
        self
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn record(&self, call: String) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(call);
    }

    fn snapshot_now(&self) -> ProxySnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn with_snapshot(&self, change: impl FnOnce(&mut ProxySnapshot)) {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        change(&mut snapshot);
    }

    fn with_authority_now(&self, change: impl FnOnce(&mut AuthorityStatus)) {
        let mut status = self
            .authority
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        change(&mut status);
    }

    fn token_seen(&self) -> Option<String> {
        self.token_seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ProxyProvider for Fake {
    fn snapshot(&self) -> Result<ProxySnapshot, String> {
        Ok(self.snapshot_now())
    }

    fn listening_ports(&self) -> Result<Vec<ProxyListeningPort>, String> {
        Ok(vec![
            ProxyListeningPort {
                port: 3000,
                process: "node".to_string(),
            },
            ProxyListeningPort {
                port: 5173,
                process: "vite".to_string(),
            },
        ])
    }

    fn save_rules(
        &self,
        expected_revision: u64,
        listener_port: i64,
        rules: Vec<ProxyRule>,
    ) -> Result<(), String> {
        self.record(format!("save {expected_revision} {listener_port}"));
        let normalized =
            normalize_proxy_rules(listener_port, &rules).map_err(|error| error.to_string())?;
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if snapshot.revision != expected_revision {
            return Err("proxy configuration changed; reload before saving".to_string());
        }
        snapshot.rules = normalized;
        snapshot.listener_port = listener_port;
        snapshot.revision += 1;
        snapshot.publication = "Pending".to_string();
        Ok(())
    }

    fn apply_proxy(&self) -> Result<(), String> {
        self.record("apply".to_string());
        if self.apply_delay_ms > 0 {
            thread::sleep(Duration::from_millis(self.apply_delay_ms));
        }
        if let Some(error) = self
            .apply_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return Err(error);
        }
        self.with_snapshot(|snapshot| {
            snapshot
                .statuses
                .insert("proxy".to_string(), "Running".to_string());
            snapshot.publication = "Published".to_string();
        });
        Ok(())
    }

    fn stop_proxy(&self) -> Result<(), String> {
        self.record("stop proxy".to_string());
        self.with_snapshot(|snapshot| {
            snapshot
                .statuses
                .insert("proxy".to_string(), "Stopped".to_string());
        });
        Ok(())
    }

    fn restart_proxy(&self) -> Result<(), String> {
        self.record("restart proxy".to_string());
        Ok(())
    }

    fn proxy_result(&self, domain: &str) -> Result<ProxyHttpResult, String> {
        self.record(format!("check proxy {domain}"));
        Ok(ProxyHttpResult {
            domain: domain.to_string(),
            fingerprint: "fp".to_string(),
            revision: self.snapshot_now().revision,
            status_code: 200,
            duration: Duration::from_millis(12),
            checked_at: Utc::now(),
            failure: self.proxy_failure.clone(),
        })
    }

    fn tunnel_result(&self, domain: &str) -> Result<ProxyHttpResult, String> {
        self.record(format!("check tunnel {domain}"));
        Ok(ProxyHttpResult {
            domain: domain.to_string(),
            fingerprint: "tf".to_string(),
            revision: self.snapshot_now().revision,
            status_code: 200,
            duration: Duration::from_millis(20),
            checked_at: Utc::now(),
            failure: self.tunnel_failure.clone(),
        })
    }

    fn authority_status(&self) -> Result<AuthorityStatus, String> {
        Ok(self
            .authority
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn authority_action(&self, action: AuthorityAction) -> Result<(), String> {
        self.record(action.label());
        self.with_authority_now(|status| match action {
            AuthorityAction::Prepare => {
                let prepared = info("prepared-1", "Ready", TrustState::NotTrusted, true, 'b');
                status.prepared = Some(prepared);
                status.revision = "rev-2".to_string();
            }
            AuthorityAction::TrustActive => status.active.trust = TrustState::Trusted,
            AuthorityAction::RemoveActiveTrust => status.active.trust = TrustState::NotTrusted,
            AuthorityAction::TrustPrepared { revision } => {
                status.revision = revision;
                if let Some(prepared) = status.prepared.as_mut() {
                    prepared.trust = TrustState::Trusted;
                }
            }
            AuthorityAction::Activate { revision } => {
                status.revision = revision;
                if let Some(prepared) = status.prepared.take() {
                    status.previous = Some(status.active.clone());
                    status.active = prepared;
                }
            }
            AuthorityAction::DiscardPrepared { revision } => {
                status.revision = revision;
                status.prepared = None;
            }
            AuthorityAction::Restore { id, .. } => {
                if let Some(index) = status.archives.iter().position(|item| item.id == id) {
                    let restored = status.archives.remove(index);
                    status.previous = Some(status.active.clone());
                    status.active = restored;
                }
            }
            AuthorityAction::TrustSaved { id, .. } => {
                if let Some(saved) = status.archives.iter_mut().find(|item| item.id == id) {
                    saved.trust = TrustState::Trusted;
                }
            }
            AuthorityAction::RemoveSavedTrust { id, .. } => {
                if let Some(saved) = status.archives.iter_mut().find(|item| item.id == id) {
                    saved.trust = TrustState::NotTrusted;
                }
            }
        });
        Ok(())
    }

    fn save_cloudflare(
        &self,
        token_path: Option<&std::path::Path>,
        account_id: &str,
        client_id: &str,
    ) -> Result<ProxySavedCloudflare, String> {
        match token_path {
            Some(path) => {
                self.record(format!("token file {}", path.display()));
                let token = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
                *self
                    .token_seen
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(token);
            }
            None => self.record("token kept".to_string()),
        }
        self.record(format!("save cloudflare {account_id} {client_id}"));
        let account = if account_id.trim().is_empty() {
            "account-123".to_string()
        } else {
            account_id.trim().to_string()
        };
        let client = if client_id.trim().is_empty() {
            "mac-1234".to_string()
        } else {
            client_id.trim().to_string()
        };
        self.with_snapshot(|snapshot| {
            snapshot.token_configured = true;
            snapshot.account_id = account.clone();
            snapshot.client_id = client.clone();
        });
        Ok(ProxySavedCloudflare {
            token_configured: true,
            account_id: account,
        })
    }

    fn refresh_cloudflare(&self) -> Result<(), String> {
        self.record("refresh cloudflare".to_string());
        self.with_snapshot(|snapshot| {
            snapshot.subdomain = "devbase".to_string();
            snapshot.zones = vec!["example.com".to_string()];
            snapshot.dns_names = vec!["www.example.com".to_string(), "api.example.com".to_string()];
        });
        Ok(())
    }

    fn deploy_workers(&self) -> Result<(), String> {
        self.record("deploy workers".to_string());
        self.with_snapshot(|snapshot| {
            let mut workers = BTreeMap::new();
            workers.insert(
                "devbase-app".to_string(),
                "https://devbase-app.devbase.workers.dev".to_string(),
            );
            snapshot.workers = ProxyWorkers::new(workers);
        });
        Ok(())
    }

    fn start_tunnel(&self) -> Result<(), String> {
        self.record("start tunnel".to_string());
        self.with_snapshot(|snapshot| {
            snapshot
                .statuses
                .insert("tunnel".to_string(), "Running".to_string());
        });
        Ok(())
    }

    fn stop_tunnel(&self) -> Result<(), String> {
        self.record("stop tunnel".to_string());
        self.with_snapshot(|snapshot| {
            snapshot
                .statuses
                .insert("tunnel".to_string(), "Stopped".to_string());
        });
        Ok(())
    }

    fn restart_tunnel(&self) -> Result<(), String> {
        self.record("restart tunnel".to_string());
        Ok(())
    }

    fn remove_cloudflare(&self, delete_workers: bool) -> Result<(), String> {
        self.record(format!("remove cloudflare {delete_workers}"));
        self.with_snapshot(|snapshot| {
            snapshot.token_configured = false;
            snapshot.account_id.clear();
            snapshot.subdomain.clear();
            snapshot.zones.clear();
            snapshot.workers = ProxyWorkers::default();
        });
        Ok(())
    }

    fn ensure_tunnel_token(&self, domain: &str) -> Result<String, String> {
        self.record(format!("ensure token {domain}"));
        Ok("secret-access-token".to_string())
    }

    fn rotate_tunnel_token(&self, domain: &str) -> Result<(), String> {
        self.record(format!("rotate token {domain}"));
        Ok(())
    }

    fn logs(&self, job: &str) -> Result<String, String> {
        self.record(format!("logs {job}"));
        Ok(format!("{job}.log:\nstarted"))
    }
}

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

fn token_file(contents: &str) -> PathBuf {
    let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "devbase-proxy-test-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("cloudflare.key");
    std::fs::write(&path, contents).unwrap();
    path
}

/// Installs the fake provider and starts the surface on the HTTPS Proxy page,
/// holding the tests' turn for as long as the returned guard lives.
fn start(fake: Arc<Fake>) -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    use_fake_proxy();
    install_proxy_provider(fake as Arc<dyn ProxyProvider>);
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    app.press("role:button[HTTPS Proxy]").unwrap();
    app.wait_for("id:proxy-rules").unwrap();
    (turn, app)
}

#[test]
fn the_saved_rules_render_from_the_provider() {
    let fake = Fake::new(snapshot(
        vec![
            rule("app.devbase.localhost", 3000, false),
            rule("api.devbase.localhost", 4000, true),
        ],
        443,
    ));
    let (_turn, mut app) = start(fake);

    app.wait_for("id:proxy-rule-0").unwrap();
    app.wait_for("id:proxy-rule-1").unwrap();
    assert!(app.screen().shows("2 rules · listener :443"));
    assert!(app.screen().shows("Published"));
    assert_eq!(
        app.screen().input_text("id:proxy-domain-0").unwrap(),
        Some("app")
    );
    assert_eq!(
        app.screen().input_text("id:proxy-port-0").unwrap(),
        Some("3000")
    );
    assert!(app.screen().shows("app.devbase.localhost"));
    assert!(app.screen().shows("api.devbase.localhost"));
    // The tunnel sub-row is only there for the tunneled rule.
    assert!(app.screen().matches("id:proxy-mode-workers-1").unwrap());
    assert_eq!(
        app.screen().input_text("id:proxy-tunnel-domain-1").unwrap(),
        Some("www.example.com")
    );
    assert!(!app.screen().matches("id:proxy-mode-workers-0").unwrap());
    // The saved table is published, so Apply is offered and enabled.
    assert!(app.screen().enabled("id:proxy-apply").unwrap());
}

#[test]
fn saving_a_rule_surfaces_the_port_and_domain_validation() {
    let fake = Fake::new(snapshot(Vec::new(), 443));
    let (_turn, mut app) = start(fake.clone());

    app.press("id:proxy-add-rule").unwrap();
    app.wait_for("id:proxy-rule-0").unwrap();
    app.type_text("id:proxy-domain-0", "app").unwrap();
    app.type_text("id:proxy-port-0", "not-a-port").unwrap();
    app.press("id:proxy-save").unwrap();
    app.wait_for("Enter valid numeric ports").unwrap();
    assert!(
        !fake.calls().iter().any(|call| call.starts_with("save")),
        "an unparsable port must not reach the provider"
    );

    app.clear("id:proxy-port-0").unwrap();
    app.type_text("id:proxy-port-0", "3000").unwrap();
    app.press("id:proxy-save").unwrap();
    app.wait_for("Saved HTTPS rules.").unwrap();
    assert!(fake.calls().iter().any(|call| call == "save 7 443"));
    let saved = fake.snapshot_now();
    assert_eq!(saved.rules.len(), 1);
    assert_eq!(saved.rules[0].domain, "app.devbase.localhost");

    // A duplicate domain reaches the validator and is shown verbatim.
    app.press("id:proxy-add-rule").unwrap();
    app.wait_for("id:proxy-rule-1").unwrap();
    app.type_text("id:proxy-domain-1", "app").unwrap();
    app.clear("id:proxy-port-1").unwrap();
    app.type_text("id:proxy-port-1", "3001").unwrap();
    app.press("id:proxy-save").unwrap();
    app.wait_for("duplicate proxy domain app.devbase.localhost")
        .unwrap();
    assert_eq!(fake.snapshot_now().rules.len(), 1);

    // Removing the second draft leaves one row.
    app.press("id:proxy-remove-1").unwrap();
    app.wait_for_gone("id:proxy-rule-1", Duration::from_secs(2))
        .unwrap();
}

#[test]
fn the_proxy_switch_is_busy_and_surfaces_the_apply_error_verbatim() {
    let fake = Fake::new(snapshot(
        vec![rule("app.devbase.localhost", 3000, false)],
        443,
    ))
    .slow(400);
    *fake
        .apply_error
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some("launchctl bootstrap failed: Input/output error".to_string());
    let (_turn, mut app) = start(fake.clone());

    app.press("id:proxy-switch").unwrap();
    // The operation is in flight: every control waits on it.
    assert!(!app.screen().enabled("id:proxy-switch").unwrap());
    // A second press while busy is refused, not queued; a disabled control
    // takes no input at all.
    assert!(app.press("id:proxy-switch").is_err());
    app.wait_for("launchctl bootstrap failed: Input/output error")
        .unwrap();
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|call| call.as_str() == "apply")
            .count(),
        1
    );
    assert!(app.screen().enabled("id:proxy-switch").unwrap());

    *fake
        .apply_error
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    app.wait(Duration::from_millis(100)).unwrap();
    assert!(app.screen().enabled("id:proxy-switch").unwrap());
    app.press("id:proxy-switch").unwrap();
    assert!(fake.calls().iter().any(|call| call == "apply"));
    app.wait_for("Proxy configuration published. Check DNS, TLS and upstream separately.")
        .unwrap();
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|call| call.as_str() == "apply")
            .count(),
        2
    );
}

#[test]
fn the_cloudflare_token_comes_from_a_file_and_is_never_shown() {
    let mut snapshot = snapshot(vec![rule("app.devbase.localhost", 3000, true)], 443);
    snapshot
        .statuses
        .insert("tunnel".to_string(), "Stopped".to_string());
    let fake = Fake::new(snapshot);
    let token = "cf-token-never-in-a-signal-9c1f";
    let path = token_file(token);
    let (_turn, mut app) = start(fake.clone());
    app.on_pick_paths(vec![path.clone()]).unwrap();

    app.press("id:tunnel-token-file").unwrap();
    app.wait(Duration::from_millis(250)).unwrap();
    app.press("id:tunnel-save").unwrap();
    app.wait_for("Saved Cloudflare settings and refreshed zones.")
        .unwrap();
    app.wait_for("Configured").unwrap();
    app.wait_for("text:/\\*\\.devbase\\.workers\\.dev/")
        .unwrap();

    assert_eq!(fake.token_seen().as_deref(), Some(token));
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == &format!("token file {}", path.display()))
    );
    // The token text is nowhere on screen, and there is no field that could
    // hold it: only the file picker fed the provider.
    assert!(!app.screen().shows(token));
    assert!(!app.screen().matches("id:tunnel-token").unwrap());
}

#[test]
fn preparing_trusting_and_activating_walks_the_ca_sequence() {
    let fake = Fake::new(snapshot(Vec::new(), 443));
    let (_turn, mut app) = start(fake.clone());

    app.press("id:ca-open").unwrap();
    app.wait_for("Local certificates").unwrap();
    app.wait_for("id:ca-active").unwrap();
    assert!(app.screen().enabled("id:ca-active-trust").unwrap());
    assert!(!app.screen().matches("id:ca-prepared").unwrap());

    app.press("id:ca-prepare").unwrap();
    app.wait_for("id:ca-prepared").unwrap();
    assert!(fake.calls().iter().any(|call| call == "prepare"));
    // A prepared CA must be trusted before it can be activated.
    assert!(!app.screen().enabled("id:ca-prepared-activate").unwrap());
    assert!(app.screen().enabled("id:ca-prepared-trust").unwrap());

    app.press("id:ca-prepared-trust").unwrap();
    app.wait_for("Local certificate status refreshed.").unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "trust prepared rev-2")
    );
    assert!(app.screen().enabled("id:ca-prepared-activate").unwrap());

    app.press("id:ca-prepared-activate").unwrap();
    app.wait_for_gone("id:ca-prepared", Duration::from_secs(3))
        .unwrap();
    assert!(fake.calls().iter().any(|call| call == "activate rev-2"));
    // The prepared generation is now the active one.
    app.wait_for("text:/b{32}/").unwrap();
}

#[test]
fn a_prepared_replacement_gates_restoring_an_archive() {
    let mut status = not_created();
    status.active = info("active-1", "Ready", TrustState::Trusted, true, 'a');
    status.prepared = Some(info(
        "prepared-1",
        "Ready",
        TrustState::NotTrusted,
        true,
        'b',
    ));
    status.archives = vec![info("saved-1", "Ready", TrustState::NotTrusted, true, 'c')];
    status.revision = "rev-9".to_string();
    let fake = Fake::new(snapshot(Vec::new(), 443)).with_authority(status);
    let (_turn, mut app) = start(fake);

    app.press("id:ca-open").unwrap();
    app.wait_for("id:ca-archive-saved-1").unwrap();
    // Restoring would replace the prepared candidate, so it waits.
    assert!(!app.screen().enabled("id:ca-archive-use-saved-1").unwrap());
    assert!(app.screen().enabled("id:ca-archive-trust-saved-1").unwrap());
    assert!(app.screen().enabled("id:ca-prepared-discard").unwrap());
}

#[test]
fn removing_the_cloudflare_token_asks_first() {
    let mut snapshot = snapshot(Vec::new(), 443);
    snapshot.token_configured = true;
    snapshot.account_id = "account-123".to_string();
    let fake = Fake::new(snapshot);
    let (_turn, mut app) = start(fake.clone());

    // The default answer is Cancel.
    app.press("id:tunnel-remove-token").unwrap();
    app.wait(Duration::from_millis(600)).unwrap();
    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| call.starts_with("remove cloudflare")),
        "a cancelled confirmation must not remove the token"
    );

    app.on_dialog(|_| DialogAnswer::Press("Remove Token".to_string()));
    app.press("id:tunnel-remove-token").unwrap();
    app.wait_for("Removed Cloudflare token and account metadata.")
        .unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "remove cloudflare false")
    );
    app.wait_for("Not configured").unwrap();
}

#[test]
fn the_activation_panel_shows_the_https_probe() {
    let fake = Fake::new(snapshot(
        vec![rule("app.devbase.localhost", 3000, false)],
        443,
    ));
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:proxy-activation-app-devbase-localhost")
        .unwrap();

    app.press("id:proxy-check-https-app-devbase-localhost")
        .unwrap();
    app.wait_for("text:/HTTP 200/").unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "check proxy app.devbase.localhost")
    );
}

#[test]
fn the_activation_panel_shows_a_failed_https_probe() {
    let fake = Fake::new(snapshot(
        vec![rule("app.devbase.localhost", 3000, false)],
        443,
    ))
    .with_proxy_failure("TLS");
    let (_turn, mut app) = start(fake);
    app.wait_for("id:proxy-activation-app-devbase-localhost")
        .unwrap();
    app.press("id:proxy-check-https-app-devbase-localhost")
        .unwrap();
    app.wait_for("HTTP failed").unwrap();
    app.wait_for("HTTP · TLS").unwrap();
}

#[test]
fn the_activation_panel_checks_a_deployed_tunnel() {
    let mut snapshot = snapshot(vec![rule("app.devbase.localhost", 3000, true)], 443);
    let mut workers = BTreeMap::new();
    workers.insert(
        "devbase-app".to_string(),
        "https://devbase-app.devbase.workers.dev".to_string(),
    );
    snapshot.workers = ProxyWorkers::new(workers);
    snapshot
        .statuses
        .insert("tunnel".to_string(), "Running".to_string());
    snapshot.statuses.insert(
        "tunnel:worker:devbase-app".to_string(),
        "Connected".to_string(),
    );
    let fake = Fake::new(snapshot).with_tunnel_failure("Unavailable");
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:proxy-activation-app-devbase-localhost")
        .unwrap();
    app.wait_for("Tunnel connected").unwrap();

    app.press("id:proxy-check-tunnel-app-devbase-localhost")
        .unwrap();
    app.wait_for("Tunnel failed").unwrap();
    app.wait_for("Tunnel · Unavailable").unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "check tunnel app.devbase.localhost")
    );
}

#[test]
fn a_rule_can_be_edited_and_its_changes_saved_with_the_listener_port() {
    let fake = Fake::new(snapshot(
        vec![rule("app.devbase.localhost", 3000, false)],
        443,
    ));
    let (_turn, mut app) = start(fake.clone());

    app.clear("id:proxy-domain-0").unwrap();
    app.type_text("id:proxy-domain-0", "shop").unwrap();
    app.clear("id:proxy-port-0").unwrap();
    app.type_text("id:proxy-port-0", "5173").unwrap();
    app.clear("id:proxy-listener-port").unwrap();
    app.type_text("id:proxy-listener-port", "8443").unwrap();
    app.wait_for("Unsaved changes").unwrap();

    app.press("id:proxy-save-apply").unwrap();
    app.wait_for("Saved and applied proxy configuration.")
        .unwrap();
    let saved = fake.snapshot_now();
    assert_eq!(saved.listener_port, 8443);
    assert_eq!(saved.rules[0].domain, "shop.devbase.localhost");
    assert_eq!(saved.rules[0].local_port, 5173);
    assert!(fake.calls().iter().any(|call| call == "apply"));
}

#[test]
fn a_stale_save_revision_is_shown_verbatim() {
    let fake = Fake::new(snapshot(
        vec![rule("app.devbase.localhost", 3000, false)],
        443,
    ));
    let (_turn, mut app) = start(fake.clone());
    // Another writer saves first; the draft still carries revision 7.
    fake.with_snapshot(|snapshot| snapshot.revision = 8);

    app.clear("id:proxy-port-0").unwrap();
    app.type_text("id:proxy-port-0", "3001").unwrap();
    app.press("id:proxy-save").unwrap();
    app.wait_for("proxy configuration changed; reload before saving")
        .unwrap();
}

#[test]
fn the_local_ca_trust_action_has_no_extra_app_confirmation() {
    let fake = Fake::new(snapshot(Vec::new(), 443));
    let (_turn, mut app) = start(fake.clone());
    app.press("id:ca-open").unwrap();
    app.wait_for("id:ca-active").unwrap();

    app.press("id:ca-active-trust").unwrap();
    app.wait_for("Local certificate status refreshed.").unwrap();
    // The Go view asks macOS for authorization inside the keychain call; it
    // shows no app dialog, and neither does this screen.
    assert!(app.pending_dialog().is_none());
    assert!(fake.calls().iter().any(|call| call == "trust active"));
}

#[test]
fn the_rule_draft_helpers_match_the_go_derivations() {
    // Keeps the pure draft shape pinned without a surface.
    let draft = ProxyRuleDraft::new();
    assert_eq!(draft.local_port, "3000");
    assert_eq!(draft.tunnel_mode, "workers.dev (Auto)");
    assert_eq!(draft.effective_tunnel_domain(), "");
    let mut custom = draft.clone();
    custom.tunnel_mode = "Custom domain".to_string();
    custom.tunnel_domain = "  WWW.Example.com ".to_string();
    assert_eq!(custom.effective_tunnel_domain(), "www.example.com");
}
