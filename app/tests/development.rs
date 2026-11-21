//! The development workspace, headless: the four tabs, the per-runtime
//! defaults and version pickers, the shell startup integration and its
//! confirmations, and the runtime terminal's selection and launch. A fake
//! development provider stands in for devcore's shell facade and a fake
//! software provider keeps the embedded Software tab off the real home: no
//! test edits a startup file, runs a shell or opens Terminal.

use std::{
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::Duration,
};

use devcore::{
    model::Package,
    shell::NodeRelease,
    software::{
        cache::DownloadCacheInfo, details::PackageDetails, integrity::PackageIntegrityResult,
    },
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

use nsh_app_devbase::testing::{
    DevelopmentJava, DevelopmentNode, DevelopmentProvider, DevelopmentReleases,
    DevelopmentSelection, DevelopmentShell, DevelopmentSnapshot, SoftwareProvider,
    SoftwareSnapshot, install_development_provider, install_software_provider,
    use_fake_development, use_fake_software,
};

/// The development tests share one fake-provider registry, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Normal,
    /// Actions take this long, so the busy state can be seen.
    Slow,
    /// Enabling the Node.js shell block fails the way a damaged file does.
    ShellError,
    /// Opening Terminal fails.
    LaunchError,
}

#[derive(Clone)]
struct Fixture {
    snapshot: DevelopmentSnapshot,
    releases: Vec<NodeRelease>,
    java_catalog: Vec<Package>,
}

struct FakeDevelopment {
    fixture: Mutex<Fixture>,
    calls: Mutex<Vec<String>>,
    behaviour: Behaviour,
}

impl FakeDevelopment {
    fn new(fixture: Fixture, behaviour: Behaviour) -> Arc<Self> {
        Arc::new(FakeDevelopment {
            fixture: Mutex::new(fixture),
            calls: Mutex::new(Vec::new()),
            behaviour,
        })
    }

    fn record(&self, call: String) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(call);
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn slow(&self) {
        if self.behaviour == Behaviour::Slow {
            thread::sleep(Duration::from_millis(600));
        }
    }

    fn snapshot(&self) -> DevelopmentSnapshot {
        self.fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot
            .clone()
    }

    fn with<R>(&self, f: impl FnOnce(&mut DevelopmentSnapshot) -> R) -> R {
        let mut fixture = self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut fixture.snapshot)
    }
}

impl DevelopmentProvider for FakeDevelopment {
    fn snapshot(&self) -> Result<DevelopmentSnapshot, String> {
        Ok(self.snapshot())
    }

    fn node_releases(&self, refresh: bool) -> Result<DevelopmentReleases, String> {
        self.record(format!("releases {refresh}"));
        let releases = self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .releases
            .clone();
        Ok(DevelopmentReleases {
            releases,
            stale: false,
        })
    }

    fn set_node_default(&self, id: &str) -> Result<(), String> {
        self.record(format!("set node default {id}"));
        self.slow();
        self.with(|snapshot| {
            for node in &mut snapshot.node {
                node.default = node.id == id;
            }
            snapshot.node_default = id.to_string();
        });
        Ok(())
    }

    fn set_java_default(&self, id: &str) -> Result<(), String> {
        self.record(format!("set java default {id}"));
        self.slow();
        self.with(|snapshot| {
            for jdk in &mut snapshot.java {
                jdk.default = jdk.id == id;
            }
            snapshot.java_default = id.to_string();
        });
        Ok(())
    }

    fn node_binary(&self, id: &str) -> Result<String, String> {
        self.record(format!("node binary {id}"));
        Ok(format!("/tmp/devbase-tests/packages/{id}/bin/node"))
    }

    fn install_python(&self, key: &str) -> Result<(), String> {
        self.record(format!("install python {key}"));
        self.slow();
        self.with(|snapshot| {
            if let Some(row) = snapshot
                .python_available
                .iter()
                .find(|row| row.key == key)
                .cloned()
                && !snapshot.python.iter().any(|held| held.key == key)
            {
                snapshot.python.push(row);
            }
        });
        Ok(())
    }

