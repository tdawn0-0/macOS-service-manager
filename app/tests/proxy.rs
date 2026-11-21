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
    AuthorityAction, AuthorityInfo, AuthorityStatus, DevLogsKnownFile, DevLogsProvider,
    DevLogsRead, DevLogsSnapshot, DevLogsStream, ProxyHttpResult, ProxyListeningPort,
    ProxyProvider, ProxyRuleDraft, ProxySavedCloudflare, ProxySnapshot, ProxyWorkers, TrustState,
    install_dev_logs_provider, install_proxy_provider, use_fake_databases, use_fake_dev_logs,
    use_fake_development, use_fake_guide, use_fake_menubar, use_fake_providers, use_fake_proxy,
    use_fake_software, use_fake_system, use_fake_updater,
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
            diagnosis: None,
            hint: None,
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
            diagnosis: None,
            hint: None,
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
    use_fake_menubar();
    use_fake_providers();
    use_fake_software();
    use_fake_development();
    use_fake_databases();
    use_fake_system();
    use_fake_guide();
    use_fake_updater();
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
    assert!(app.screen().shows("2 rules · :443"));
    assert!(app.screen().shows("Published"));
    assert_eq!(
        app.screen().input_text("id:proxy-domain-0").unwrap(),
        Some("app")
    );
    assert_eq!(
        app.screen().input_text("id:proxy-domain-1").unwrap(),
        Some("api")
    );
    assert_eq!(
        app.screen().input_text("id:proxy-port-0").unwrap(),
        Some("3000")
    );
    assert!(app.screen().shows(".devbase.localhost"));
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

#[test]
fn rule_inputs_and_menus_stay_mounted_across_notices_and_port_scans() {
    let mut initial = snapshot(vec![rule("app.devbase.localhost", 3000, true)], 443);
    initial.zones = vec!["example.com".to_string()];
    initial.dns_names = vec!["www.example.com".to_string(), "api.example.com".to_string()];
    let fake = Fake::new(initial).slow(250);
    let (_turn, mut app) = start(fake);

    app.wait_for("id:proxy-rule-0").unwrap();
    app.wait_for("id:proxy-ports-0").unwrap();
    app.wait_for("id:proxy-dns-suggestion-0").unwrap();

    let header_id = app.screen().id("id:proxy-rules").unwrap();
    let domain_id = app.screen().id("id:proxy-domain-0").unwrap();
    let upstream_id = app.screen().id("id:proxy-upstream-0").unwrap();
    let port_id = app.screen().id("id:proxy-port-0").unwrap();
    let ports_menu_id = app.screen().id("id:proxy-ports-0").unwrap();
    let tunnel_domain_id = app.screen().id("id:proxy-tunnel-domain-0").unwrap();
    let host_menu_id = app.screen().id("id:proxy-dns-suggestion-0").unwrap();

    // Start an async apply, then open ports_menu while it is in flight. When the
    // notice arrives and refresh() completes, the open menu and every rule node
    // must remain mounted without collapsing.
    let notice = "Proxy configuration published. Check DNS, TLS and upstream separately.";
    app.press("id:proxy-apply").unwrap();
    app.press("id:proxy-ports-0").unwrap();
    app.wait_for("3000 · node").unwrap();
    app.wait_for("5173 · vite").unwrap();
    app.wait_for(notice).unwrap();
    assert!(
        app.screen().shows("3000 · node"),
        "an async notice must not collapse an open ports_menu"
    );
    assert_eq!(app.screen().id("id:proxy-rules").unwrap(), header_id);
    assert_eq!(app.screen().id("id:proxy-domain-0").unwrap(), domain_id);
    assert_eq!(app.screen().id("id:proxy-upstream-0").unwrap(), upstream_id);
    assert_eq!(app.screen().id("id:proxy-port-0").unwrap(), port_id);
    assert_eq!(app.screen().id("id:proxy-ports-0").unwrap(), ports_menu_id);
    assert_eq!(
        app.screen().id("id:proxy-tunnel-domain-0").unwrap(),
        tunnel_domain_id
    );
    assert_eq!(
        app.screen().id("id:proxy-dns-suggestion-0").unwrap(),
        host_menu_id
    );

    // Refreshing listening ports updates PortsState without tearing down the menu trigger.
    app.press("text:Refresh listening ports").unwrap();
    app.wait(Duration::from_millis(150)).unwrap();
    assert_eq!(app.screen().id("id:proxy-ports-0").unwrap(), ports_menu_id);

    // Clearing the notice via an unrelated Cloudflare draft edit must keep every rule node mounted.
    app.type_text("id:tunnel-account", "acct-1").unwrap();
    app.wait_for_gone(notice, Duration::from_secs(2)).unwrap();
    assert_eq!(app.screen().id("id:proxy-rules").unwrap(), header_id);
    assert_eq!(app.screen().id("id:proxy-domain-0").unwrap(), domain_id);
    assert_eq!(app.screen().id("id:proxy-upstream-0").unwrap(), upstream_id);
    assert_eq!(app.screen().id("id:proxy-port-0").unwrap(), port_id);
    assert_eq!(app.screen().id("id:proxy-ports-0").unwrap(), ports_menu_id);
    assert_eq!(
        app.screen().id("id:proxy-tunnel-domain-0").unwrap(),
        tunnel_domain_id
    );
    assert_eq!(
        app.screen().id("id:proxy-dns-suggestion-0").unwrap(),
        host_menu_id
    );

    // Selecting a DNS suggestion from host_menu updates the field while keeping triggers stable.
    app.press("id:proxy-dns-suggestion-0").unwrap();
    app.wait_for("api.example.com").unwrap();
    app.press("text:api.example.com").unwrap();
    app.wait(Duration::from_millis(100)).unwrap();
    assert_eq!(
        app.screen().input_text("id:proxy-tunnel-domain-0").unwrap(),
        Some("api.example.com")
    );
    assert_eq!(
        app.screen().id("id:proxy-dns-suggestion-0").unwrap(),
        host_menu_id
    );
    assert_eq!(app.screen().id("id:proxy-ports-0").unwrap(), ports_menu_id);
}

