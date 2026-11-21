//! The software workspace, headless: the catalog and installed versions, the
//! review before an install, the busy state, the confirmation before
//! uninstalling, the file check and an operation's error. A fake provider
//! stands in for devcore: no test downloads a package, runs Homebrew or
//! touches the development home.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::Duration,
};

use devcore::{
    model::Package,
    software::{
        cache::DownloadCacheInfo, details::PackageDetails, integrity::PackageIntegrityResult,
    },
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

use nsh_app_devbase::testing::{
    SoftwareProvider, SoftwareSnapshot, install_software_provider, use_fake_databases,
    use_fake_development, use_fake_guide, use_fake_menubar, use_fake_providers, use_fake_proxy,
    use_fake_software, use_fake_system, use_fake_updater,
};

/// The software tests share one fake-provider registry, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Normal,
    /// Installing takes this long, so the busy state can be seen.
    SlowInstall,
    /// Every install fails the way a broken download does.
    InstallError,
    /// Checking the files finds a missing executable.
    VerifyRepair,
}

struct FakeSoftware {
    snapshot: Mutex<SoftwareSnapshot>,
    calls: Mutex<Vec<String>>,
    behaviour: Behaviour,
}

impl FakeSoftware {
    fn new(installed: Vec<Package>, node_default: &str, behaviour: Behaviour) -> Arc<Self> {
        Arc::new(FakeSoftware {
            snapshot: Mutex::new(SoftwareSnapshot {
                installed,
                node_default: node_default.to_string(),
                ..SoftwareSnapshot::default()
            }),
            calls: Mutex::new(Vec::new()),
            behaviour,
        })
    }