    fn python_action(&self, key: &str, action: &str) -> Result<(), String> {
        self.record(format!("python {action} {key}"));
        self.slow();
        self.with(|snapshot| match action {
            "default" => {
                snapshot.python_pin = key.to_string();
                for row in &mut snapshot.python {
                    row.pin = row.key == key;
                }
            }
            "uninstall" => {
                snapshot.python.retain(|row| row.key != key);
                if snapshot.python_pin == key {
                    snapshot.python_pin.clear();
                }
            }
            _ => {}
        });
        Ok(())
    }

    fn set_uv_path(&self, path: &str) -> Result<(), String> {
        self.record(format!("set uv path {path}"));
        self.with(|snapshot| snapshot.uv_path = path.to_string());
        Ok(())
    }

    fn java_catalog(&self) -> Result<Vec<Package>, String> {
        self.record("java catalog".to_string());
        Ok(self
            .fixture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .java_catalog
            .clone())
    }

    fn install_java(&self, package: Package) -> Result<(), String> {
        self.record(format!("install java {}", package.id()));
        self.slow();
        self.with(|snapshot| {
            if !snapshot.java.iter().any(|held| held.id == package.id()) {
                snapshot.java.push(DevelopmentJava {
                    id: package.id(),
                    version: package.version.clone(),
                    product: package.java_product.clone(),
                    default: false,
                });
            }
        });
        Ok(())
    }

    fn node_shell(&self, enable: bool) -> Result<(), String> {
        self.record(format!("node shell {enable}"));
        self.slow();
        if self.behaviour == Behaviour::ShellError {
            return Err("duplicate runtime shell blocks; review this file".to_string());
        }
        self.with(|snapshot| {
            snapshot.node_shell_enabled = enable;
            for shell in &mut snapshot.shells {
                shell.node_applied = enable && shell.available;
            }
        });
        Ok(())
    }

    fn java_shell(&self, enable: bool) -> Result<(), String> {
        self.record(format!("java shell {enable}"));
        self.slow();
        self.with(|snapshot| {
            snapshot.java_shell_enabled = enable;
            for shell in &mut snapshot.shells {
                shell.java_applied = enable && shell.available;
            }
        });
        Ok(())
    }

    fn select_node(
        &self,
        dir: &str,
        override_spec: Option<&str>,
    ) -> Result<DevelopmentSelection, String> {
        self.record(format!(
            "select {dir} {}",
            override_spec.unwrap_or("default")
        ));
        let snapshot = self.snapshot();
        let wanted = match override_spec {
            Some(spec) => snapshot.node.iter().find(|node| node.version == spec),
            None => snapshot
                .node
                .iter()
                .find(|node| node.id == snapshot.node_default),
        };
        let Some(node) = wanted else {
            return Err(
                "no default Node.js version. Install one in Devbase: Software, Node.js".to_string(),
            );
        };
        let source = match override_spec {
            Some(_) if dir.contains(".nvmrc") => dir.to_string(),
            Some(_) => "the requested version".to_string(),
            None => "default".to_string(),
        };
        Ok(DevelopmentSelection {
            id: node.id.clone(),
            version: node.version.clone(),
            dir: dir.to_string(),
            prefix: format!("/tmp/devbase-tests/node/global/{}", node.version),
            path: format!(
                "/tmp/devbase-tests/node/global/{0}/bin:/tmp/devbase-tests/node/{0}/bin",
                node.version
            ),
            spec: override_spec.unwrap_or_default().to_string(),
            source: source.clone(),
            system: dir.contains("system"),
        })
    }

    fn launch_terminal(&self, selection: &DevelopmentSelection) -> Result<(), String> {
        self.record(format!(
            "launch {} {} {}",
            selection.version, selection.dir, selection.path
        ));
        self.slow();
        if self.behaviour == Behaviour::LaunchError {
            return Err("Terminal could not be opened".to_string());
        }
        Ok(())
    }
}