#[test]
fn authority_header_integrates_back_button_and_returns_to_rules() {
    let fake = Fake::new(snapshot(Vec::new(), 443));
    let (_turn, mut app) = start(fake);

    app.press("id:ca-open").unwrap();
    app.wait_for("id:ca-header").unwrap();
    app.wait_for("id:ca-back").unwrap();
    assert!(app.screen().shows("← HTTPS Proxy"));

    app.press("id:ca-back").unwrap();
    app.wait_for("id:proxy-rules").unwrap();
}

#[test]
fn ca_runtime_trust_card_shows_exports_and_copies_shell_and_dotenv() {
    let mut status = not_created();
    status.active = info("active-1", "Ready", TrustState::Trusted, true, 'a');
    let fake = Fake::new(snapshot(Vec::new(), 443)).with_authority(status);
    let (_turn, mut app) = start(fake);

    app.press("id:ca-open").unwrap();
    app.wait_for("id:ca-runtime-trust").unwrap();
    assert!(app.screen().shows("CLI & Runtime Trust"));
    assert!(app.screen().shows("/tmp/devbase-ca/active-1.pem"));
    assert!(app.screen().matches("text:/NODE_EXTRA_CA_CERTS=/").unwrap());
    assert!(app.screen().matches("text:/SSL_CERT_FILE=/").unwrap());
    assert!(app.screen().matches("text:/REQUESTS_CA_BUNDLE=/").unwrap());
    assert!(app.screen().matches("text:/CURL_CA_BUNDLE=/").unwrap());

    app.press("id:ca-copy-shell-exports").unwrap();
    app.wait_for("Copied CA shell exports").unwrap();
    let expected_shell = concat!(
        "export NODE_EXTRA_CA_CERTS=\"/tmp/devbase-ca/active-1.pem\"\n",
        "export SSL_CERT_FILE=\"/tmp/devbase-ca/active-1.pem\"\n",
        "export REQUESTS_CA_BUNDLE=\"/tmp/devbase-ca/active-1.pem\"\n",
        "export CURL_CA_BUNDLE=\"/tmp/devbase-ca/active-1.pem\""
    );
    assert_eq!(app.screen().clipboard(), Some(expected_shell));

    app.press("id:ca-copy-dotenv").unwrap();
    app.wait_for("Copied CA .env lines").unwrap();
    let expected_dotenv = concat!(
        "NODE_EXTRA_CA_CERTS=\"/tmp/devbase-ca/active-1.pem\"\n",
        "SSL_CERT_FILE=\"/tmp/devbase-ca/active-1.pem\"\n",
        "REQUESTS_CA_BUNDLE=\"/tmp/devbase-ca/active-1.pem\"\n",
        "CURL_CA_BUNDLE=\"/tmp/devbase-ca/active-1.pem\""
    );
    assert_eq!(app.screen().clipboard(), Some(expected_dotenv));
}

