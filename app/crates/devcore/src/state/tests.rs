//! Tests for the state store, schema validation, paths and leases.
//!
//! Every test works in a `tempfile::TempDir` root and supplies an explicit
//! home, so no test can reach the real development home or a real user
//! directory. Locking is exercised with real files and real threads; it is
//! never mocked.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::fs::{digest_bytes, read_regular_file};
use crate::model::{
    CloudflareDnsRecord, CloudflareZone, ConnectionProfile, Instance, Package, ProxyRule, State,
    TunnelWorkerState, homebrew_binaries, validate_connection_profile,
};

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    store: Store,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dev");
    let home = dir.path().join("home");
    let store = Store::new(&root).with_home(&home);
    Fixture {
        _dir: dir,
        root,
        store,
    }
}

fn write_raw(f: &Fixture, bytes: &[u8]) {
    std::fs::create_dir_all(&f.root).unwrap();
    std::fs::write(f.root.join("state.json"), bytes).unwrap();
}

fn read_raw(f: &Fixture, raw: &str) -> Result<State, StateError> {
    write_raw(f, raw.as_bytes());
    f.store.read()
}

fn error_text(result: Result<State, StateError>) -> String {
    match result {
        Ok(_) => panic!("state unexpectedly accepted"),
        Err(error) => error.to_string(),
    }
}

fn base_state_json() -> serde_json::Value {
    json!({
        "schemaVersion": 2,
        "packages": {},
        "instances": {},
        "proxyRules": [],
        "proxyListenerPort": 443,
    })
}

fn valid_profile_fixture() -> serde_json::Value {
    let mut value = base_state_json();
    value["instances"] = json!({
        "blog": {"id": "blog", "kind": "postgresql", "package": "node@22.14.0-arm64", "port": 5432, "persistent": true}
    });
    value["connectionProfiles"] = json!({
        "blog-pg": {"id": "blog-pg", "instanceId": "blog", "name": "Blog", "database": "blog", "username": "devmanager"}
    });
    value
}

fn sample_package() -> Package {
    Package {
        name: "node".into(),
        version: "22.14.0".into(),
        arch: crate::platform::HostTarget::current().arch_name().into(),
        url: "https://nodejs.org/dist/v22.14.0/node.zip".into(),
        sha256: digest_bytes(b"archive"),
        format: "zip".into(),
        binaries: BTreeMap::from([("node".into(), "bin/node".into())]),
        license: "MIT".into(),
        ..Default::default()
    }
}

fn sample_state() -> State {
    let mut state = State::default();
    let package = sample_package();
    let package_id = package.id();
    state.packages.insert(package_id.clone(), package);
    state.instances.insert(
        "blog".into(),
        Instance {
            id: "blog".into(),
            kind: "postgresql".into(),
            package: package_id,
            port: 5432,
            persistent: true,
            ..Default::default()
        },
    );
    state.connection_profiles.insert(
        "blog-pg".into(),
        ConnectionProfile {
            id: "blog-pg".into(),
            instance_id: "blog".into(),
            name: "Blog".into(),
            database: "blog".into(),
            username: "devmanager".into(),
            ..Default::default()
        },
    );
    state.proxy_rules = vec![ProxyRule {
        domain: "app.devbase.localhost".into(),
        local_port: 3000,
        tunnel: true,
        tunnel_worker: "devbase-app".into(),
        tunnel_domain: "app.example.com".into(),
        ..Default::default()
    }];
    state.proxy_rules_revision = 7;
    state
        .runtime_defaults
        .insert("node".into(), "node@22.14.0-arm64".into());
    state.uv_executable = "/Users/dev/.local/bin/uv".into();
    state.cloudflare_account_id = "0123456789abcdef".into();
    state.cloudflare_subdomain = "acme".into();
    state.cloudflare_zones = vec![CloudflareZone {
        id: "z1".into(),
        name: "example.com".into(),
    }];
    state.cloudflare_dns = vec![CloudflareDnsRecord {
        zone_id: "z1".into(),
        name: "app.example.com".into(),
        record_type: "CNAME".into(),
        ..Default::default()
    }];
    state.tunnel_client_id = "client-01".into();
    state.tunnel_workers.insert(
        "devbase-app".into(),
        TunnelWorkerState {
            script_name: "devbase-app".into(),
            public_url: "https://app.example.com".into(),
            local_domain: "app.devbase.localhost".into(),
            local_port: 3000,
            ..Default::default()
        },
    );
    state
        .package_integrity
        .insert("node@22.14.0-arm64".into(), digest_bytes(b"index"));
    state.node_default = "node@22.14.0-arm64".into();
    state.log_rotation_disabled = true;
    state
}