/// The embedded Software tab asks for a snapshot; nothing else is exercised.
struct EmptySoftware;

impl SoftwareProvider for EmptySoftware {
    fn snapshot(&self) -> Result<SoftwareSnapshot, String> {
        Ok(SoftwareSnapshot::default())
    }

    fn preview(&self, _name: &str, _version: &str) -> Result<PackageDetails, String> {
        Err("unused".to_string())
    }

    fn install(&self, _package: &Package) -> Result<(), String> {
        Err("unused".to_string())
    }

    fn uninstall(&self, _id: &str) -> Result<(), String> {
        Err("unused".to_string())
    }

    fn verify(&self, _id: &str) -> Result<PackageIntegrityResult, String> {
        Err("unused".to_string())
    }

    fn repair(&self, _id: &str) -> Result<(), String> {
        Err("unused".to_string())
    }

    fn clear_cache(&self) -> Result<DownloadCacheInfo, String> {
        Err("unused".to_string())
    }
}

fn node(version: &str, default: bool) -> DevelopmentNode {
    DevelopmentNode {
        id: format!("node@{version}-arm64"),
        version: version.to_string(),
        lts: if version.starts_with("22") {
            "Jod".to_string()
        } else {
            String::new()
        },
        default,
    }
}

fn java(version: &str, default: bool) -> DevelopmentJava {
    DevelopmentJava {
        id: format!("java@{version}-arm64"),
        version: version.to_string(),
        product: "Eclipse Temurin".to_string(),
        default,
    }
}

fn python(key: &str, version: &str, pin: bool) -> nsh_app_devbase::testing::DevelopmentPython {
    nsh_app_devbase::testing::DevelopmentPython {
        key: key.to_string(),
        version: version.to_string(),
        path: format!("/Users/test/.local/share/uv/python/{key}/bin/python3"),
        pin,
    }
}

fn shell(name: &str, available: bool, node: bool, java: bool) -> DevelopmentShell {
    DevelopmentShell {
        shell: name.to_string(),
        label: match name {
            "bash" => "Bash".to_string(),
            other => other.to_string(),
        },
        available,
        files: vec![format!("/Users/test/.{name}rc")],
        node_applied: node,
        java_applied: java,
    }
}

fn release(version: &str, lts: &str) -> NodeRelease {
    NodeRelease {
        version: version.to_string(),
        lts: lts.to_string(),
        ..NodeRelease::default()
    }
}

fn fixture() -> Fixture {
    Fixture {
        snapshot: DevelopmentSnapshot {
            node: vec![node("22.14.0", true), node("20.19.0", false)],
            node_default: "node@22.14.0-arm64".to_string(),
            java: vec![java("21.0.5", true), java("17.0.12", false)],
            java_default: "java@21.0.5-arm64".to_string(),
            python: vec![python("cpython-3.12.7", "3.12.7", true)],
            python_available: vec![
                python("cpython-3.13.0", "3.13.0", false),
                python("cpython-3.12.7", "3.12.7", false),
            ],
            python_pin: "cpython-3.12.7".to_string(),
            uv_description: "uv 0.5.0 · /usr/local/bin/uv".to_string(),
            uv_path: "/usr/local/bin/uv".to_string(),
            uv_error: String::new(),
            shells: vec![
                shell("zsh", true, false, false),
                shell("bash", true, false, false),
                shell("fish", false, false, false),
            ],
            node_shell_enabled: false,
            java_shell_enabled: false,
            bash_command:
                "BASH_ENV='/tmp/devbase-tests/shell/bash-env.sh' /bin/bash -c 'YOUR_COMMAND'"
                    .to_string(),
        },
        releases: vec![
            release("24.2.0", ""),
            release("22.14.0", "Jod"),
            release("20.19.0", ""),
        ],
        java_catalog: vec![Package {
            name: "java".to_string(),
            version: "24.0.1".to_string(),
            arch: "arm64".to_string(),
            java_product: "JetBrains Runtime".to_string(),
            java_install_name: "jbr_jcef-24.0.1".to_string(),
            ..Package::default()
        }],
    }
}