    fn with_cache(self: Arc<Self>, files: i64, bytes: i64) -> Arc<Self> {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cache = DownloadCacheInfo {
            directory: PathBuf::from("/tmp/devbase-tests/downloads"),
            files,
            bytes,
        };
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
}

impl SoftwareProvider for FakeSoftware {
    fn snapshot(&self) -> Result<SoftwareSnapshot, String> {
        Ok(self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn preview(&self, name: &str, version: &str) -> Result<PackageDetails, String> {
        self.record(format!("preview {name}@{version}"));
        let snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let manifest = snapshot
            .installed
            .iter()
            .find(|package| package.name == name && package.version == version)
            .cloned()
            .unwrap_or_else(|| {
                if name == "postgresql" {
                    // Homebrew resolves the major version to its exact stable
                    // keg, as `brew info` does in production.
                    let mut package = package("postgresql", "17.11");
                    package.format = "homebrew".to_string();
                    package.formula = "postgresql@17".to_string();
                    package
                } else {
                    package(name, version)
                }
            });
        let installed = snapshot
            .installed
            .iter()
            .any(|package| package.id() == manifest.id());
        Ok(PackageDetails {
            package: manifest.clone(),
            archive_bytes: 12_345_678,
            cached: false,
            installed,
            compatible: true,
            compatibility: "Compatible architecture".to_string(),
            location: PathBuf::from("/tmp/devbase-tests/packages").join(manifest.id()),
            references: Vec::new(),
            integrity: if installed {
                "Not recorded".to_string()
            } else {
                String::new()
            },
            integrity_note: String::new(),
        })
    }

    fn install(&self, package: &Package) -> Result<(), String> {
        self.record(format!("install {}", package.id()));
        if self.behaviour == Behaviour::SlowInstall {
            thread::sleep(Duration::from_millis(600));
        }
        if self.behaviour == Behaviour::InstallError {
            return Err("download failed: connection reset".to_string());
        }
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !snapshot
            .installed
            .iter()
            .any(|held| held.id() == package.id())
        {
            snapshot.installed.push(package.clone());
            snapshot.installed.sort_by_key(Package::id);
        }
        Ok(())
    }

    fn uninstall(&self, id: &str) -> Result<(), String> {
        self.record(format!("uninstall {id}"));
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        snapshot.installed.retain(|package| package.id() != id);
        Ok(())
    }

    fn verify(&self, id: &str) -> Result<PackageIntegrityResult, String> {
        self.record(format!("verify {id}"));
        if self.behaviour == Behaviour::VerifyRepair {
            return Ok(PackageIntegrityResult {
                status: "Needs repair".to_string(),
                checked_at: "2026-11-20T12:00:00Z".to_string(),
                files: 3,
                issues: vec!["Required executable is missing or invalid: node".to_string()],
                baseline: String::new(),
            });
        }
        Ok(PackageIntegrityResult {
            status: "Verified".to_string(),
            checked_at: "2026-11-20T12:00:00Z".to_string(),
            files: 42,
            issues: Vec::new(),
            baseline: String::new(),
        })
    }

    fn repair(&self, id: &str) -> Result<(), String> {
        self.record(format!("repair {id}"));
        Ok(())
    }

    fn clear_cache(&self) -> Result<DownloadCacheInfo, String> {
        self.record("clear cache".to_string());
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = DownloadCacheInfo {
            directory: snapshot.cache.directory.clone(),
            files: snapshot.cache.files,
            bytes: snapshot.cache.bytes,
        };
        snapshot.cache.files = 0;
        snapshot.cache.bytes = 0;
        Ok(removed)
    }
}

fn package(name: &str, version: &str) -> Package {
    Package {
        name: name.to_string(),
        version: version.to_string(),
        arch: "arm64".to_string(),
        url: format!("https://example.test/{name}/{version}.tar.gz"),
        sha256: "a".repeat(64),
        format: "tar.gz".to_string(),
        license: "MIT".to_string(),
        min_macos: "26.0".to_string(),
        binaries: BTreeMap::new(),
        ..Package::default()
    }
}

fn node(version: &str) -> Package {
    package("node", version)
}

fn mysql(version: &str) -> Package {
    package("mysql", version)
}

/// Installs the fake provider and starts the surface on the Software page,
/// holding the tests' turn for as long as the returned guard lives.
fn start(fake: Arc<FakeSoftware>) -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    use_fake_menubar();
    use_fake_providers();
    use_fake_development();
    use_fake_databases();
    use_fake_proxy();
    use_fake_system();
    use_fake_guide();
    use_fake_updater();
    use_fake_software();
    install_software_provider(fake as Arc<dyn SoftwareProvider>);
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    app.press("role:button[Development]").unwrap();
    app.wait_for("id:software-detail").unwrap();
    (turn, app)
}

#[test]
fn the_catalog_and_installed_versions_render() {
    let fake = FakeSoftware::new(
        vec![node("22.14.0"), mysql("8.4.13")],
        "node@22.14.0-arm64",
        Behaviour::Normal,
    )
    .with_cache(2, 2048);
    let (_turn, mut app) = start(fake.clone());

    app.wait_for("id:installed-node-22.14.0").unwrap();
    app.wait_for("Default").unwrap();
    // The catalog's Node versions are offered without a network read.
    assert!(app.screen().shows("20.19.0"));
    assert!(app.screen().matches("id:version-node-22.14.0").unwrap());
    assert!(app.screen().shows("2 archives · 2.0 KiB"));
    assert!(app.screen().matches("id:clear-cache").unwrap());

    // Another engine has its own installed version and catalog.
    app.press("id:software-mysql").unwrap();
    app.wait_for("id:installed-mysql-8.4.13").unwrap();
    assert!(app.screen().shows("8.4.4"));
    assert!(!app.screen().shows("20.19.0"));
}

#[test]
fn reviewing_then_installing_shows_the_busy_state() {
    let fake = FakeSoftware::new(Vec::new(), "", Behaviour::SlowInstall);
    let (_turn, mut app) = start(fake.clone());

    app.press("id:software-review").unwrap();
    app.wait_for("id:software-preview").unwrap();
    app.wait_for("Copy Source").unwrap();
    assert!(app.screen().shows("Download and install"));

    app.press("id:software-install").unwrap();
    app.wait_for("Installing node@22.14.0-arm64…").unwrap();
    app.wait_for_within("id:uninstall-node-22.14.0", Duration::from_secs(5))
        .unwrap();

    assert!(app.screen().shows("Files not recorded"));
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "install node@22.14.0-arm64")
    );
}