#[test]
fn fresh_workspace_has_complete_defaults() {
    let f = fixture();
    let state = f.store.read().unwrap();
    assert_eq!(state, State::default());
    assert_eq!(state.schema_version, 2);
    assert_eq!(state.proxy_listener_port, 443);
    assert!(state.packages.is_empty());
    assert!(state.instances.is_empty());
    assert!(state.proxy_rules.is_empty());
    assert!(state.connection_profiles.is_empty());

    f.store.init().unwrap();
    for directory in [
        "",
        "packages",
        "instances",
        "jobs",
        "logs",
        "proxy",
        "backups",
        "integrity",
        "package-locks",
        "downloads",
    ] {
        assert!(
            f.root.join(directory).is_dir(),
            "missing {}",
            f.root.join(directory).display()
        );
    }
    assert!(!f.root.join("state.json").exists());
}

#[test]
fn older_schema_is_rejected_and_bytes_are_untouched() {
    let f = fixture();
    let old = br#"{"schemaVersion":1,"packages":{},"instances":{}}"#;
    write_raw(&f, old);

    let error = error_text(f.store.read());
    assert!(
        error.contains("saved settings use unsupported format 1 (expected 2)"),
        "{error}"
    );
    assert!(
        error.contains("existing files were left unchanged"),
        "{error}"
    );

    let mutation = f.store.with_state(|state| {
        state.proxy_listener_port = 8443;
        Ok(())
    });
    assert!(mutation.is_err());
    assert_eq!(std::fs::read(f.root.join("state.json")).unwrap(), old);

    write_raw(&f, br#"{"schemaVersion":3,"packages":{},"instances":{}}"#);
    assert!(error_text(f.store.read()).contains("unsupported format 3"));
}

#[test]
fn unknown_and_incomplete_state_is_rejected() {
    let f = fixture();

    let mut unknown_top = base_state_json();
    unknown_top["unexpected"] = json!(true);
    assert!(read_raw(&f, &unknown_top.to_string()).is_err());

    let mut unknown_nested = base_state_json();
    unknown_nested["packages"] = json!({
        "node@22.14.0-arm64": {"name": "node", "version": "22.14.0", "arch": "arm64", "unexpected": 1}
    });
    assert!(read_raw(&f, &unknown_nested.to_string()).is_err());

    let mut unknown_rule = base_state_json();
    unknown_rule["proxyRules"] = json!([
        {"domain": "a.devbase.localhost", "localPort": 3000, "unexpected": 1}
    ]);
    assert!(read_raw(&f, &unknown_rule.to_string()).is_err());

    let mut unknown_instance = base_state_json();
    unknown_instance["instances"] =
        json!({"blog": {"id": "blog", "kind": "postgresql", "unexpected": 1}});
    assert!(read_raw(&f, &unknown_instance.to_string()).is_err());

    for raw in [
        // proxyRules missing
        r#"{"schemaVersion":2,"packages":{},"instances":{},"proxyListenerPort":443}"#,
        // listener out of range
        r#"{"schemaVersion":2,"packages":{},"instances":{},"proxyRules":[],"proxyListenerPort":0}"#,
        // required collections null / missing
        r#"{"schemaVersion":2,"packages":null,"instances":{},"proxyRules":[],"proxyListenerPort":443}"#,
        r#"{"schemaVersion":2,"packages":{},"proxyRules":[],"proxyListenerPort":443}"#,
        r#"{"schemaVersion":2,"packages":{},"instances":{},"proxyRules":null,"proxyListenerPort":443}"#,
        // trailing document
        r#"{"schemaVersion":2,"packages":{},"instances":{},"proxyRules":[],"proxyListenerPort":443}{}"#,
    ] {
        assert!(read_raw(&f, raw).is_err(), "accepted {raw}");
    }
}

#[test]
fn state_round_trips_pretty_private_and_without_secrets() {
    let f = fixture();
    let state = sample_state();
    f.store.write_state(&state).unwrap();

    let path = f.root.join("state.json");
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.ends_with(b"\n"));
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("\n  \"schemaVersion\": 2"), "{text}");
    assert!(!text.contains("password"));
    assert!(!text.contains("token"));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let leftovers: Vec<_> = std::fs::read_dir(&f.root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".write-"))
        .collect();
    assert!(leftovers.is_empty(), "leaked temp files: {leftovers:?}");

    assert_eq!(f.store.read().unwrap(), state);
}