/// Installs both fake providers and starts the surface on the Development
/// page, holding the tests' turn for as long as the returned guard lives.
fn start(fake: Arc<FakeDevelopment>) -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    use_fake_software();
    install_software_provider(Arc::new(EmptySoftware));
    use_fake_development();
    install_development_provider(fake as Arc<dyn DevelopmentProvider>);
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    app.press("role:button[Development]").unwrap();
    app.wait_for("id:dev-tab-runtimes").unwrap();
    (turn, app)
}

fn open_runtimes(app: &mut Headless) {
    app.press("id:dev-tab-runtimes").unwrap();
    app.wait_for("Node.js · 22.14.0").unwrap();
}

#[test]
fn the_four_tabs_render_and_switch() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Normal);
    let (_turn, mut app) = start(fake);

    // Software is the default tab, so its workspace shows first.
    app.wait_for("id:software-detail").unwrap();
    for tab in ["software", "runtimes", "shell", "overview"] {
        assert!(
            app.screen().matches(&format!("id:dev-tab-{tab}")).unwrap(),
            "the {tab} tab is missing"
        );
    }

    app.press("id:dev-tab-runtimes").unwrap();
    app.wait_for("Node.js · 22.14.0").unwrap();
    app.wait_for("Default Node.js 22.14.0").unwrap();
    app.wait_for("Eclipse Temurin · 21.0.5").unwrap();
    app.wait_for("Python · 3.12.7").unwrap();

    app.press("id:dev-tab-shell").unwrap();
    app.wait_for("id:shell-row-zsh").unwrap();
    app.wait_for("Node.js not applied").unwrap();
    app.wait_for("Java not applied").unwrap();
    app.wait_for("Not installed").unwrap();

    app.press("id:dev-tab-overview").unwrap();
    app.wait_for("Node.js default").unwrap();
    app.wait_for("Java default").unwrap();
    app.wait_for("Python default (uv)").unwrap();

    app.press("id:dev-tab-software").unwrap();
    app.wait_for("id:software-detail").unwrap();
}

#[test]
fn runtimes_show_each_default_and_a_refreshed_release_list() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());
    open_runtimes(&mut app);

    app.wait_for("Default Node.js 22.14.0").unwrap();
    app.wait_for("Default JDK 21.0.5").unwrap();
    app.wait_for("uv global pin: cpython-3.12.7").unwrap();
    // The release index is read once when the tab first shows.
    app.wait_for("3 tracked · install a version in Software")
        .unwrap();
    app.wait_for("24.2.0").unwrap();
    assert!(fake.calls().iter().any(|call| call == "releases false"));

    app.press("id:dev-refresh-releases").unwrap();
    app.wait_for("24.2.0").unwrap();
    assert!(
        fake.calls().iter().any(|call| call == "releases true"),
        "Refresh available versions must force a read"
    );
}

#[test]
fn setting_a_node_default_from_the_overview_picker_is_a_task() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Slow);
    let (_turn, mut app) = start(fake.clone());

    app.press("id:dev-tab-overview").unwrap();
    app.wait_for("id:defaults-node-node@20.19.0-arm64").unwrap();
    app.press("id:defaults-node-node@20.19.0-arm64").unwrap();
    app.wait_for("id:dev-busy").unwrap();
    app.wait_for_within("Default is Node.js 20.19.0", Duration::from_secs(5))
        .unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "set node default node@20.19.0-arm64")
    );
    app.press("id:dev-tab-runtimes").unwrap();
    app.wait_for("Default Node.js 20.19.0").unwrap();
}

#[test]
fn setting_the_java_default_reaches_the_shell_facade() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());
    open_runtimes(&mut app);

    app.press("id:java-default-java@17.0.12-arm64").unwrap();
    app.wait_for("Java default is 17.0.12; enable terminal integration to use it")
        .unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "set java default java@17.0.12-arm64")
    );
    app.wait_for("Default JDK 17.0.12").unwrap();
}