#[test]
fn uninstalling_asks_first_and_cancelling_changes_nothing() {
    let fake = FakeSoftware::new(
        vec![node("22.14.0")],
        "node@22.14.0-arm64",
        Behaviour::Normal,
    );
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:uninstall-node-22.14.0").unwrap();

    // The default answer is Cancel.
    app.press("id:uninstall-node-22.14.0").unwrap();
    app.wait(Duration::from_millis(800)).unwrap();
    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| call.starts_with("uninstall")),
        "a cancelled confirmation must not uninstall"
    );
    assert!(app.screen().matches("id:uninstall-node-22.14.0").unwrap());

    app.on_dialog(|_| DialogAnswer::Press("Uninstall Version".to_string()));
    app.press("id:uninstall-node-22.14.0").unwrap();
    app.wait_for_gone("id:uninstall-node-22.14.0", Duration::from_secs(3))
        .unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "uninstall node@22.14.0-arm64")
    );
}

#[test]
fn a_homebrew_version_reviews_with_its_resolved_keg() {
    let fake = FakeSoftware::new(Vec::new(), "", Behaviour::Normal);
    let (_turn, mut app) = start(fake);

    app.press("id:software-postgresql").unwrap();
    app.wait_for("id:software-detail").unwrap();
    app.press("id:software-review").unwrap();
    app.wait_for("Install with Homebrew").unwrap();
    app.wait_for("text:/Homebrew formula: postgresql@17/")
        .unwrap();
    assert!(app.screen().matches("id:software-preview").unwrap());
}

#[test]
fn an_install_error_is_surfaced_verbatim() {
    let fake = FakeSoftware::new(Vec::new(), "", Behaviour::InstallError);
    let (_turn, mut app) = start(fake);

    app.press("id:software-review").unwrap();
    app.wait_for("id:software-preview").unwrap();
    app.press("id:software-install").unwrap();
    app.wait_for("download failed: connection reset").unwrap();
    // The review stays so the same version can be tried again.
    assert!(app.screen().matches("id:software-preview").unwrap());
}

#[test]
fn clearing_the_cache_asks_first() {
    let fake = FakeSoftware::new(
        vec![node("22.14.0")],
        "node@22.14.0-arm64",
        Behaviour::Normal,
    )
    .with_cache(2, 2048);
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:clear-cache").unwrap();

    // The default answer is Cancel.
    app.press("id:clear-cache").unwrap();
    app.wait(Duration::from_millis(800)).unwrap();
    assert!(
        !fake.calls().iter().any(|call| call == "clear cache"),
        "a cancelled confirmation must not clear the cache"
    );
    assert!(app.screen().shows("2 archives · 2.0 KiB"));

    app.on_dialog(|_| DialogAnswer::Press("Clear Cache".to_string()));
    app.press("id:clear-cache").unwrap();
    app.wait_for("Cleared 2 archives (2.0 KiB) from the download cache.")
        .unwrap();
    app.wait_for("Empty").unwrap();
    assert!(fake.calls().iter().any(|call| call == "clear cache"));
}

#[test]
fn checking_files_shows_the_result_and_its_issues() {
    let fake = FakeSoftware::new(
        vec![node("22.14.0")],
        "node@22.14.0-arm64",
        Behaviour::VerifyRepair,
    );
    let (_turn, mut app) = start(fake.clone());
    app.wait_for("id:check-node-22.14.0").unwrap();

    app.press("id:check-node-22.14.0").unwrap();
    app.wait_for("Files needs repair").unwrap();
    app.wait_for("Required executable is missing or invalid: node")
        .unwrap();
    assert!(app.screen().shows("3 files checked"));
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "verify node@22.14.0-arm64")
    );
}

fn node_id(app: &Headless, test_id: &str) -> nsh_core::NodeId {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(test_id))
        .unwrap_or_else(|| panic!("{test_id} should be mounted"))
        .id
}

