//! The Cloudflare API client and Worker deployment.
//!
//! Ported from `cloudflare_api.go`. Every endpoint, header, multipart part,
//! binding and migration tag is preserved; the base URL is injectable so tests
//! run against a local fake server and never reach the network.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use devcore::model::{CloudflareDnsRecord, CloudflareZone};

/// The production API base.
pub const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";
/// The Workers Script API date header value.
pub const SCRIPT_API_DATE: &str = "2025-08-01";
/// The Worker compatibility date; part of the deployed script digest.
pub const WORKER_COMPAT_DATE: &str = "2026-04-07";
/// The byte-frozen Worker module. Any edit changes every deployed digest.
pub const WORKER_INDEX_MJS: &str = include_str!("worker.mjs");

/// `sha256("2026-04-07\n" + worker.mjs)`, first 8 bytes as 16 hex characters.
pub fn current_worker_digest() -> String {
    let digest = Sha256::digest(format!("{WORKER_COMPAT_DATE}\n{WORKER_INDEX_MJS}").as_bytes());
    hex::encode(&digest[..8])
}

/// One structured Cloudflare API error entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct CfApiError {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

/// A non-2xx or `success:false` answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudflareApiError {
    pub status_code: u16,
    pub errors: Vec<CfApiError>,
    pub body: String,
}

impl std::error::Error for CloudflareApiError {}