#[test]
fn installing_a_jetbrains_jdk_adds_it_to_the_installed_list() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());
    open_runtimes(&mut app);

    let id = "java@24.0.1-arm64";
    app.wait_for(&format!("id:java-catalog-{id}")).unwrap();
    app.press(&format!("id:java-catalog-{id}")).unwrap();
    app.press("id:java-install").unwrap();
    app.wait_for("Installed Java 24.0.1").unwrap();
    app.wait_for(&format!("id:java-{id}")).unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == &format!("install java {id}"))
    );
}

#[test]
fn installing_a_uv_python_updates_the_installed_list() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());
    open_runtimes(&mut app);

    app.press("id:python-available-cpython-3.13.0").unwrap();
    app.press("id:python-install").unwrap();
    app.wait_for("Installed Python cpython-3.13.0 with uv")
        .unwrap();
    app.wait_for("id:python-cpython-3.13.0").unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "install python cpython-3.13.0")
    );
}

#[test]
fn shell_integration_shows_each_shell_and_asks_before_enabling() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Slow);
    let (_turn, mut app) = start(fake.clone());

    app.press("id:dev-tab-shell").unwrap();
    app.wait_for("id:shell-row-zsh").unwrap();
    app.wait_for("id:shell-row-bash").unwrap();
    app.wait_for("id:shell-row-fish").unwrap();
    app.wait_for("Node.js not applied").unwrap();
    app.wait_for("Not installed").unwrap();

    // The default answer is Cancel.
    app.press("id:shell-enable-node").unwrap();
    app.wait(Duration::from_millis(800)).unwrap();
    assert!(
        !fake.calls().iter().any(|call| call == "node shell true"),
        "a cancelled confirmation must not edit a startup file"
    );

    app.on_dialog(|_| DialogAnswer::Press("Enable Node.js".to_string()));
    app.press("id:shell-enable-node").unwrap();
    app.wait_for("id:dev-busy").unwrap();
    app.wait_for_within(
        "Enabled for installed shells. Start a new terminal or AI session.",
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(fake.calls().iter().any(|call| call == "node shell true"));
    app.wait_for("Enabled for detected shells").unwrap();
    app.wait_for("Node.js applied").unwrap();
}

#[test]
fn removing_shell_integration_asks_first_and_removes_only_the_block() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());

    app.press("id:dev-tab-shell").unwrap();
    app.wait_for("id:shell-remove-node").unwrap();
    app.on_dialog(|_| DialogAnswer::Press("Remove Node.js".to_string()));
    app.press("id:shell-remove-node").unwrap();
    app.wait_for("Removed the Devbase Node.js blocks from all supported shells.")
        .unwrap();
    assert!(fake.calls().iter().any(|call| call == "node shell false"));
}

#[test]
fn a_shell_error_is_shown_verbatim() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::ShellError);
    let (_turn, mut app) = start(fake.clone());

    app.press("id:dev-tab-shell").unwrap();
    app.wait_for("id:shell-enable-node").unwrap();
    app.on_dialog(|_| DialogAnswer::Press("Enable Node.js".to_string()));
    app.press("id:shell-enable-node").unwrap();
    app.wait_for("duplicate runtime shell blocks; review this file")
        .unwrap();
}

#[test]
fn the_terminal_resolves_the_folder_and_launches_the_selection() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Normal);
    let (_turn, mut app) = start(fake.clone());
    open_runtimes(&mut app);

    app.press("id:dev-open-terminal").unwrap();
    app.wait_for("id:dev-terminal").unwrap();
    app.clear("id:terminal-dir").unwrap();
    app.type_text("id:terminal-dir", "/tmp/devbase-tests/app/sub")
        .unwrap();
    app.press("id:terminal-resolve").unwrap();
    app.wait_for("Node.js 22.14.0").unwrap();
    app.wait_for("default").unwrap();
    assert!(
        fake.calls()
            .iter()
            .any(|call| call == "select /tmp/devbase-tests/app/sub default")
    );

    app.press("id:terminal-override-20.19.0").unwrap();
    app.press("id:terminal-resolve").unwrap();
    app.wait_for("Node.js 20.19.0").unwrap();
    app.wait_for("the requested version").unwrap();

    app.press("id:terminal-launch").unwrap();
    app.wait_for("Opened Terminal with Node.js 20.19.0.")
        .unwrap();
    let launch = fake
        .calls()
        .into_iter()
        .find(|call| call.starts_with("launch "))
        .expect("the launch must reach the provider");
    assert_eq!(
        launch,
        "launch 20.19.0 /tmp/devbase-tests/app/sub /tmp/devbase-tests/node/global/20.19.0/bin:/tmp/devbase-tests/node/20.19.0/bin"
    );
}