#[test]
fn concurrent_writers_do_not_lose_updates() {
    let f = fixture();
    f.store.init().unwrap();
    let stop = Arc::new(AtomicBool::new(false));

    let reader_store = f.store.clone();
    let reader_stop = stop.clone();
    let reader = std::thread::spawn(move || {
        let mut reads = 0u32;
        while !reader_stop.load(Ordering::Relaxed) {
            let state = reader_store
                .read()
                .expect("a rename never exposes a partial file");
            assert_eq!(state.schema_version, 2);
            reads += 1;
        }
        reads
    });

    let writers: Vec<_> = (0..2)
        .map(|thread| {
            let store = f.store.clone();
            std::thread::spawn(move || {
                for index in 0..25 {
                    store
                        .with_state(|state| {
                            let name = format!("p{thread}-{index}");
                            let package = Package {
                                name: name.clone(),
                                version: "1.0".into(),
                                arch: "arm64".into(),
                                ..Default::default()
                            };
                            state.packages.insert(package.id(), package);
                            Ok(())
                        })
                        .unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    assert!(reader.join().unwrap() > 0);

    let state = f.store.read().unwrap();
    assert_eq!(state.packages.len(), 50);
    for thread in 0..2 {
        for index in 0..25 {
            assert!(
                state
                    .packages
                    .contains_key(&format!("p{thread}-{index}@1.0-arm64"))
            );
        }
    }
}

#[test]
fn exclusive_package_leases_report_busy_and_shared_leases_wait() {
    let f = fixture();
    let package = "node@22.14.0-arm64";
    let exclusive = f.store.package_lease(package, true).unwrap();

    let error = f.store.package_lease(package, true).err().unwrap();
    match &error {
        StateError::PackageBusy(busy) => assert_eq!(busy.package_id, package),
        other => panic!("expected package busy, got {other}"),
    }
    assert!(error.is_busy());

    // A shared lease waits while the exclusive one is held.
    let waiting_store = f.store.clone();
    let (tx, rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let lease = waiting_store.package_lease(package, false);
        tx.send(lease.is_ok()).unwrap();
    });
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    drop(exclusive);
    assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    waiter.join().unwrap();

    // Two shared leases coexist.
    let first = f.store.package_lease(package, false).unwrap();
    let second = f.store.package_lease(package, false).unwrap();
    drop((first, second));

    // A shared lease blocks a later exclusive acquisition.
    let shared = f.store.package_lease(package, false).unwrap();
    assert!(matches!(
        f.store.package_lease(package, true),
        Err(StateError::PackageBusy(_))
    ));
    drop(shared);

    assert!(f.store.package_lease("no-at-sign", true).is_err());
    assert!(f.store.package_lease("../escape@1-arm64", true).is_err());
}

#[test]
fn resource_leases_are_try_only_sorted_and_deduplicated() {
    let f = fixture();
    let blog = ResourceKey::new("database instance", "blog");
    let port = ResourceKey::new("TCP port", "5432");
    let lease = f
        .store
        .resource_leases(&[port.clone(), blog.clone(), blog.clone()])
        .unwrap();

    let error = f
        .store
        .resource_leases(std::slice::from_ref(&blog))
        .err()
        .unwrap();
    match &error {
        StateError::ResourceBusy(busy) => {
            assert_eq!(busy.kind, "database instance");
            assert_eq!(busy.id, "blog");
        }
        other => panic!("expected resource busy, got {other}"),
    }
    assert!(error.is_busy());

    // A different resource is independent.
    f.store
        .resource_leases(&[ResourceKey::new("TCP port", "5433")])
        .unwrap();
    drop(lease);
    f.store.resource_leases(&[blog]).unwrap();
}

#[test]
fn database_work_guard_is_consulted_for_instance_leases() {
    let f = fixture();
    let checked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let log = checked.clone();
    let guarded = f
        .store
        .clone()
        .with_database_work_check(Arc::new(move |id| {
            log.lock().unwrap().push(id.to_string());
            Err(StateError::invalid(format!(
                "unfinished database task for {id}"
            )))
        }));
    let key = ResourceKey::new("database instance", "blog");
    let error = guarded
        .resource_leases(std::slice::from_ref(&key))
        .err()
        .unwrap();
    assert!(error.to_string().contains("unfinished database task"));
    assert_eq!(checked.lock().unwrap().as_slice(), ["blog"]);
    // The failed acquisition released the flock.
    f.store.resource_leases(&[key]).unwrap();
}

#[test]
fn read_regular_file_refuses_symlinks_and_enforces_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("small"), b"hello").unwrap();
    assert_eq!(read_regular_file(dir.path(), "small", 5).unwrap(), b"hello");
    assert!(read_regular_file(dir.path(), "small", 4).is_err());
    assert!(read_regular_file(dir.path(), "missing", 10).is_err());

    std::os::unix::fs::symlink(dir.path().join("small"), dir.path().join("link")).unwrap();
    let error = read_regular_file(dir.path(), "link", 10).unwrap_err();
    assert!(error.to_string().contains("regular file"), "{error}");

    std::fs::create_dir(dir.path().join("folder")).unwrap();
    assert!(read_regular_file(dir.path(), "folder", 10).is_err());

    // Empty files are fine.
    std::fs::write(dir.path().join("empty"), b"").unwrap();
    assert!(
        read_regular_file(dir.path(), "empty", 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn user_storage_locations_are_used_only_for_the_default_root() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = default_storage_root(&home);
    let user_store = Store::new(&root).with_home(&home);
    let expected_cache = crate::platform::paths::user_cache_directory(&home);
    let expected_java = crate::platform::paths::user_java_directory(&home);
    assert!(user_store.uses_user_storage());
    assert_eq!(user_store.cache_directory(), expected_cache);
    assert_eq!(
        user_store.download_directory(),
        expected_cache.join("downloads")
    );
    assert_eq!(user_store.java_directory(), expected_java);

    let java = Package {
        name: "java".into(),
        version: "21.0.13_9".into(),
        arch: crate::platform::HostTarget::current().arch_name().into(),
        ..Default::default()
    };
    assert_eq!(
        user_store.package_directory(&java),
        expected_java.join("temurin-21.0.13")
    );
    user_store.init().unwrap();
    assert!(user_store.download_directory().is_dir());
    assert!(!root.join("downloads").exists());

    let isolated = Store::new(dir.path().join("isolated")).with_home(&home);
    assert!(!isolated.uses_user_storage());
    assert_eq!(isolated.cache_directory(), isolated.root().join("cache"));
    assert_eq!(
        isolated.download_directory(),
        isolated.root().join("downloads")
    );
    assert_eq!(
        isolated.java_directory(),
        isolated.root().join("JavaVirtualMachines")
    );

    let node = Package {
        name: "node".into(),
        version: "22.14.0".into(),
        arch: "arm64".into(),
        ..Default::default()
    };
    assert_eq!(
        isolated.package_directory(&node),
        isolated.root().join("packages/node@22.14.0-arm64")
    );

    let brew = Package {
        name: "postgresql".into(),
        version: "17.2".into(),
        arch: "arm64".into(),
        formula: "postgresql@17".into(),
        ..Default::default()
    };
    let cellar = dir.path().join("cellar");
    let cellar_store = isolated.with_brew_cellar(&cellar);
    assert_eq!(
        cellar_store.package_directory(&brew),
        cellar.join("postgresql@17").join("17.2")
    );
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn default_root_honours_the_dev_home_override() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let previous = std::env::var_os(DEV_HOME_ENV);
    let previous_home = std::env::var_os("HOME");

    // SAFETY: the test serializes environment mutation with ENV_LOCK.
    unsafe {
        std::env::set_var(DEV_HOME_ENV, dir.path());
    }
    assert_eq!(default_root(), dir.path());

    let fake_home = dir.path().join("fake-home");
    unsafe {
        std::env::remove_var(DEV_HOME_ENV);
        std::env::set_var("HOME", &fake_home);
    }
    assert_eq!(default_root(), default_storage_root(&fake_home));

    unsafe {
        std::env::set_var(DEV_HOME_ENV, "");
    }
    assert_eq!(default_root(), default_storage_root(&fake_home));

    unsafe {
        match previous {
            Some(value) => std::env::set_var(DEV_HOME_ENV, value),
            None => std::env::remove_var(DEV_HOME_ENV),
        }
        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[test]
fn secrets_never_enter_the_state_document() {
    let f = fixture();

    let mut top = valid_profile_fixture();
    top["password"] = json!("sentinel");
    assert!(read_raw(&f, &top.to_string()).is_err());

    let mut top = valid_profile_fixture();
    top["cloudflareToken"] = json!("sentinel");
    assert!(read_raw(&f, &top.to_string()).is_err());

    let mut profile = valid_profile_fixture();
    profile["connectionProfiles"]["blog-pg"]["password"] = json!("sentinel");
    assert!(read_raw(&f, &profile.to_string()).is_err());

    let mut worker = base_state_json();
    worker["tunnelWorkers"] = json!({
        "devbase-app": {
            "scriptName": "devbase-app",
            "publicUrl": "https://app.example.com",
            "localDomain": "app.devbase.localhost",
            "localPort": 3000,
            "token": "sentinel"
        }
    });
    assert!(read_raw(&f, &worker.to_string()).is_err());
}

#[test]
fn connection_profiles_are_validated_per_engine() {
    let f = fixture();

    // The fixture's PostgreSQL profile is valid.
    let valid = valid_profile_fixture();
    assert!(read_raw(&f, &valid.to_string()).is_ok());

    let mut wrong_key = valid_profile_fixture();
    wrong_key["connectionProfiles"] = json!({
        "other": {"id": "blog-pg", "instanceId": "blog", "name": "Blog", "database": "blog", "username": "devmanager"}
    });
    assert!(read_raw(&f, &wrong_key.to_string()).is_err());

    let mut missing_instance = valid_profile_fixture();
    missing_instance["connectionProfiles"]["blog-pg"]["instanceId"] = json!("gone");
    assert!(read_raw(&f, &missing_instance.to_string()).is_err());

    let mut pg_with_mongo = valid_profile_fixture();
    pg_with_mongo["connectionProfiles"]["blog-pg"]["mongoAuthSource"] = json!("admin");
    assert!(read_raw(&f, &pg_with_mongo.to_string()).is_err());

    let mut newline_name = valid_profile_fixture();
    newline_name["connectionProfiles"]["blog-pg"]["name"] = json!("bad\nname");
    assert!(read_raw(&f, &newline_name.to_string()).is_err());

    let mut mongo = valid_profile_fixture();
    mongo["instances"]["blog"]["kind"] = json!("mongodb");
    mongo["connectionProfiles"] = json!({
        "blog-pg": {"id": "blog-pg", "instanceId": "blog", "name": "Blog", "database": "admin", "username": "devmanager"}
    });
    assert!(read_raw(&f, &mongo.to_string()).is_err());
    mongo["connectionProfiles"]["blog-pg"]["mongoAuthSource"] = json!("admin");
    assert!(read_raw(&f, &mongo.to_string()).is_ok());

    let mut redis = valid_profile_fixture();
    redis["instances"]["blog"]["kind"] = json!("redis");
    redis["connectionProfiles"] = json!({
        "blog-pg": {"id": "blog-pg", "instanceId": "blog", "name": "Blog", "username": "default", "redisDb": 15}
    });
    assert!(read_raw(&f, &redis.to_string()).is_ok());
    redis["connectionProfiles"]["blog-pg"]["redisDb"] = json!(16);
    assert!(read_raw(&f, &redis.to_string()).is_err());
    redis["connectionProfiles"]["blog-pg"]["redisDb"] = json!(1);
    redis["connectionProfiles"]["blog-pg"]["database"] = json!("other");
    assert!(read_raw(&f, &redis.to_string()).is_err());

    assert!(validate_connection_profile(&ConnectionProfile::default(), "redis").is_err());
    assert!(validate_connection_profile(&ConnectionProfile::default(), "sqlite").is_err());
}

#[test]
fn tunnel_state_is_validated() {
    let f = fixture();
    let mut value = base_state_json();
    value["tunnelClientId"] = json!("client-01");
    value["cloudflareAccountId"] = json!("0123456789abcdef");
    value["tunnelWorkers"] = json!({
        "devbase-app": {
            "scriptName": "devbase-app",
            "publicUrl": "https://app.example.com",
            "localDomain": "app.devbase.localhost",
            "localPort": 3000,
            "scriptDigest": "0123456789abcdef",
            "authDigest": "fedcba9876543210"
        }
    });
    assert!(read_raw(&f, &value.to_string()).is_ok());

    for (field, replacement) in [
        ("scriptName", json!("other")),
        ("publicUrl", json!("")),
        ("localPort", json!(0)),
        ("scriptDigest", json!("xyz")),
        ("authDigest", json!("XYZ")),
    ] {
        let mut bad = value.clone();
        bad["tunnelWorkers"]["devbase-app"][field] = replacement;
        assert!(
            read_raw(&f, &bad.to_string()).is_err(),
            "accepted {field} replacement"
        );
    }

    let mut bad_name = value.clone();
    bad_name["tunnelWorkers"] = json!({
        "bad": {"scriptName": "bad", "publicUrl": "https://x.example.com", "localDomain": "a.devbase.localhost", "localPort": 3000}
    });
    assert!(read_raw(&f, &bad_name.to_string()).is_err());

    let mut unnormalized = value.clone();
    unnormalized["tunnelClientId"] = json!("Client-01");
    assert!(read_raw(&f, &unnormalized.to_string()).is_err());

    let mut short_account = value.clone();
    short_account["cloudflareAccountId"] = json!("short");
    assert!(read_raw(&f, &short_account.to_string()).is_err());
}

#[test]
fn package_validation_matches_the_go_rules() {
    assert!(sample_package().validate().is_ok());

    let mut bad_arch = sample_package();
    bad_arch.arch = if cfg!(target_os = "macos") {
        "x86_64".into()
    } else if cfg!(target_arch = "aarch64") {
        "amd64".into()
    } else {
        "arm64".into()
    };
    assert!(bad_arch.validate().is_err());

    let mut php = sample_package();
    php.name = "php".into();
    assert!(php.validate().is_err());

    let mut postgres = sample_package();
    postgres.name = "postgresql".into();
    assert!(postgres.validate().is_err());

    let mut http = sample_package();
    http.url = "http://nodejs.org/node.zip".into();
    assert!(http.validate().is_err());

    let mut userinfo = sample_package();
    userinfo.url = "https://user@nodejs.org/node.zip".into();
    assert!(userinfo.validate().is_err());
    userinfo.url = "https://@nodejs.org/node.zip".into();
    assert!(userinfo.validate().is_err());

    let mut bad_sha = sample_package();
    bad_sha.sha256 = "zz".into();
    assert!(bad_sha.validate().is_err());

    let mut bad_format = sample_package();
    bad_format.format = "rar".into();
    assert!(bad_format.validate().is_err());

    let mut strip = sample_package();
    strip.strip = 9;
    assert!(strip.validate().is_err());

    let mut no_binaries = sample_package();
    no_binaries.binaries.clear();
    assert!(no_binaries.validate().is_err());

    let mut escape = sample_package();
    escape.binaries = BTreeMap::from([("node".into(), "../../bin/node".into())]);
    assert!(escape.validate().is_err());

    let mut formula_without_homebrew = sample_package();
    formula_without_homebrew.formula = "postgresql@17".into();
    assert!(formula_without_homebrew.validate().is_err());

    let mut homebrew = sample_package();
    homebrew.name = "postgresql".into();
    homebrew.version = "17.2".into();
    homebrew.formula = "postgresql@17".into();
    homebrew.format = "homebrew".into();
    homebrew.binaries = homebrew_binaries("postgresql");
    assert!(homebrew.validate().is_ok());
    homebrew.binaries.remove("psql");
    assert!(homebrew.validate().is_err());

    let mut java = sample_package();
    java.name = "java".into();
    java.version = "21.0.13_9".into();
    java.java_install_name = "temurin-21".into();
    java.java_folder = "temurin-21-3".into();
    assert!(java.validate().is_ok());
    java.java_folder = "temurin-other".into();
    assert!(java.validate().is_err());
    java.java_folder = "temurin-21-3".into();
    java.java_install_name = "bad name".into();
    assert!(java.validate().is_err());
}

#[test]
fn proxy_rules_are_normalized_and_validated() {
    use crate::model::{default_tunnel_worker_name, local_domain, normalize_proxy_rules};

    assert_eq!(local_domain(" App "), "app.devbase.localhost");
    assert_eq!(
        local_domain("app.devbase.localhost"),
        "app.devbase.localhost"
    );
    assert_eq!(
        default_tunnel_worker_name("app.devbase.localhost"),
        "devbase-app"
    );

    let rule = ProxyRule {
        domain: " App.Devbase.Localhost ".into(),
        local_port: 3000,
        upstream_host: "localhost".into(),
        tunnel: true,
        tunnel_url: "https://app.example.com".into(),
        ..Default::default()
    };
    let normalized = normalize_proxy_rules(443, &[rule]).unwrap();
    assert_eq!(normalized[0].domain, "app.devbase.localhost");
    assert_eq!(normalized[0].upstream_host, "");
    assert_eq!(normalized[0].resolved_tunnel_worker(), "devbase-app");
    assert_eq!(normalized[0].tunnel_url, "https://app.example.com");

    let not_tunneled = ProxyRule {
        domain: "api.devbase.localhost".into(),
        local_port: 3001,
        tunnel_url: "https://api.example.com".into(),
        ..Default::default()
    };
    let normalized = normalize_proxy_rules(443, &[not_tunneled]).unwrap();
    assert_eq!(normalized[0].tunnel_url, "");

    let duplicate = ProxyRule {
        domain: "app.devbase.localhost".into(),
        local_port: 3000,
        ..Default::default()
    };
    assert!(normalize_proxy_rules(443, &[duplicate.clone(), duplicate]).is_err());

    let listener_collision = ProxyRule {
        domain: "app.devbase.localhost".into(),
        local_port: 443,
        ..Default::default()
    };
    assert!(normalize_proxy_rules(443, &[listener_collision]).is_err());

    let unspecified = ProxyRule {
        domain: "app.devbase.localhost".into(),
        local_port: 3000,
        upstream_host: "0.0.0.0".into(),
        ..Default::default()
    };
    assert!(normalize_proxy_rules(443, &[unspecified]).is_err());

    let bad_worker = ProxyRule {
        domain: "app.devbase.localhost".into(),
        local_port: 3000,
        tunnel: true,
        tunnel_worker: "worker".into(),
        ..Default::default()
    };
    assert!(normalize_proxy_rules(443, &[bad_worker]).is_err());

    assert!(normalize_proxy_rules(0, &[]).is_err());
}

#[test]
fn absolute_root_is_required() {
    let relative = Store::new(Path::new("relative/dev")).with_home(Path::new("/tmp/home"));
    assert!(relative.init().is_err());
    assert!(
        relative.read().is_ok(),
        "a missing relative root is empty state"
    );
    assert!(relative.with_state(|_| Ok(())).is_err());
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("dev")).with_home(dir.path().join("home"));
    assert!(store.package_lease("node@1-arm64", true).is_ok());
}