#[test]
fn tunnel_qr_dialog_renders_svg_copies_url_and_protected_curl_without_leaking_token() {
    let mut protected_rule = rule("api.devbase.localhost", 4000, true);
    protected_rule.tunnel_auth = true;
    let public_tunnel_rule = rule("web.devbase.localhost", 5173, true);
    let fake = Fake::new(snapshot(
        vec![
            rule("local.devbase.localhost", 3000, false),
            protected_rule,
            public_tunnel_rule,
        ],
        443,
    ));
    let (_turn, mut app) = start(fake.clone());

    app.wait_for("id:proxy-rule-0").unwrap();
    app.wait_for("id:rule-qr-1").unwrap();
    app.wait_for("id:rule-qr-2").unwrap();
    assert!(!app.screen().matches("id:rule-qr-0").unwrap());

    // Open QR dialog for protected tunnel rule.
    app.press("id:rule-qr-1").unwrap();
    app.wait_for("id:tunnel-qr-dialog").unwrap();
    app.wait_for("id:tunnel-qr-svg").unwrap();
    assert!(app.screen().shows("https://www.example.com"));
    assert!(app.screen().matches("id:tunnel-qr-copy-curl").unwrap());

    app.press("id:tunnel-qr-copy-url").unwrap();
    app.wait_for("Copied tunnel URL").unwrap();
    assert_eq!(app.screen().clipboard(), Some("https://www.example.com"));

    app.press("id:tunnel-qr-copy-curl").unwrap();
    app.wait_for("Copied curl command with token for api.devbase.localhost")
        .unwrap();
    assert_eq!(
        app.screen().clipboard(),
        Some("curl -H 'Authorization: Bearer secret-access-token' 'https://www.example.com'")
    );
    assert!(
        !app.screen().shows("secret-access-token"),
        "the tunnel access token must never be rendered on screen"
    );
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "ensure token api.devbase.localhost")
    );

    app.press("id:tunnel-qr-close").unwrap();
    app.wait_for_gone("id:tunnel-qr-dialog", Duration::from_secs(2))
        .unwrap();

    // Open QR dialog for an unprotected tunnel rule: curl-with-token button must not appear.
    app.press("id:rule-qr-2").unwrap();
    app.wait_for("id:tunnel-qr-dialog").unwrap();
    assert!(!app.screen().matches("id:tunnel-qr-copy-curl").unwrap());
    app.press("id:tunnel-qr-close").unwrap();
    app.wait_for_gone("id:tunnel-qr-dialog", Duration::from_secs(2))
        .unwrap();
}