impl std::fmt::Display for CloudflareApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.errors.is_empty() {
            let parts: Vec<String> = self
                .errors
                .iter()
                .filter_map(|item| {
                    if item.code != 0 {
                        Some(format!("{} (code {})", item.message, item.code))
                    } else if !item.message.is_empty() {
                        Some(item.message.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if !parts.is_empty() {
                return write!(
                    f,
                    "Cloudflare API error ({}): {}",
                    self.status_code,
                    parts.join("; ")
                );
            }
        }
        if !self.body.trim().is_empty() {
            return write!(
                f,
                "Cloudflare API error ({}): {}",
                self.status_code,
                self.body.trim()
            );
        }
        write!(f, "Cloudflare API error ({})", self.status_code)
    }
}

/// Everything the API client can fail with.
#[derive(Debug, thiserror::Error)]
pub enum CloudflareError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Api(#[from] CloudflareApiError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

/// Whether an error has the given HTTP status or any of the Cloudflare codes.
pub fn has_cf_error_code(error: &CloudflareError, status: u16, codes: &[i64]) -> bool {
    let CloudflareError::Api(api) = error else {
        return false;
    };
    if status > 0 && api.status_code == status {
        return true;
    }
    api.errors.iter().any(|item| codes.contains(&item.code))
}

#[derive(Debug, serde::Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    errors: Vec<CfApiError>,
    #[serde(default)]
    result: Option<T>,
}

/// A reusable Cloudflare API client. Cloning shares the connection pool.
#[derive(Clone)]
pub struct CloudflareClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl CloudflareClient {
    pub fn new(base_url: &str, token: &str) -> Result<Self, CloudflareError> {
        let base = base_url.trim().trim_end_matches('/');
        let base = if base.is_empty() {
            DEFAULT_API_BASE
        } else {
            base
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(CloudflareClient {
            base_url: base.to_string(),
            token: token.to_string(),
            http,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The API token, only read by request construction.
    fn authorization(&self) -> String {
        format!("Bearer {}", self.token)
    }

    async fn do_json<T: DeserializeOwned + Default>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Result<T, CloudflareError> {
        let mut request = self
            .http
            .request(method, format!("{}{}", self.base_url, path))
            .header(reqwest::header::AUTHORIZATION, self.authorization());
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(&body)?);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await?;
        let limited = &bytes[..bytes.len().min(4 << 20)];
        self.decode_response(status, limited)
    }

    fn decode_response<T: DeserializeOwned + Default>(
        &self,
        status: u16,
        raw: &[u8],
    ) -> Result<T, CloudflareError> {
        let envelope = if raw.is_empty() {
            None
        } else {
            match serde_json::from_slice::<Envelope<T>>(raw) {
                Ok(envelope) => Some(envelope),
                Err(error) => {
                    if status >= 400 {
                        return Err(CloudflareError::Api(CloudflareApiError {
                            status_code: status,
                            errors: Vec::new(),
                            body: String::from_utf8_lossy(raw).into_owned(),
                        }));
                    }
                    return Err(CloudflareError::Invalid(format!(
                        "decode Cloudflare response: {error}"
                    )));
                }
            }
        };
        let envelope = envelope.unwrap_or(Envelope {
            success: false,
            errors: Vec::new(),
            result: None,
        });
        if status >= 400 || !envelope.success {
            return Err(CloudflareError::Api(CloudflareApiError {
                status_code: status,
                errors: envelope.errors,
                body: String::from_utf8_lossy(raw).into_owned(),
            }));
        }
        Ok(envelope.result.unwrap_or_default())
    }

    async fn do_empty(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Result<(), CloudflareError> {
        let _: Value = self.do_json(method, path, body, headers).await?;
        Ok(())
    }

    /// `GET /accounts?per_page=20`, falling back to the first active zone's
    /// account.
    pub async fn discover_account_id(&self) -> Result<String, CloudflareError> {
        let accounts: Vec<AccountId> = self
            .do_json(reqwest::Method::GET, "/accounts?per_page=20", None, &[])
            .await?;
        if let Some(account) = accounts.first()
            && !account.id.is_empty()
        {
            return Ok(account.id.clone());
        }
        let zones: Vec<ZoneAccount> = self
            .do_json(
                reqwest::Method::GET,
                "/zones?status=active&per_page=50",
                None,
                &[],
            )
            .await?;
        if let Some(zone) = zones.iter().find(|zone| !zone.account.id.is_empty()) {
            return Ok(zone.account.id.clone());
        }
        Err(CloudflareError::Invalid(
            "no Cloudflare account found for API token; enter Account ID manually".into(),
        ))
    }

    /// `GET /accounts/{account}/workers/subdomain`; 404/10007 means none.
    pub async fn get_workers_subdomain(&self, account_id: &str) -> Result<String, CloudflareError> {
        let result = self
            .do_json::<Subdomain>(
                reqwest::Method::GET,
                &format!("/accounts/{}/workers/subdomain", path_escape(account_id)),
                None,
                &[],
            )
            .await;
        match result {
            Ok(value) => Ok(value.subdomain.trim().to_lowercase()),
            Err(error) if has_cf_error_code(&error, 404, &[10007]) => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    /// Active zones, lowercased and sorted by name.
    pub async fn list_zones(&self) -> Result<Vec<CloudflareZone>, CloudflareError> {
        let raw: Vec<ZoneId> = self
            .do_json(
                reqwest::Method::GET,
                "/zones?status=active&per_page=50",
                None,
                &[],
            )
            .await?;
        let mut zones: Vec<CloudflareZone> = raw
            .into_iter()
            .filter_map(|zone| {
                let name = zone.name.trim().to_lowercase();
                (!zone.id.is_empty() && !name.is_empty())
                    .then_some(CloudflareZone { id: zone.id, name })
            })
            .collect();
        zones.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(zones)
    }

    /// A/AAAA/CNAME records for one zone, deduplicated and sorted.
    pub async fn list_dns_records(
        &self,
        zone_id: &str,
    ) -> Result<Vec<CloudflareDnsRecord>, CloudflareError> {
        let raw: Vec<DnsRecord> = self
            .do_json(
                reqwest::Method::GET,
                &format!("/zones/{}/dns_records?per_page=100", path_escape(zone_id)),
                None,
                &[],
            )
            .await?;
        let mut seen = std::collections::BTreeSet::new();
        let mut records = Vec::new();
        for record in raw {
            let record_type = record.record_type.trim().to_uppercase();
            if !matches!(record_type.as_str(), "A" | "AAAA" | "CNAME") {
                continue;
            }
            let name = record.name.trim().to_lowercase();
            if name.is_empty() || !seen.insert(format!("{name}:{record_type}")) {
                continue;
            }
            records.push(CloudflareDnsRecord {
                id: record.id,
                zone_id: zone_id.to_string(),
                name,
                record_type,
            });
        }
        records.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.record_type.cmp(&b.record_type))
        });
        Ok(records)
    }

    /// The Worker's current migration tag; 404/10007/10092 means none.
    pub async fn get_script_migration_tag(
        &self,
        account_id: &str,
        script_name: &str,
    ) -> Result<String, CloudflareError> {
        let result = self
            .do_json::<WorkerService>(
                reqwest::Method::GET,
                &format!(
                    "/accounts/{}/workers/services/{}",
                    path_escape(account_id),
                    path_escape(script_name)
                ),
                None,
                &[],
            )
            .await;
        match result {
            Ok(value) => Ok(value
                .default_environment
                .script
                .migration_tag
                .trim()
                .to_string()),
            Err(error) if has_cf_error_code(&error, 404, &[10007, 10092]) => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    /// Deploy a Worker module with the Durable Object binding and migration.
    pub async fn put_worker_script(
        &self,
        account_id: &str,
        script_name: &str,
        tunnel_secret: &str,
        access_token: &str,
        existing_migration_tag: &str,
    ) -> Result<(), CloudflareError> {
        if !script_name.starts_with("devbase-") {
            return Err(CloudflareError::Invalid(format!(
                "refusing to deploy non-devbase worker {script_name:?}"
            )));
        }
        let mut bindings = vec![
            json!({
                "name": "TUNNEL_GATEWAY",
                "type": "durable_object_namespace",
                "class_name": "TunnelGateway",
            }),
            json!({
                "name": "TUNNEL_SECRET",
                "type": "secret_text",
                "text": tunnel_secret,
            }),
        ];
        if !access_token.is_empty() {
            bindings.push(json!({
                "name": "ACCESS_TOKEN",
                "type": "secret_text",
                "text": access_token,
            }));
        }
        let mut metadata = json!({
            "main_module": "index.mjs",
            "compatibility_date": WORKER_COMPAT_DATE,
            "bindings": bindings,
        });
        if existing_migration_tag != "v1" {
            let mut migration = json!({
                "new_tag": "v1",
                "steps": [{"new_sqlite_classes": ["TunnelGateway"]}],
            });
            if !existing_migration_tag.is_empty() {
                migration["old_tag"] = Value::String(existing_migration_tag.to_string());
            }
            metadata["migrations"] = migration;
        }

        let boundary = format!("devbase-{}", hex::encode(rand::random::<[u8; 16]>()));
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n",
        );
        body.extend_from_slice(&serde_json::to_vec(&metadata)?);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"index.mjs\"; filename=\"index.mjs\"\r\nContent-Type: application/javascript+module\r\n\r\n",
        );
        body.extend_from_slice(WORKER_INDEX_MJS.as_bytes());
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let response = self
            .http
            .request(
                reqwest::Method::PUT,
                format!(
                    "{}/accounts/{}/workers/scripts/{}",
                    self.base_url,
                    path_escape(account_id),
                    path_escape(script_name)
                ),
            )
            .header(reqwest::header::AUTHORIZATION, self.authorization())
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await?;
        let _: Value = self.decode_response(status, &bytes)?;
        Ok(())
    }

    /// Enable or disable the Worker's `*.workers.dev` route.
    pub async fn set_script_subdomain_enabled(
        &self,
        account_id: &str,
        script_name: &str,
        enabled: bool,
    ) -> Result<(), CloudflareError> {
        self.do_empty(
            reqwest::Method::POST,
            &format!(
                "/accounts/{}/workers/scripts/{}/subdomain",
                path_escape(account_id),
                path_escape(script_name)
            ),
            Some(json!({"enabled": enabled})),
            &[("Cloudflare-Workers-Script-Api-Date", SCRIPT_API_DATE)],
        )
        .await
    }

    /// Replace the Worker's Custom Domains; an empty hostname clears them.
    pub async fn set_script_custom_domains(
        &self,
        account_id: &str,
        script_name: &str,
        zone_id: &str,
        hostname: &str,
    ) -> Result<(), CloudflareError> {
        let origins = if hostname.is_empty() {
            Vec::new()
        } else {
            vec![json!({"hostname": hostname, "zone_id": zone_id})]
        };
        self.do_empty(
            reqwest::Method::PUT,
            &format!(
                "/accounts/{}/workers/scripts/{}/domains/records",
                path_escape(account_id),
                path_escape(script_name)
            ),
            Some(json!({
                "override_scope": true,
                "override_existing_origin": true,
                "override_existing_dns_record": true,
                "origins": origins,
            })),
            &[],
        )
        .await
    }

    /// Delete a `devbase-*` Worker after detaching Custom Domains.
    pub async fn delete_worker_script(
        &self,
        account_id: &str,
        script_name: &str,
    ) -> Result<(), CloudflareError> {
        if !script_name.starts_with("devbase-") {
            return Err(CloudflareError::Invalid(format!(
                "refusing to delete non-devbase worker {script_name:?}"
            )));
        }
        if let Err(error) = self
            .set_script_custom_domains(account_id, script_name, "", "")
            .await
            && !has_cf_error_code(&error, 404, &[10007])
        {
            return Err(error);
        }
        let result: Result<Value, CloudflareError> = self
            .do_json(
                reqwest::Method::DELETE,
                &format!(
                    "/accounts/{}/workers/scripts/{}?force=true",
                    path_escape(account_id),
                    path_escape(script_name)
                ),
                None,
                &[],
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) if has_cf_error_code(&error, 404, &[10007]) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// The longest active zone whose name is a suffix of `hostname`.
pub fn match_zone_for_hostname<'a>(
    hostname: &str,
    zones: &'a [CloudflareZone],
) -> Option<&'a CloudflareZone> {
    let host = hostname.trim().to_lowercase();
    zones
        .iter()
        .filter(|zone| {
            let name = zone.name.trim().to_lowercase();
            !name.is_empty() && (host == name || host.ends_with(&format!(".{name}")))
        })
        .max_by_key(|zone| zone.name.len())
}

fn path_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[derive(Default, serde::Deserialize)]
struct AccountId {
    #[serde(default)]
    id: String,
}

#[derive(Default, serde::Deserialize)]
struct ZoneAccount {
    #[serde(default)]
    account: AccountId,
}

#[derive(Default, serde::Deserialize)]
struct ZoneId {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Default, serde::Deserialize)]
struct DnsRecord {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    record_type: String,
}

#[derive(Default, serde::Deserialize)]
struct Subdomain {
    #[serde(default)]
    subdomain: String,
}

#[derive(Default, serde::Deserialize)]
struct WorkerService {
    #[serde(default)]
    default_environment: DefaultEnvironment,
}

#[derive(Default, serde::Deserialize)]
struct DefaultEnvironment {
    #[serde(default)]
    script: ScriptInfo,
}

#[derive(Default, serde::Deserialize)]
struct ScriptInfo {
    #[serde(default)]
    migration_tag: String,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use http::{HeaderMap, Request, Response, StatusCode};
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Debug, Clone)]
    struct Recorded {
        method: String,
        path: String,
        headers: HeaderMap,
        body: Vec<u8>,
    }

    struct FakeCloudflare {
        base_url: String,
        recorded: Arc<Mutex<Vec<Recorded>>>,
    }

    async fn fake_server(routes: BTreeMap<(String, String), (u16, String)>) -> FakeCloudflare {
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
                            let headers = request.headers().clone();
                            let body = request
                                .into_body()
                                .collect()
                                .await
                                .map(|collected| collected.to_bytes().to_vec())
                                .unwrap_or_default();
                            recorded.lock().unwrap().push(Recorded {
                                method: method.clone(),
                                path: path.clone(),
                                headers,
                                body,
                            });
                            let (status, body) = routes
                                .get(&(method, path))
                                .cloned()
                                .unwrap_or_else(|| {
                                    (
                                        404,
                                        json!({"success":false,"errors":[{"code":10007,"message":"not found"}],"result":null})
                                            .to_string(),
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
        FakeCloudflare {
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

    #[tokio::test]
    async fn metadata_and_worker_requests_match_the_go_shapes() {
        let mut routes = BTreeMap::new();
        route(
            &mut routes,
            "GET",
            "/accounts?per_page=20",
            success(json!([{"id": "acct-1234", "name": "Me"}])),
        );
        route(
            &mut routes,
            "GET",
            "/accounts/acct-1234/workers/subdomain",
            success(json!({"subdomain": "Example"})),
        );
        route(
            &mut routes,
            "GET",
            "/zones?status=active&per_page=50",
            success(json!([
                {"id": "zone-b", "name": "B.Example"},
                {"id": "zone-a", "name": "a.example"},
                {"id": "zone-empty", "name": ""}
            ])),
        );
        route(
            &mut routes,
            "GET",
            "/zones/zone-a/dns_records?per_page=100",
            success(json!([
                {"id": "1", "name": "WWW.A.example", "type": "a"},
                {"id": "2", "name": "www.a.example", "type": "A"},
                {"id": "3", "name": "mail.a.example", "type": "MX"},
                {"id": "4", "name": "a.example", "type": "CNAME"}
            ])),
        );
        route(
            &mut routes,
            "GET",
            "/accounts/acct-1234/workers/services/devbase-app",
            success(json!({"default_environment":{"script":{"migration_tag":"v1"}}})),
        );
        route(
            &mut routes,
            "PUT",
            "/accounts/acct-1234/workers/scripts/devbase-app",
            success(json!({})),
        );
        route(
            &mut routes,
            "POST",
            "/accounts/acct-1234/workers/scripts/devbase-app/subdomain",
            success(json!({})),
        );
        route(
            &mut routes,
            "PUT",
            "/accounts/acct-1234/workers/scripts/devbase-app/domains/records",
            success(json!({})),
        );
        route(
            &mut routes,
            "DELETE",
            "/accounts/acct-1234/workers/scripts/devbase-app?force=true",
            success(json!({})),
        );

        let fake = fake_server(routes).await;
        let client = CloudflareClient::new(&fake.base_url, "token-abcdefghij").unwrap();
        assert_eq!(client.discover_account_id().await.unwrap(), "acct-1234");
        assert_eq!(
            client.get_workers_subdomain("acct-1234").await.unwrap(),
            "example"
        );
        let zones = client.list_zones().await.unwrap();
        assert_eq!(
            zones
                .iter()
                .map(|zone| zone.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a.example", "b.example"]
        );
        let records = client.list_dns_records("zone-a").await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "a.example");
        assert_eq!(records[1].record_type, "A");

        assert_eq!(
            client
                .get_script_migration_tag("acct-1234", "devbase-app")
                .await
                .unwrap(),
            "v1"
        );
        client
            .put_worker_script("acct-1234", "devbase-app", "secret", "edge-token", "v1")
            .await
            .unwrap();
        assert!(
            client
                .put_worker_script("acct-1234", "other-app", "s", "", "")
                .await
                .unwrap_err()
                .to_string()
                .contains("refusing to deploy")
        );
        client
            .set_script_subdomain_enabled("acct-1234", "devbase-app", true)
            .await
            .unwrap();
        client
            .set_script_custom_domains("acct-1234", "devbase-app", "zone-a", "app.example")
            .await
            .unwrap();
        client
            .delete_worker_script("acct-1234", "devbase-app")
            .await
            .unwrap();

        let recorded = fake.recorded.lock().unwrap().clone();
        let put = recorded
            .iter()
            .find(|request| {
                request.method == "PUT" && request.path.ends_with("/scripts/devbase-app")
            })
            .unwrap();
        assert_eq!(
            put.headers["authorization"], "Bearer token-abcdefghij",
            "token goes only in the Authorization header"
        );
        let content_type = put.headers["content-type"].to_str().unwrap();
        assert!(content_type.starts_with("multipart/form-data; boundary=devbase-"));
        let body = String::from_utf8_lossy(&put.body);
        assert!(body.contains("name=\"metadata\""));
        assert!(body.contains("Content-Type: application/json"));
        assert!(body.contains("name=\"index.mjs\"; filename=\"index.mjs\""));
        assert!(body.contains("Content-Type: application/javascript+module"));
        let metadata_start = body.find("\r\n\r\n").unwrap() + 4;
        let metadata_end = body[metadata_start..].find("\r\n").unwrap() + metadata_start;
        let metadata: Value = serde_json::from_str(&body[metadata_start..metadata_end]).unwrap();
        assert_eq!(metadata["main_module"], "index.mjs");
        assert_eq!(metadata["compatibility_date"], WORKER_COMPAT_DATE);
        assert!(
            metadata.get("migrations").is_none(),
            "v1 needs no migration"
        );
        assert_eq!(metadata["bindings"][0]["name"], "TUNNEL_GATEWAY");
        assert_eq!(metadata["bindings"][0]["type"], "durable_object_namespace");
        assert_eq!(metadata["bindings"][1]["name"], "TUNNEL_SECRET");
        assert_eq!(metadata["bindings"][1]["text"], "secret");
        assert!(body.contains(WORKER_INDEX_MJS.trim_end()));

        let subdomain = recorded
            .iter()
            .find(|request| request.method == "POST")
            .unwrap();
        assert_eq!(
            subdomain.headers["cloudflare-workers-script-api-date"],
            SCRIPT_API_DATE
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&subdomain.body).unwrap(),
            json!({"enabled": true})
        );

        let domains = recorded
            .iter()
            .find(|request| request.method == "PUT" && request.path.ends_with("/domains/records"))
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&domains.body).unwrap(),
            json!({
                "override_scope": true,
                "override_existing_origin": true,
                "override_existing_dns_record": true,
                "origins": [{"hostname": "app.example", "zone_id": "zone-a"}]
            })
        );

        let deletes: Vec<&Recorded> = recorded
            .iter()
            .filter(|request| request.method == "DELETE")
            .collect();
        assert_eq!(deletes.len(), 1);
        assert_eq!(
            deletes[0].path,
            "/accounts/acct-1234/workers/scripts/devbase-app?force=true"
        );
    }

    #[tokio::test]
    async fn missing_resources_are_tolerated_and_errors_are_typed() {
        let mut routes = BTreeMap::new();
        route(
            &mut routes,
            "GET",
            "/accounts/acct/workers/services/devbase-app",
            (
                404,
                json!({"success":false,"errors":[{"code":10007,"message":"gone"}]}).to_string(),
            ),
        );
        let fake = fake_server(routes).await;
        let client = CloudflareClient::new(&fake.base_url, "token").unwrap();
        assert_eq!(
            client
                .get_script_migration_tag("acct", "devbase-app")
                .await
                .unwrap(),
            ""
        );
        let error = client.list_zones().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Cloudflare API error (404): not found (code 10007)"
        );
        assert!(has_cf_error_code(&error, 404, &[]));
        assert!(has_cf_error_code(&error, 0, &[10007]));
    }

    #[tokio::test]
    async fn migration_is_sent_only_for_older_tags() {
        let mut routes = BTreeMap::new();
        route(
            &mut routes,
            "PUT",
            "/accounts/acct/workers/scripts/devbase-app",
            success(json!({})),
        );
        let fake = fake_server(routes).await;
        let client = CloudflareClient::new(&fake.base_url, "token").unwrap();
        client
            .put_worker_script("acct", "devbase-app", "secret", "", "old-tag")
            .await
            .unwrap();
        let recorded = fake.recorded.lock().unwrap();
        let body = String::from_utf8_lossy(&recorded[0].body);
        let start = body.find("\r\n\r\n").unwrap() + 4;
        let end = body[start..].find("\r\n").unwrap() + start;
        let metadata: Value = serde_json::from_str(&body[start..end]).unwrap();
        assert_eq!(metadata["migrations"]["new_tag"], "v1");
        assert_eq!(metadata["migrations"]["old_tag"], "old-tag");
        assert_eq!(
            metadata["migrations"]["steps"][0]["new_sqlite_classes"][0],
            "TunnelGateway"
        );
        assert_eq!(metadata["bindings"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn zone_matching_uses_the_longest_suffix() {
        let zones = vec![
            CloudflareZone {
                id: "1".into(),
                name: "example.com".into(),
            },
            CloudflareZone {
                id: "2".into(),
                name: "api.example.com".into(),
            },
        ];
        assert_eq!(
            match_zone_for_hostname("x.api.example.com", &zones)
                .unwrap()
                .id,
            "2"
        );
        assert_eq!(
            match_zone_for_hostname("app.example.com", &zones)
                .unwrap()
                .id,
            "1"
        );
        assert!(match_zone_for_hostname("notexample.com", &zones).is_none());
        assert!(match_zone_for_hostname("example.com.evil", &zones).is_none());
    }

    #[test]
    fn worker_digest_is_sixteen_hex_and_stable() {
        let digest = current_worker_digest();
        assert_eq!(digest.len(), 16);
        assert!(
            digest
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
        assert_eq!(digest, current_worker_digest());
        assert!(WORKER_INDEX_MJS.contains("TunnelGateway"));
        assert!(WORKER_INDEX_MJS.contains("__devbase/connect"));
    }
}