#[test]
fn homebrew_reinstall_copy_button_copies_command_and_shows_notice() {
    let mut pg = package("postgresql", "17.11");
    pg.format = "homebrew".to_string();
    pg.formula = "postgresql@17".to_string();
    let fake = FakeSoftware::new(vec![pg], "", Behaviour::Normal);
    let (_turn, mut app) = start(fake);

    app.press("id:software-postgresql").unwrap();
    app.wait_for("id:copy-brew-postgresql-17.11").unwrap();
    assert!(app.screen().shows("brew reinstall postgresql@17"));

    app.press("id:copy-brew-postgresql-17.11").unwrap();
    app.wait_for("Copied").unwrap();
    assert_eq!(
        app.screen().clipboard(),
        Some("brew reinstall postgresql@17")
    );
}

#[test]
fn typing_version_draft_keeps_cards_and_preview_mounted() {
    let fake = FakeSoftware::new(
        vec![node("22.14.0")],
        "node@22.14.0-arm64",
        Behaviour::Normal,
    )
    .with_cache(2, 2048);
    let (_turn, mut app) = start(fake);

    app.wait_for("id:installed-node-22.14.0").unwrap();
    app.press("id:software-review").unwrap();
    app.wait_for("id:software-preview").unwrap();

    let installed_id = node_id(&app, "installed-node-22.14.0");
    let chip_id = node_id(&app, "version-node-22.14.0");
    let input_id = node_id(&app, "software-version-exact");
    let review_id = node_id(&app, "software-review");
    let install_id = node_id(&app, "software-install");
    let preview_id = node_id(&app, "software-preview");
    let cache_id = node_id(&app, "clear-cache");

    app.clear("id:software-version-exact").unwrap();
    app.type_text("id:software-version-exact", "20.19.0")
        .unwrap();

    assert_eq!(
        app.screen()
            .input_text("id:software-version-exact")
            .unwrap(),
        Some("20.19.0")
    );
    assert_eq!(node_id(&app, "installed-node-22.14.0"), installed_id);
    assert_eq!(node_id(&app, "version-node-22.14.0"), chip_id);
    assert_eq!(node_id(&app, "software-version-exact"), input_id);
    assert_eq!(node_id(&app, "software-review"), review_id);
    assert_eq!(node_id(&app, "software-install"), install_id);
    assert_eq!(node_id(&app, "software-preview"), preview_id);
    assert_eq!(node_id(&app, "clear-cache"), cache_id);
}

#[test]
fn installed_database_engines_offer_create_database_instance_cta() {
    let fake = FakeSoftware::new(
        vec![
            node("22.14.0"),
            mysql("8.4.13"),
            package("postgresql", "17.11"),
            package("redis", "7.4.2"),
            package("mongodb", "8.0.4"),
        ],
        "node@22.14.0-arm64",
        Behaviour::Normal,
    );
    let (_turn, mut app) = start(fake);

    // Node.js is a runtime, not a database engine: no database CTA.
    app.wait_for("id:installed-node-22.14.0").unwrap();
    assert!(
        !app.screen().matches("id:software-create-instance").unwrap(),
        "Node.js must not show a Create database instance CTA"
    );

    // Every installed database engine shows the CTA beside its installed row.
    for (engine, installed_id) in [
        ("mysql", "id:installed-mysql-8.4.13"),
        ("postgresql", "id:installed-postgresql-17.11"),
        ("redis", "id:installed-redis-7.4.2"),
        ("mongodb", "id:installed-mongodb-8.0.4"),
    ] {
        app.press(&format!("id:software-{engine}")).unwrap();
        app.wait_for(installed_id).unwrap();
        app.wait_for("id:software-create-instance").unwrap();
        assert!(app.screen().shows("Create database instance"));
    }

    // Pressing the CTA navigates to Databases and pre-selects the installed engine.
    app.press("id:software-create-instance").unwrap();
    app.wait_for("id:db-create-form").unwrap();
    app.wait_for("New instance").unwrap();
    assert!(app.screen().selected("id:db-engine-mongodb").unwrap());
}
