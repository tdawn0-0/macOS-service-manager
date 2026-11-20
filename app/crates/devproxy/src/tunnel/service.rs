//! Cloudflare settings and `SyncTunnels`.
//!
//! Ported from `cloudflare_api.go`'s settings lifecycle. The API sequence is
//! 1:1 with the Go code: metadata discovery, zone-ownership validation, orphan
//! deletion, then a sorted deploy per Worker, and finally one state
//! transaction that also rewrites `proxy/tunnel.json`.

use std::path::PathBuf;
use std::sync::Arc;

use devcore::model::{CloudflareDnsRecord, CloudflareZone, TunnelWorkerState};
use devcore::state::Store;

use super::cloudflare::{
    CloudflareClient, CloudflareError, current_worker_digest, has_cf_error_code,
    match_zone_for_hostname,
};
use super::config::{
    TunnelConfigError, TunnelStatusSnapshot, needs_sync, published_config_exists,
    write_config_from_state, write_status,
};
use crate::secrets::{SecretError, SecretStore, tunnel_auth_digest};

/// Errors from settings and synchronization.
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    State(#[from] devcore::state::StateError),
    #[error(transparent)]
    Config(#[from] TunnelConfigError),
    #[error(transparent)]
    Cloudflare(#[from] CloudflareError),
    #[error(transparent)]
    Secret(#[from] SecretError),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Job(String),
}

/// The launchd tunnel job seam. Phase 4 wires the real job control; tests use
/// [`NoTunnelJob`].
pub trait TunnelJobControl: Send + Sync {
    fn stop(&self) -> Result<(), TunnelError>;
    fn is_loaded(&self) -> bool {
        false
    }
}

/// A workspace with no tunnel job (tests and a fresh install).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTunnelJob;

impl TunnelJobControl for NoTunnelJob {
    fn stop(&self) -> Result<(), TunnelError> {
        Ok(())
    }
}

/// The settings and synchronization facade.
#[derive(Clone)]
pub struct TunnelService {
    root: PathBuf,
    store: Store,
    api_base: String,
    jobs: Arc<dyn TunnelJobControl>,
}

impl TunnelService {
    pub fn new(store: Store, api_base: impl Into<String>) -> Self {
        TunnelService {
            root: store.root().to_path_buf(),
            store,
            api_base: api_base.into(),
            jobs: Arc::new(NoTunnelJob),
        }
    }

    pub fn with_jobs(mut self, jobs: Arc<dyn TunnelJobControl>) -> Self {
        self.jobs = jobs;
        self
    }

    pub fn secrets(&self) -> SecretStore {
        SecretStore::new(&self.root)
    }

    pub fn has_cloudflare_token(&self) -> bool {
        self.secrets().has_cloudflare_token()
    }

    pub fn read_cloudflare_token(&self) -> Result<String, TunnelError> {
        Ok(self.secrets().read_cloudflare_token()?)
    }

    /// Whether the saved target differs from the deployed Workers.
    pub fn needs_sync(&self) -> Result<bool, TunnelError> {
        let state = self.store.read()?;
        Ok(needs_sync(
            &state,
            &self.secrets(),
            &current_worker_digest(),
        ))
    }

    /// Save the API token (0600 file), account ID, and client ID. The token is
    /// never written into state.
    pub fn save_settings(
        &self,
        api_token: &str,
        account_id: &str,
        client_id: &str,
    ) -> Result<(), TunnelError> {
        self.store.init()?;
        let account = normalize_account_id(account_id)?;
        let mut client = normalize_client_id(client_id)?;
        let cleaned_token = if api_token.trim().is_empty() {
            None
        } else {
            Some(crate::secrets::validate_cloudflare_token(api_token)?)
        };
        self.store.with_state(|state| {
            if client.is_empty() && !state.tunnel_client_id.is_empty() {
                client = state.tunnel_client_id.clone();
            }
            if client.is_empty()
                && (cleaned_token.is_some()
                    || !account.is_empty()
                    || !state.tunnel_workers.is_empty())
            {
                client = format!("mac-{}", short_id());
            }
            state.cloudflare_account_id = account.clone();
            state.tunnel_client_id = client.clone();
            if let Some(token) = &cleaned_token {
                self.secrets()
                    .write_cloudflare_token(token)
                    .map_err(|error| {
                        devcore::state::StateError::Io(std::io::Error::other(error))
                    })?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Stop the tunnel, optionally delete Workers, then drop all Cloudflare
    /// settings from state and disk.
    pub async fn remove_settings(&self, delete_workers: bool) -> Result<(), TunnelError> {
        self.store.init()?;
        let state = self.store.read()?;
        self.jobs.stop()?;
        write_status(&self.root, TunnelStatusSnapshot::default())?;

        if delete_workers && !state.tunnel_workers.is_empty() {
            let token = self.secrets().read_cloudflare_token().map_err(|error| {
                TunnelError::Invalid(format!(
                    "read Cloudflare API token to delete Workers: {error}"
                ))
            })?;
            let client = CloudflareClient::new(&self.api_base, &token)?;
            let account = if state.cloudflare_account_id.is_empty() {
                client.discover_account_id().await?
            } else {
                state.cloudflare_account_id.clone()
            };
            let mut names: Vec<String> = state.tunnel_workers.keys().cloned().collect();
            names.sort();
            for name in names {
                client.delete_worker_script(&account, &name).await?;
            }
        }

        let token_path = self.secrets().cloudflare_token_path();
        self.store.with_state(|state| {
            match std::fs::remove_file(&token_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            state.cloudflare_account_id.clear();
            state.cloudflare_subdomain.clear();
            state.cloudflare_zones.clear();
            state.cloudflare_dns.clear();
            if delete_workers {
                let workers = state.tunnel_workers.clone();
                for rule in &mut state.proxy_rules {
                    if workers.contains_key(&rule.resolved_tunnel_worker()) {
                        rule.tunnel_url.clear();
                    }
                }
                state.tunnel_workers.clear();
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Discover the account, workers.dev subdomain, zones, and DNS records.
    pub async fn refresh_metadata(&self) -> Result<(), TunnelError> {
        self.store.init()?;
        let token = self.secrets().read_cloudflare_token().map_err(|_| {
            TunnelError::Invalid("configure and save a Cloudflare API token first".into())
        })?;
        let state = self.store.read()?;
        let client = CloudflareClient::new(&self.api_base, &token)?;
        let metadata = fetch_metadata(&client, &state.cloudflare_account_id).await?;
        self.store.with_state(|state| {
            if state.cloudflare_account_id.is_empty() {
                state.cloudflare_account_id = metadata.account_id.clone();
            }
            state.cloudflare_subdomain = metadata.subdomain.clone();
            state.cloudflare_zones = metadata.zones.clone();
            state.cloudflare_dns = metadata.dns_records.clone();
            if state.tunnel_client_id.is_empty() {
                state.tunnel_client_id = format!("mac-{}", short_id());
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Reconcile desired Workers with Cloudflare and refresh the snapshot.
    pub async fn sync_tunnels(&self) -> Result<(), TunnelError> {
        self.store.init()?;
        let state = self.store.read()?;
        let desired: Vec<_> = state
            .proxy_rules
            .iter()
            .filter(|rule| rule.tunnel)
            .cloned()
            .collect();
        if desired.is_empty() && state.tunnel_workers.is_empty() {
            self.store.with_state(|state| {
                write_config_from_state(&self.root, state).map_err(|error| {
                    devcore::state::StateError::Io(std::io::Error::other(error))
                })?;
                Ok(())
            })?;
            return Ok(());
        }

        let token = self.secrets().read_cloudflare_token().map_err(|_| {
            TunnelError::Invalid("configure and save a Cloudflare API token first".into())
        })?;
        let tunnel_secret = self.secrets().ensure_tunnel_secret()?;
        let client = CloudflareClient::new(&self.api_base, &token)?;
        let metadata = fetch_metadata(&client, &state.cloudflare_account_id).await?;

        // Validate custom-domain ownership before mutating anything.
        let mut zone_ids = std::collections::BTreeMap::new();
        for rule in &desired {
            let custom_host = rule.effective_tunnel_domain();
            if custom_host.is_empty() {
                if metadata.subdomain.is_empty() {
                    return Err(TunnelError::Invalid(
                        "register a workers.dev subdomain in Cloudflare Dashboard, or set a Custom domain"
                            .into(),
                    ));
                }
                continue;
            }
            let Some(zone) = match_zone_for_hostname(&custom_host, &metadata.zones) else {
                return Err(TunnelError::Invalid(format!(
                    "custom domain {custom_host} does not belong to any active Cloudflare zone on this account"
                )));
            };
            zone_ids.insert(rule.resolved_tunnel_worker(), zone.id.clone());
        }

        let desired_workers: std::collections::BTreeSet<String> = desired
            .iter()
            .map(|rule| rule.resolved_tunnel_worker())
            .collect();
        let mut orphans: Vec<String> = state
            .tunnel_workers
            .keys()
            .filter(|name| !desired_workers.contains(*name))
            .cloned()
            .collect();
        orphans.sort();
        for name in orphans {
            client
                .delete_worker_script(&metadata.account_id, &name)
                .await?;
        }

        let mut desired = desired;
        desired.sort_by_key(|rule| rule.resolved_tunnel_worker());
        let mut deployed = std::collections::BTreeMap::new();
        let mut urls_by_domain = std::collections::BTreeMap::new();
        for rule in &desired {
            let worker_name = rule.resolved_tunnel_worker();
            let access_token = if rule.tunnel_auth {
                self.secrets().ensure_tunnel_access_token(&worker_name)?
            } else {
                String::new()
            };
            let existing_tag = client
                .get_script_migration_tag(&metadata.account_id, &worker_name)
                .await?;
            client
                .put_worker_script(
                    &metadata.account_id,
                    &worker_name,
                    &tunnel_secret,
                    &access_token,
                    &existing_tag,
                )
                .await?;

            let custom_host = rule.effective_tunnel_domain();
            let (zone_id, public_url) = if !custom_host.is_empty() {
                let zone_id = zone_ids.get(&worker_name).cloned().unwrap_or_default();
                if !metadata.subdomain.is_empty() {
                    match client
                        .set_script_subdomain_enabled(&metadata.account_id, &worker_name, false)
                        .await
                    {
                        Ok(()) => {}
                        Err(error) if has_cf_error_code(&error, 404, &[10007]) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                client
                    .set_script_custom_domains(
                        &metadata.account_id,
                        &worker_name,
                        &zone_id,
                        &custom_host,
                    )
                    .await?;
                (zone_id, format!("https://{custom_host}"))
            } else {
                if let Some(previous) = state.tunnel_workers.get(&worker_name)
                    && !previous.custom_domain.is_empty()
                {
                    match client
                        .set_script_custom_domains(&metadata.account_id, &worker_name, "", "")
                        .await
                    {
                        Ok(()) => {}
                        Err(error) if has_cf_error_code(&error, 404, &[10007]) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                client
                    .set_script_subdomain_enabled(&metadata.account_id, &worker_name, true)
                    .await?;
                (
                    String::new(),
                    format!("https://{worker_name}.{}.workers.dev", metadata.subdomain),
                )
            };

            deployed.insert(
                worker_name.clone(),
                TunnelWorkerState {
                    script_name: worker_name,
                    public_url: public_url.clone(),
                    custom_domain: custom_host,
                    zone_id,
                    local_domain: rule.domain.clone(),
                    local_port: rule.local_port,
                    script_digest: current_worker_digest(),
                    auth_digest: tunnel_auth_digest(&access_token),
                },
            );
            urls_by_domain.insert(rule.domain.clone(), public_url);
        }

        self.store.with_state(|state| {
            if state.cloudflare_account_id.is_empty() {
                state.cloudflare_account_id = metadata.account_id.clone();
            }
            state.cloudflare_subdomain = metadata.subdomain.clone();
            state.cloudflare_zones = metadata.zones.clone();
            state.cloudflare_dns = metadata.dns_records.clone();
            if state.tunnel_client_id.is_empty() {
                state.tunnel_client_id = format!("mac-{}", short_id());
            }
            state.tunnel_workers = deployed.clone();
            for rule in &mut state.proxy_rules {
                if let Some(url) = urls_by_domain.get(&rule.domain) {
                    rule.tunnel_url = url.clone();
                } else if !rule.tunnel {
                    rule.tunnel_url.clear();
                }
            }
            write_config_from_state(&self.root, state)
                .map_err(|error| devcore::state::StateError::Io(std::io::Error::other(error)))?;
            Ok(())
        })?;
        Ok(())
    }

    /// Whether a tunnel snapshot has been published.
    pub fn published_snapshot_exists(&self) -> bool {
        published_config_exists(&self.root)
    }
}

/// Discovered Cloudflare metadata.
#[derive(Debug, Clone)]
pub struct CloudflareMetadata {
    pub account_id: String,
    pub subdomain: String,
    pub zones: Vec<CloudflareZone>,
    pub dns_records: Vec<CloudflareDnsRecord>,
}

/// Fetch account, subdomain, zones, and DNS records in the Go order.
pub async fn fetch_metadata(
    client: &CloudflareClient,
    account_id: &str,
) -> Result<CloudflareMetadata, CloudflareError> {
    let account_id = if account_id.trim().is_empty() {
        client.discover_account_id().await?
    } else {
        account_id.trim().to_string()
    };
    let subdomain = client.get_workers_subdomain(&account_id).await?;
    let zones = client.list_zones().await?;
    let mut dns_records = Vec::new();
    for zone in &zones {
        match client.list_dns_records(&zone.id).await {
            Ok(records) => dns_records.extend(records),
            Err(_) => continue,
        }
    }
    Ok(CloudflareMetadata {
        account_id,
        subdomain,
        zones,
        dns_records,
    })
}

pub fn normalize_client_id(raw: &str) -> Result<String, TunnelError> {
    let id = raw.trim().to_lowercase();
    if id.is_empty() {
        return Ok(id);
    }
    let valid = (2..=40).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid {
        return Err(TunnelError::Invalid(
            "client ID must be 2–40 lowercase letters, digits, or hyphens".into(),
        ));
    }
    Ok(id)
}

pub fn normalize_account_id(raw: &str) -> Result<String, TunnelError> {
    let id = raw.trim();
    if id.is_empty() {
        return Ok(String::new());
    }
    let valid = (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if !valid {
        return Err(TunnelError::Invalid(
            "Cloudflare account ID must be 8–64 alphanumeric characters, hyphens, or underscores"
                .into(),
        ));
    }
    Ok(id.to_string())
}

fn short_id() -> String {
    let raw: [u8; 8] = rand::random();
    hex::encode(raw)[..8].to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use http::{Request, Response, StatusCode};
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tokio::net::TcpListener;

    use super::*;
    use crate::tunnel::config::read_published_config;
    use devcore::model::{ProxyRule, TunnelWorkerState};

    #[derive(Debug, Clone)]
    struct Recorded {
        method: String,
        path: String,
    }

    struct Fake {
        base_url: String,
        recorded: Arc<Mutex<Vec<Recorded>>>,
    }

    async fn fake_server(routes: BTreeMap<(String, String), (u16, String)>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let routes = Arc::new(routes);
        let stored = recorded.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                let recorded = stored.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<Incoming>| {
                        let routes = routes.clone();
                        let recorded = recorded.clone();
                        async move {
                            let method = request.method().to_string();
                            let path = request
                                .uri()
                                .path_and_query()
                                .map(|value| value.as_str())
                                .unwrap_or("/")
                                .to_string();
                            let _ = request.into_body().collect().await;
                            recorded.lock().unwrap().push(Recorded {
                                method: method.clone(),
                                path: path.clone(),
                            });
                            let (status, body) = routes.get(&(method, path)).cloned().unwrap_or_else(|| {
                                (
                                    404,
                                    json!({"success":false,"errors":[{"code":10007,"message":"not found"}],"result":null}).to_string(),
                                )
                            });
                            Ok::<_, std::convert::Infallible>(
                                Response::builder()
                                    .status(StatusCode::from_u16(status).unwrap())
                                    .header("content-type", "application/json")
                                    .body(Full::new(Bytes::from(body)))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Fake {
            base_url: format!("http://{address}"),
            recorded,
        }
    }

    fn success(result: Value) -> (u16, String) {
        (
            200,
            json!({"success": true, "errors": [], "result": result}).to_string(),
        )
    }

    fn route(
        routes: &mut BTreeMap<(String, String), (u16, String)>,
        method: &str,
        path: &str,
        response: (u16, String),
    ) {
        routes.insert((method.into(), path.into()), response);
    }

    fn rule(domain: &str, port: i64) -> ProxyRule {
        ProxyRule {
            domain: domain.into(),
            local_port: port,
            ..ProxyRule::default()
        }
    }

    fn base_state() -> devcore::model::State {
        let mut app = rule("app.devbase.localhost", 3000);
        app.tunnel = true;
        let mut api = rule("api.devbase.localhost", 3001);
        api.tunnel = true;
        api.tunnel_domain = "app.example.com".into();
        devcore::model::State {
            proxy_listener_port: 443,
            proxy_rules: vec![app, api, rule("plain.devbase.localhost", 3002)],
            ..devcore::model::State::default()
        }
    }

    fn setup(state: devcore::model::State) -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        store.write_state(&state).unwrap();
        (dir, store)
    }

    fn metadata_routes(subdomain: &str, zones: Value) -> BTreeMap<(String, String), (u16, String)> {
        let mut routes = BTreeMap::new();
        route(
            &mut routes,
            "GET",
            "/accounts/acct1234/workers/subdomain",
            success(json!({"subdomain": subdomain})),
        );
        route(
            &mut routes,
            "GET",
            "/zones?status=active&per_page=50",
            success(zones),
        );
        routes
    }

    #[test]
    fn save_settings_writes_only_files_for_secrets() {
        let (dir, store) = setup(devcore::model::State::default());
        let service = TunnelService::new(store.clone(), "http://127.0.0.1:1");
        service
            .save_settings("token-abcdefghij", "acct1234", "")
            .unwrap();
        let state = store.read().unwrap();
        assert_eq!(state.cloudflare_account_id, "acct1234");
        assert!(state.tunnel_client_id.starts_with("mac-"));
        let token_path = service.secrets().cloudflare_token_path();
        assert_eq!(
            std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            service.secrets().read_cloudflare_token().unwrap(),
            "token-abcdefghij"
        );
        let raw = std::fs::read_to_string(store.state_path()).unwrap();
        assert!(!raw.contains("token-abcdefghij"));
        assert!(!raw.contains("cloudflare.key"));
        assert!(!String::from_utf8_lossy(&std::fs::read(&token_path).unwrap()).is_empty());

        for (token, account, client) in [
            ("short", "acct1234", ""),
            ("has space here", "acct1234", ""),
            ("token-abcdefghij", "bad", ""),
            ("token-abcdefghij", "acct1234", "bad id"),
        ] {
            assert!(
                service.save_settings(token, account, client).is_err(),
                "{token:?} {account:?} {client:?}"
            );
        }
        assert!(dir.path().exists());
    }

    #[tokio::test]
    async fn sync_deploys_sorted_workers_and_publishes_the_snapshot() {
        let (_dir, store) = setup(base_state());
        let service = TunnelService::new(store.clone(), "placeholder");
        service
            .save_settings("token-abcdefghij", "acct1234", "")
            .unwrap();

        let mut routes = metadata_routes("edge", json!([{"id": "zone1", "name": "example.com"}]));
        route(
            &mut routes,
            "GET",
            "/zones/zone1/dns_records?per_page=100",
            success(json!([{"id": "r1", "name": "app.example.com", "type": "A"}])),
        );
        for worker in ["devbase-app", "devbase-api"] {
            route(
                &mut routes,
                "PUT",
                &format!("/accounts/acct1234/workers/scripts/{worker}"),
                success(json!({})),
            );
            route(
                &mut routes,
                "POST",
                &format!("/accounts/acct1234/workers/scripts/{worker}/subdomain"),
                success(json!({})),
            );
        }
        route(
            &mut routes,
            "PUT",
            "/accounts/acct1234/workers/scripts/devbase-api/domains/records",
            success(json!({})),
        );
        let fake = fake_server(routes).await;
        let service = TunnelService::new(store.clone(), fake.base_url.clone());
        service
            .save_settings("token-abcdefghij", "acct1234", "")
            .unwrap();
        service.sync_tunnels().await.unwrap();

        let state = store.read().unwrap();
        assert_eq!(state.cloudflare_subdomain, "edge");
        assert_eq!(state.cloudflare_zones.len(), 1);
        assert_eq!(state.cloudflare_dns.len(), 1);
        assert_eq!(state.tunnel_workers.len(), 2);
        let app = &state.tunnel_workers["devbase-app"];
        assert_eq!(app.public_url, "https://devbase-app.edge.workers.dev");
        assert_eq!(app.local_domain, "app.devbase.localhost");
        assert_eq!(app.local_port, 3000);
        assert_eq!(app.script_digest, current_worker_digest());
        assert!(app.custom_domain.is_empty());
        let api = &state.tunnel_workers["devbase-api"];
        assert_eq!(api.public_url, "https://app.example.com");
        assert_eq!(api.custom_domain, "app.example.com");
        assert_eq!(api.zone_id, "zone1");
        assert_eq!(state.proxy_rules[0].tunnel_url, app.public_url);
        assert_eq!(state.proxy_rules[1].tunnel_url, api.public_url);
        assert!(state.proxy_rules[2].tunnel_url.is_empty());

        let snapshot = read_published_config(store.root()).unwrap();
        assert_eq!(snapshot.routes.len(), 2);
        assert_eq!(snapshot.routes[0].worker_name, "devbase-api");
        assert_eq!(snapshot.routes[1].worker_name, "devbase-app");
        assert_eq!(snapshot.client_id, state.tunnel_client_id);

        let recorded = fake.recorded.lock().unwrap().clone();
        let sequence: Vec<(String, String)> = recorded
            .iter()
            .map(|request| (request.method.clone(), request.path.clone()))
            .collect();
        assert_eq!(
            sequence,
            vec![
                ("GET".into(), "/accounts/acct1234/workers/subdomain".into()),
                ("GET".into(), "/zones?status=active&per_page=50".into()),
                ("GET".into(), "/zones/zone1/dns_records?per_page=100".into()),
                (
                    "GET".into(),
                    "/accounts/acct1234/workers/services/devbase-api".into()
                ),
                (
                    "PUT".into(),
                    "/accounts/acct1234/workers/scripts/devbase-api".into()
                ),
                (
                    "POST".into(),
                    "/accounts/acct1234/workers/scripts/devbase-api/subdomain".into()
                ),
                (
                    "PUT".into(),
                    "/accounts/acct1234/workers/scripts/devbase-api/domains/records".into()
                ),
                (
                    "GET".into(),
                    "/accounts/acct1234/workers/services/devbase-app".into()
                ),
                (
                    "PUT".into(),
                    "/accounts/acct1234/workers/scripts/devbase-app".into()
                ),
                (
                    "POST".into(),
                    "/accounts/acct1234/workers/scripts/devbase-app/subdomain".into()
                ),
            ]
        );
        let raw_state = std::fs::read_to_string(store.state_path()).unwrap();
        let raw_snapshot =
            std::fs::read_to_string(crate::tunnel::config::published_config_path(store.root()))
                .unwrap();
        for secret in [
            "token-abcdefghij",
            &service.secrets().read_tunnel_secret().unwrap(),
        ] {
            assert!(!raw_state.contains(secret));
            assert!(!raw_snapshot.contains(secret));
        }
    }

    #[tokio::test]
    async fn orphaned_workers_are_deleted_before_deploying() {
        let mut state = base_state();
        state.proxy_rules.truncate(1);
        state.tunnel_workers.insert(
            "devbase-old".into(),
            TunnelWorkerState {
                script_name: "devbase-old".into(),
                public_url: "https://devbase-old.edge.workers.dev".into(),
                local_domain: "old.devbase.localhost".into(),
                local_port: 3999,
                ..TunnelWorkerState::default()
            },
        );
        let (_dir, store) = setup(state);
        let mut routes = metadata_routes("edge", json!([]));
        route(
            &mut routes,
            "DELETE",
            "/accounts/acct1234/workers/scripts/devbase-old?force=true",
            success(json!({})),
        );
        route(
            &mut routes,
            "PUT",
            "/accounts/acct1234/workers/scripts/devbase-app",
            success(json!({})),
        );
        route(
            &mut routes,
            "POST",
            "/accounts/acct1234/workers/scripts/devbase-app/subdomain",
            success(json!({})),
        );
        let fake = fake_server(routes).await;
        let service = TunnelService::new(store.clone(), fake.base_url.clone());
        service
            .save_settings("token-abcdefghij", "acct1234", "")
            .unwrap();
        service.sync_tunnels().await.unwrap();

        let recorded = fake.recorded.lock().unwrap().clone();
        let delete = recorded
            .iter()
            .position(|request| request.method == "DELETE")
            .expect("orphan deleted");
        let deploy = recorded
            .iter()
            .position(|request| {
                request.method == "PUT" && request.path.ends_with("/scripts/devbase-app")
            })
            .expect("worker deployed");
        assert!(delete < deploy);
        assert_eq!(
            recorded[delete].path,
            "/accounts/acct1234/workers/scripts/devbase-old?force=true"
        );
        assert!(recorded.iter().any(|request| {
            request
                .path
                .ends_with("/scripts/devbase-old/domains/records")
        }));
        let state = store.read().unwrap();
        assert!(!state.tunnel_workers.contains_key("devbase-old"));
        assert!(state.tunnel_workers.contains_key("devbase-app"));
    }

    #[tokio::test]
    async fn custom_domains_must_belong_to_an_active_zone() {
        let (_dir, store) = setup(base_state());
        let mut routes = metadata_routes("edge", json!([{"id": "zone1", "name": "example.com"}]));
        route(
            &mut routes,
            "GET",
            "/zones/zone1/dns_records?per_page=100",
            success(json!([])),
        );
        let fake = fake_server(routes).await;
        let service = TunnelService::new(store.clone(), fake.base_url.clone());
        service
            .save_settings("token-abcdefghij", "acct1234", "")
            .unwrap();

        let mut state = store.read().unwrap();
        state.proxy_rules[1].tunnel_domain = "app.other.test".into();
        store.write_state(&state).unwrap();
        let error = service.sync_tunnels().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "custom domain app.other.test does not belong to any active Cloudflare zone on this account"
        );
        let recorded = fake.recorded.lock().unwrap().clone();
        assert!(
            !recorded
                .iter()
                .any(|request| request.method == "PUT" || request.method == "DELETE"),
            "no Cloudflare resources were mutated"
        );
    }

    #[tokio::test]
    async fn workers_dev_requires_a_registered_subdomain() {
        let mut state = base_state();
        state.proxy_rules.truncate(1);
        let (_dir, store) = setup(state);
        let routes = metadata_routes("", json!([]));
        let fake = fake_server(routes).await;
        let service = TunnelService::new(store.clone(), fake.base_url.clone());
        service
            .save_settings("token-abcdefghij", "acct1234", "")
            .unwrap();
        let error = service.sync_tunnels().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "register a workers.dev subdomain in Cloudflare Dashboard, or set a Custom domain"
        );
    }

    #[tokio::test]
    async fn remove_settings_stops_the_job_and_clears_secrets() {
        let mut state = base_state();
        state.tunnel_workers.insert(
            "devbase-app".into(),
            TunnelWorkerState {
                script_name: "devbase-app".into(),
                public_url: "https://devbase-app.edge.workers.dev".into(),
                local_domain: "app.devbase.localhost".into(),
                local_port: 3000,
                ..TunnelWorkerState::default()
            },
        );
        state.proxy_rules[0].tunnel_url = "https://devbase-app.edge.workers.dev".into();
        let (_dir, store) = setup(state);
        let mut routes = BTreeMap::new();
        route(
            &mut routes,
            "DELETE",
            "/accounts/acct1234/workers/scripts/devbase-app?force=true",
            success(json!({})),
        );
        let fake = fake_server(routes).await;
        let service = TunnelService::new(store.clone(), fake.base_url.clone());
        service
            .save_settings("token-abcdefghij", "acct1234", "")
            .unwrap();
        service.remove_settings(true).await.unwrap();

        let state = store.read().unwrap();
        assert!(state.cloudflare_account_id.is_empty());
        assert!(state.cloudflare_subdomain.is_empty());
        assert!(state.tunnel_workers.is_empty());
        assert!(state.proxy_rules[0].tunnel_url.is_empty());
        assert!(!service.secrets().cloudflare_token_path().exists());
        assert_eq!(
            crate::tunnel::config::read_status(store.root())
                .unwrap()
                .phase,
            "Stopped"
        );
        assert!(
            fake.recorded
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.method == "DELETE")
        );
    }

    #[tokio::test]
    async fn refresh_metadata_requires_a_token() {
        let (_dir, store) = setup(devcore::model::State::default());
        let service = TunnelService::new(store, "http://127.0.0.1:1");
        assert_eq!(
            service.refresh_metadata().await.unwrap_err().to_string(),
            "configure and save a Cloudflare API token first"
        );
    }
}