#[test]
fn a_terminal_launch_error_is_shown_verbatim() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::LaunchError);
    let (_turn, mut app) = start(fake.clone());
    open_runtimes(&mut app);

    app.press("id:dev-open-terminal").unwrap();
    app.wait_for("id:terminal-resolve").unwrap();
    app.press("id:terminal-resolve").unwrap();
    app.wait_for("Node.js 22.14.0").unwrap();
    app.press("id:terminal-launch").unwrap();
    app.wait_for("Terminal could not be opened").unwrap();
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
fn typing_uv_path_and_terminal_dir_keeps_sibling_cards_mounted() {
    let fake = FakeDevelopment::new(fixture(), Behaviour::Normal);
    let (_turn, mut app) = start(fake);
    open_runtimes(&mut app);

    app.wait_for("id:uv-path-input").unwrap();
    app.wait_for("id:python-cpython-3.12.7").unwrap();

    let uv_input_id = node_id(&app, "uv-path-input");
    let node_card_id = node_id(&app, "node-installed");
    let java_row_id = node_id(&app, "java-java@21.0.5-arm64");
    let python_row_id = node_id(&app, "python-cpython-3.12.7");

    app.clear("id:uv-path-input").unwrap();
    app.type_text("id:uv-path-input", "/opt/homebrew/bin/uv")
        .unwrap();

    assert_eq!(
        app.screen().input_text("id:uv-path-input").unwrap(),
        Some("/opt/homebrew/bin/uv")
    );
    assert_eq!(node_id(&app, "uv-path-input"), uv_input_id);
    assert_eq!(node_id(&app, "node-installed"), node_card_id);
    assert_eq!(node_id(&app, "java-java@21.0.5-arm64"), java_row_id);
    assert_eq!(node_id(&app, "python-cpython-3.12.7"), python_row_id);

    // Copying the Python executable path populates the clipboard without tearing down sibling cards.
    app.press("id:python-copy-cpython-3.12.7").unwrap();
    app.wait_for("Copied Python executable path.").unwrap();
    assert_eq!(
        app.screen().clipboard(),
        Some("/Users/test/.local/share/uv/python/cpython-3.12.7/bin/python3")
    );
    assert_eq!(node_id(&app, "node-installed"), node_card_id);
    assert_eq!(node_id(&app, "java-java@21.0.5-arm64"), java_row_id);
    assert_eq!(node_id(&app, "python-cpython-3.12.7"), python_row_id);

    // Typing into terminal-dir does not rebuild selection_card or drop input focus.
    app.press("id:dev-open-terminal").unwrap();
    app.wait_for("id:terminal-dir").unwrap();
    app.wait_for("id:terminal-selection").unwrap();

    let dir_input_id = node_id(&app, "terminal-dir");
    let selection_id = node_id(&app, "terminal-selection");

    app.clear("id:terminal-dir").unwrap();
    app.type_text("id:terminal-dir", "/tmp/devbase-tests/project")
        .unwrap();

    assert_eq!(
        app.screen().input_text("id:terminal-dir").unwrap(),
        Some("/tmp/devbase-tests/project")
    );
    assert_eq!(node_id(&app, "terminal-dir"), dir_input_id);
    assert_eq!(node_id(&app, "terminal-selection"), selection_id);
}