#[test]
fn failed_probes_surface_categorized_diagnosis_hints() {
    let mut initial = snapshot(vec![rule("app.devbase.localhost", 3000, true)], 443);
    let mut workers = BTreeMap::new();
    workers.insert(
        "devbase-app".to_string(),
        "https://devbase-app.devbase.workers.dev".to_string(),
    );
    initial.workers = ProxyWorkers::new(workers);
    initial
        .statuses
        .insert("tunnel".to_string(), "Running".to_string());
    initial.statuses.insert(
        "tunnel:worker:devbase-app".to_string(),
        "Connecting".to_string(),
    );

    // 1. Upstream port not listening + Tunnel disconnected.
    let fake = Fake::new(initial)
        .with_proxy_failure("Connection refused")
        .with_tunnel_failure("Tunnel disconnected");
    let (_turn, mut app) = start(fake);

    app.press("id:proxy-check-https-app-devbase-localhost")
        .unwrap();
    app.wait_for("id:proxy-probe-hint-app-devbase-localhost")
        .unwrap();
    assert!(
        app.screen().shows(
            "Upstream port :3000 is not listening. Start your app server in a terminal first."
        )
    );

    app.press("id:proxy-check-tunnel-app-devbase-localhost")
        .unwrap();
    app.wait_for("id:tunnel-probe-hint-app-devbase-localhost")
        .unwrap();
    assert!(
        app.screen()
            .shows("Cloudflare tunnel is not connected for this rule.")
    );
    drop((_turn, app));

    // 2. Local CA untrusted / TLS failure.
    let fake_tls = Fake::new(snapshot(
        vec![rule("app.devbase.localhost", 3000, false)],
        443,
    ))
    .with_proxy_failure("TLS");
    let (_turn2, mut app2) = start(fake_tls);
    app2.press("id:proxy-check-https-app-devbase-localhost")
        .unwrap();
    app2.wait_for("id:proxy-probe-hint-app-devbase-localhost")
        .unwrap();
    assert!(
        app2.screen()
            .shows("Local CA is not trusted yet. Trust the CA in Certificate Authority settings.")
    );
}

struct FakeDevLogs {
    requested: Mutex<Vec<String>>,
}

impl FakeDevLogs {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            requested: Mutex::new(Vec::new()),
        })
    }

    fn requested(&self) -> Vec<String> {
        self.requested
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl DevLogsProvider for FakeDevLogs {
    fn logs_root(&self) -> PathBuf {
        PathBuf::from("/tmp/devbase-fake-logs")
    }

    fn database_ids(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn read_log(
        &self,
        job_id: &str,
        _stream: DevLogsStream,
        _known: Option<DevLogsKnownFile>,
    ) -> Result<DevLogsRead, String> {
        self.requested
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(job_id.to_string());
        Ok(DevLogsRead::Snapshot(DevLogsSnapshot {
            lines: vec![format!("{job_id} live log line")],
            truncated: false,
            size: 24,
            modified: None,
        }))
    }

    fn open_in_console(&self, _path: &std::path::Path) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn explicit_logs_buttons_open_live_log_drawer_for_proxy_and_tunnel() {
    let logs_provider = FakeDevLogs::new();
    use_fake_dev_logs();
    install_dev_logs_provider(logs_provider.clone() as Arc<dyn DevLogsProvider>);

    let fake = Fake::new(snapshot(
        vec![rule("app.devbase.localhost", 3000, true)],
        443,
    ));
    let (_turn, mut app) = start(fake);

    app.wait_for("id:proxy-open-logs").unwrap();
    app.wait_for("id:tunnel-open-logs").unwrap();

    app.press("id:proxy-open-logs").unwrap();
    app.wait_for("id:dev-log-drawer").unwrap();
    app.wait_for("proxy live log line").unwrap();
    assert!(logs_provider.requested().iter().any(|job| job == "proxy"));

    app.press("id:tunnel-open-logs").unwrap();
    app.wait_for("tunnel live log line").unwrap();
    assert!(logs_provider.requested().iter().any(|job| job == "tunnel"));
}

#[test]
fn top_status_bar_hides_trust_action_when_ca_is_trusted_and_removes_duplicates() {
    let mut status = not_created();
    status.active = info("active-1", "Ready", TrustState::Trusted, true, 'a');
    let fake = Fake::new(snapshot(
        vec![rule("app.devbase.localhost", 3000, false)],
        443,
    ))
    .with_authority(status);
    let (_turn, mut app) = start(fake);

    app.wait_for("id:ca-trust-label").unwrap();
    assert!(app.screen().shows("Trusted"));
    assert!(app.screen().matches("id:ca-open").unwrap());
    assert!(!app.screen().matches("id:ca-trust").unwrap());
    assert!(!app.screen().matches("id:ca-remove-trust").unwrap());
    assert!(!app.screen().matches("id:proxy-logs").unwrap());
    assert!(app.screen().matches("id:proxy-open-logs").unwrap());
}
