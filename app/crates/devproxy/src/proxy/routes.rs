//! The live routing table watched by SHA-256.
//!
//! Ported from `proxy_routes.go`. Every request asks for the current snapshot;
//! the table re-reads `proxy/rules.json`, compares its digest, and only swaps
//! the table after the whole document validates. A missing or corrupt snapshot
//! leaves the last good routes and upstream transports serving.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::rules::{
    ProxyRoute, PublishedProxyConfig, listener_ports, parse_published_config,
    published_config_path, routes_from_config, validate_proxy_routes,
};

/// One reverse route. The route is immutable; unchanged routes are reused when
/// a new snapshot is built, so their connections and state survive reloads.
#[derive(Debug)]
pub struct Route {
    pub site: ProxyRoute,
}

/// An immutable routing snapshot.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub sites: Vec<ProxyRoute>,
    pub routes: BTreeMap<String, Arc<Route>>,
}

impl Snapshot {
    /// Look up a route by lowercased domain and listener port.
    pub fn route(&self, domain: &str, listener_port: i64) -> Option<&Arc<Route>> {
        self.routes.get(&format!("{domain}:{listener_port}"))
    }

    pub fn listener_ports(&self) -> Vec<i64> {
        listener_ports(&self.sites)
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

struct Inner {
    digest: Option<[u8; 32]>,
    snapshot: Arc<Snapshot>,
}

/// The digest-watched live table.
pub struct LiveRoutes {
    root: PathBuf,
    inner: Mutex<Inner>,
}

impl LiveRoutes {
    /// Build the initial table from already validated routes.
    pub fn new(root: impl Into<PathBuf>, sites: Vec<ProxyRoute>) -> Self {
        let snapshot = build_snapshot(&sites, None);
        LiveRoutes {
            root: root.into(),
            inner: Mutex::new(Inner {
                digest: None,
                snapshot: Arc::new(snapshot),
            }),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the current snapshot, reloading `proxy/rules.json` first when its
    /// digest changed. Reload failures retain the previous table.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        let path = published_config_path(&self.root);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => return self.current(),
        };
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        {
            let inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if inner.digest == Some(digest) {
                return Arc::clone(&inner.snapshot);
            }
        }
        let config = match parse_published_config(&bytes) {
            Ok(config) => config,
            Err(_) => return self.current(),
        };
        let sites = routes_from_config(&config);
        if validate_proxy_routes(&sites).is_err() {
            return self.current();
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A concurrent reload may have installed the same digest already.
        if inner.digest == Some(digest) {
            return Arc::clone(&inner.snapshot);
        }
        let next = build_snapshot(&sites, Some(&inner.snapshot));
        inner.snapshot = Arc::new(next);
        inner.digest = Some(digest);
        Arc::clone(&inner.snapshot)
    }

    fn current(&self) -> Arc<Snapshot> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(&inner.snapshot)
    }

    /// Install a snapshot directly (tests and the initial server build).
    pub fn install(&self, sites: Vec<ProxyRoute>) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.snapshot = Arc::new(build_snapshot(&sites, Some(&inner.snapshot)));
        inner.digest = None;
    }
}

fn build_snapshot(sites: &[ProxyRoute], old: Option<&Snapshot>) -> Snapshot {
    let mut routes = BTreeMap::new();
    for site in sites {
        let id = site.route_id();
        let reuse = old.and_then(|old| old.routes.get(&id)).filter(|route| {
            route.site.local_port == site.local_port
                && route.site.upstream_host == site.upstream_host
        });
        let route = match reuse {
            Some(route) => Arc::clone(route),
            None => Arc::new(Route { site: site.clone() }),
        };
        routes.insert(id, route);
    }
    Snapshot {
        sites: sites.to_vec(),
        routes,
    }
}

/// Build a snapshot from a validated configuration.
pub fn snapshot_from_config(config: &PublishedProxyConfig) -> Snapshot {
    build_snapshot(&routes_from_config(config), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::rules::write_published_config;
    use devcore::model::ProxyRule;
    use tempfile::TempDir;

    fn config(port: i64, rules: Vec<ProxyRule>) -> PublishedProxyConfig {
        PublishedProxyConfig {
            schema_version: 1,
            listener_port: port,
            rules,
        }
    }

    fn rule(domain: &str, port: i64) -> ProxyRule {
        ProxyRule {
            domain: domain.into(),
            local_port: port,
            ..ProxyRule::default()
        }
    }

    #[test]
    fn digest_change_swaps_the_table() {
        let dir = TempDir::new().unwrap();
        let initial = crate::proxy::rules::routes_from_config(&config(
            443,
            vec![rule("a.devbase.localhost", 3000)],
        ));
        write_published_config(
            dir.path(),
            &config(443, vec![rule("a.devbase.localhost", 3000)]),
        )
        .unwrap();
        let live = LiveRoutes::new(dir.path(), initial);
        let first = live.snapshot();
        assert!(first.route("a.devbase.localhost", 443).is_some());
        assert!(
            Arc::ptr_eq(&first, &live.snapshot()),
            "digest hit reuses the table"
        );

        write_published_config(
            dir.path(),
            &config(
                443,
                vec![
                    rule("a.devbase.localhost", 3000),
                    rule("b.devbase.localhost", 4000),
                ],
            ),
        )
        .unwrap();
        let second = live.snapshot();
        assert!(second.route("b.devbase.localhost", 443).is_some());
        assert!(
            Arc::ptr_eq(
                first.route("a.devbase.localhost", 443).unwrap(),
                second.route("a.devbase.localhost", 443).unwrap()
            ),
            "unchanged routes are reused"
        );
    }

    #[test]
    fn corrupt_snapshot_keeps_last_good_routes() {
        let dir = TempDir::new().unwrap();
        write_published_config(
            dir.path(),
            &config(443, vec![rule("a.devbase.localhost", 3000)]),
        )
        .unwrap();
        let live = LiveRoutes::new(dir.path(), Vec::new());
        let good = live.snapshot();
        assert!(good.route("a.devbase.localhost", 443).is_some());

        std::fs::write(published_config_path(dir.path()), b"{not json").unwrap();
        let after = live.snapshot();
        assert!(Arc::ptr_eq(&good, &after));

        // A semantically invalid but well-formed snapshot is also ignored.
        let invalid = serde_json::json!({
            "schemaVersion": 1,
            "listenerPort": 443,
            "rules": [{"domain": "elsewhere.example", "localPort": 3000}]
        });
        std::fs::write(
            published_config_path(dir.path()),
            serde_json::to_vec(&invalid).unwrap(),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&good, &live.snapshot()));
        assert!(after.route("a.devbase.localhost", 443).is_some());
    }

    #[test]
    fn deleted_snapshot_keeps_the_table() {
        let dir = TempDir::new().unwrap();
        write_published_config(
            dir.path(),
            &config(443, vec![rule("a.devbase.localhost", 3000)]),
        )
        .unwrap();
        let live = LiveRoutes::new(dir.path(), Vec::new());
        let good = live.snapshot();
        std::fs::remove_file(published_config_path(dir.path())).unwrap();
        assert!(Arc::ptr_eq(&good, &live.snapshot()));
    }
}
