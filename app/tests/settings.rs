//! Settings, headless: the helper's passive status and message, its install
//! busy line and verbatim failure, the CLI wrapper's status, and the
//! same-binary dispatch. The Settings provider is a fake: no test prompts for
//! an administrator password, runs `osascript`, installs a launchd job or
//! writes a wrapper script.

use std::{
    process::ExitCode,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use nsh_app_devbase::{
    launch::{self, Seams},
    testing::{
        AvailableUpdate, SystemCacheClear, SystemCacheInfo, SystemCliChange, SystemCliStatus,
        SystemHelperChange, SystemHelperStatus, SystemLoginChange, SystemSettingsProvider,
        SystemSettingsSnapshot, UpdateFeedInfo, UpdateProvider, UpdateState,
        clear_fake_updater_provider, install_system_settings_provider, install_updater_provider,
        system_request_quit, use_fake_databases, use_fake_development, use_fake_guide,
        use_fake_providers, use_fake_proxy, use_fake_software, use_fake_system, use_fake_updater,
    },
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

/// The settings tests share the fake-provider registry, so they take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperBehaviour {
    Normal,
    Slow,
    Error,
}

struct FakeSettings {
    snapshot: Mutex<SystemSettingsSnapshot>,
    calls: Mutex<Vec<String>>,
    helper: HelperBehaviour,
    cli_install_ok: AtomicBool,
    shutdown_plan: Mutex<devcore::shutdown::ShutdownPlan>,
    shutdown_delay: Mutex<Duration>,
}

impl FakeSettings {
    fn new(helper: HelperBehaviour) -> Arc<Self> {
        Arc::new(FakeSettings {
            snapshot: Mutex::new(SystemSettingsSnapshot {
                helper: SystemHelperStatus {
                    installed: false,
                    message: "Not installed. Root operations need it; installing asks for your administrator password once."
                        .to_string(),
                },
                cli: SystemCliStatus {
                    installed: false,
                    wrapper_path: "/tmp/devbase-tests/bin/devbase".to_string(),
                    target_executable: "/tmp/devbase-tests/Devbase".to_string(),
                    in_path: false,
                    managed_by_devbase: false,
                },
                cache: SystemCacheInfo {
                    directory: "/tmp/devbase-tests/downloads".to_string(),
                    files: 0,
                    bytes: 0,
                },
                log_rotation_enabled: true,
                login_at_login: false,
                workspace_root: "/tmp/devbase-tests/root".to_string(),
            }),
            calls: Mutex::new(Vec::new()),
            helper,
            cli_install_ok: AtomicBool::new(false),
            shutdown_plan: Mutex::new(devcore::shutdown::ShutdownPlan::default()),
            shutdown_delay: Mutex::new(Duration::ZERO),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn record(&self, call: impl Into<String>) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(call.into());
    }

    fn snapshot(&self) -> SystemSettingsSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl SystemSettingsProvider for FakeSettings {
    fn snapshot(&self) -> Result<SystemSettingsSnapshot, String> {
        Ok(self.snapshot())
    }

    fn install_helper(&self) -> SystemHelperChange {
        self.record("install helper");
        match self.helper {
            HelperBehaviour::Slow => thread::sleep(Duration::from_millis(600)),
            HelperBehaviour::Error => {
                return SystemHelperChange {
                    status: self.snapshot().helper,
                    error: Some("administrator password was rejected".to_string()),
                };
            }
            HelperBehaviour::Normal => {}
        }
        let status = SystemHelperStatus {
            installed: true,
            message: "Installed. Root operations run through it without password prompts."
                .to_string(),
        };
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .helper = status.clone();
        SystemHelperChange {
            status,
            error: None,
        }
    }

    fn uninstall_helper(&self) -> SystemHelperChange {
        self.record("uninstall helper");
        let status = SystemHelperStatus {
            installed: false,
            message: "Not installed. Root operations need it; installing asks for your administrator password once."
                .to_string(),
        };
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .helper = status.clone();
        SystemHelperChange {
            status,
            error: None,
        }
    }

    fn install_cli(&self) -> SystemCliChange {
        self.record("install cli");
        let status = SystemCliStatus {
            installed: true,
            wrapper_path: "/tmp/devbase-tests/bin/devbase".to_string(),
            target_executable: "/tmp/devbase-tests/Devbase".to_string(),
            in_path: true,
            managed_by_devbase: true,
        };
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cli = status.clone();
        self.cli_install_ok.store(true, Ordering::Relaxed);
        SystemCliChange {
            status,
            error: None,
        }
    }

    fn uninstall_cli(&self) -> SystemCliChange {
        self.record("uninstall cli");
        let status = SystemCliStatus {
            wrapper_path: "/tmp/devbase-tests/bin/devbase".to_string(),
            ..SystemCliStatus::default()
        };
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cli = status.clone();
        SystemCliChange {
            status,
            error: None,
        }
    }

    fn clear_cache(&self) -> Result<SystemCacheClear, String> {
        self.record("clear cache");
        Ok(SystemCacheClear {
            removed: SystemCacheInfo {
                directory: "/tmp/devbase-tests/downloads".to_string(),
                files: 2,
                bytes: 2048,
            },
            current: SystemCacheInfo {
                directory: "/tmp/devbase-tests/downloads".to_string(),
                files: 0,
                bytes: 0,
            },
        })
    }

    fn set_log_rotation(&self, enabled: bool) -> Result<(), String> {
        self.record(format!("log rotation {enabled}"));
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .log_rotation_enabled = enabled;
        Ok(())
    }

    fn set_login_item(&self, enabled: bool) -> SystemLoginChange {
        self.record(format!("login item {enabled}"));
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .login_at_login = enabled;
        SystemLoginChange {
            enabled,
            error: None,
        }
    }

    fn open_full_disk_access(&self) -> Result<(), String> {
        self.record("open full disk access");
        Ok(())
    }

    fn open_login_items(&self) -> Result<(), String> {
        self.record("open login items");
        Ok(())
    }

    fn reveal(&self, path: &str) -> Result<(), String> {
        self.record(format!("reveal {path}"));
        Ok(())
    }

    fn preview_shutdown(&self) -> Result<devcore::shutdown::ShutdownPlan, String> {
        self.record("preview shutdown");
        let delay = *self
            .shutdown_delay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        Ok(self
            .shutdown_plan
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn shutdown(&self, _plan: &devcore::shutdown::ShutdownPlan) -> Result<(), String> {
        self.record("shutdown");
        Ok(())
    }
}

fn start_with_updater(
    fake: Arc<FakeSettings>,
    updater: Option<Arc<dyn UpdateProvider>>,
) -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    use_fake_providers();
    use_fake_software();
    use_fake_development();
    use_fake_databases();
    use_fake_proxy();
    use_fake_guide();
    use_fake_system();
    use_fake_updater();
    clear_fake_updater_provider();
    if let Some(provider) = updater {
        install_updater_provider(provider);
    }
    install_system_settings_provider(fake);
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    app.press("role:button[Settings]").unwrap();
    app.wait_for("id:settings-page").unwrap();
    (turn, app)
}

fn start(fake: Arc<FakeSettings>) -> (MutexGuard<'static, ()>, Headless) {
    start_with_updater(fake, None)
}

#[component]
fn QuitDialogHarness() -> impl IntoElement {
    view()
        .size_full()
        .flex_col()
        .child(
            nsh_base::button()
                .test_id("trigger-quit")
                .on_press(system_request_quit)
                .child(text("Trigger Quit")),
        )
        .child(nsh_app_devbase::app())
}

#[test]
fn quit_dialog_updates_reactively_from_loading_to_loaded_scope() {
    let fake = FakeSettings::new(HelperBehaviour::Normal);
    *fake
        .shutdown_delay
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Duration::from_millis(250);
    *fake
        .shutdown_plan
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = devcore::shutdown::ShutdownPlan {
        instances: vec![
            devcore::shutdown::ShutdownResource {
                id: "app".to_string(),
                name: "app".to_string(),
            },
            devcore::shutdown::ShutdownResource {
                id: "cache".to_string(),
                name: "cache".to_string(),
            },
        ],
        fingerprint: "fp-2".to_string(),
    };

    let turn = serial();
    use_fake_providers();
    use_fake_software();
    use_fake_development();
    use_fake_databases();
    use_fake_proxy();
    use_fake_guide();
    use_fake_system();
    install_system_settings_provider(fake.clone());

    let mut app = Headless::start(EngineConfig::new(Root::of::<QuitDialogHarness>())).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("id:trigger-quit").unwrap();

    // Open quit dialog: initially shows loading text while preview_shutdown runs.
    app.press("id:trigger-quit").unwrap();
    app.wait_for("id:quit-dialog").unwrap();
    app.wait_for("Reading managed service scope…").unwrap();

    // Once preview_shutdown resolves, dialog updates reactively to show scope,
    // running instance IDs, and enabled actions.
    app.wait_for_within("Stop scope: 2 managed databases", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:quit-dialog-backdrop").unwrap();
    app.wait_for("id:quit-instance-list").unwrap();
    app.wait_for("id:quit-instance-app").unwrap();
    app.wait_for("id:quit-instance-cache").unwrap();
    assert!(app.screen().shows("app"));
    assert!(app.screen().shows("cache"));
    app.wait_for("Keep services and quit").unwrap();
    app.wait_for("Stop managed databases and quit").unwrap();

    // Pressing "Stop managed databases and quit" executes provider.shutdown(&plan).
    app.press("id:quit-stop").unwrap();
    app.wait_for_gone("id:quit-dialog", Duration::from_secs(5))
        .unwrap();
    assert!(fake.calls().iter().any(|call| call == "preview shutdown"));
    assert!(fake.calls().iter().any(|call| call == "shutdown"));
    drop(turn);
}

#[test]
fn the_helper_and_cli_status_render() {
    let fake = FakeSettings::new(HelperBehaviour::Normal);
    {
        let mut snapshot = fake
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        snapshot.helper = SystemHelperStatus {
            installed: true,
            message: "Installed. Root operations run through it without password prompts."
                .to_string(),
        };
        snapshot.cli = SystemCliStatus {
            installed: true,
            wrapper_path: "/tmp/devbase-tests/bin/devbase".to_string(),
            target_executable: "/tmp/devbase-tests/Devbase".to_string(),
            in_path: false,
            managed_by_devbase: true,
        };
        snapshot.cache = SystemCacheInfo {
            directory: "/tmp/devbase-tests/downloads".to_string(),
            files: 2,
            bytes: 2048,
        };
    }
    let (_turn, mut app) = start(fake);

    app.wait_for("Privileged helper").unwrap();
    app.wait_for("Installed. Root operations run through it without password prompts.")
        .unwrap();
    app.wait_for(devcore::helper::PLIST_PATH).unwrap();
    app.wait_for("id:settings-remove-helper").unwrap();
    app.wait_for("Command-line interface").unwrap();
    app.wait_for("/tmp/devbase-tests/bin/devbase").unwrap();
    app.wait_for("id:settings-remove-cli").unwrap();
    app.wait_for("Add /tmp/devbase-tests/bin to PATH to run devbase from any shell.")
        .unwrap();
    app.wait_for("2.0 KiB · 2 archives").unwrap();
    app.wait_for("Automatic log rotation").unwrap();
    app.wait_for("Updates & About").unwrap();
    app.wait_for("text:/Devbase v.* · macOS 26\\.0\\+ · xyz\\.jyeu\\.devbase/")
        .unwrap();

    // The grouped-card sheet is horizontally centered inside the Settings page.
    let nodes = app.screen().layout_nodes();
    let page = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("settings-page"))
        .expect("settings-page should be laid out");
    let card = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("settings-updates-card"))
        .expect("settings-updates-card should be laid out");
    let left_gutter = card.bounds.origin.x.0 - page.bounds.origin.x.0;
    let right_gutter = (page.bounds.origin.x.0 + page.bounds.size.width.0)
        - (card.bounds.origin.x.0 + card.bounds.size.width.0);
    assert!(
        (left_gutter - right_gutter).abs() <= 2.0,
        "settings sheet should be horizontally centered (left={left_gutter}, right={right_gutter})"
    );
}

#[test]
fn installing_the_helper_shows_the_busy_line_then_the_installed_status() {
    let fake = FakeSettings::new(HelperBehaviour::Slow);
    let (_turn, mut app) = start(fake.clone());

    app.wait_for("id:settings-install-helper").unwrap();
    app.wait_for("text:/Not installed\\. Root operations need it/")
        .unwrap();
    app.press("id:settings-install-helper").unwrap();
    app.wait_for("Installing helper…").unwrap();
    app.wait_for_within(
        "Installed. Root operations run through it without password prompts.",
        Duration::from_secs(5),
    )
    .unwrap();
    app.wait_for("Privileged helper installed").unwrap();
    assert!(fake.calls().iter().any(|call| call == "install helper"));
}

#[test]
fn a_helper_install_error_is_surfaced_verbatim() {
    let fake = FakeSettings::new(HelperBehaviour::Error);
    let (_turn, mut app) = start(fake.clone());

    app.wait_for("id:settings-install-helper").unwrap();
    app.wait_for("text:/Not installed\\. Root operations need it/")
        .unwrap();
    app.press("id:settings-install-helper").unwrap();
    app.wait_for("Could not install privileged helper: administrator password was rejected")
        .unwrap();
    // The passive status stays honest: still not installed.
    app.wait_for("text:/Not installed\\. Root operations need it/")
        .unwrap();
    assert!(fake.calls().iter().any(|call| call == "install helper"));
}

#[test]
fn installing_the_cli_wrapper_updates_its_status() {
    let fake = FakeSettings::new(HelperBehaviour::Normal);
    let (_turn, mut app) = start(fake.clone());

    app.wait_for("id:settings-install-cli").unwrap();
    app.wait_for("Not installed").unwrap();
    app.press("id:settings-install-cli").unwrap();
    app.wait_for("Installed devbase CLI at /tmp/devbase-tests/bin/devbase")
        .unwrap();
    app.wait_for("id:settings-remove-cli").unwrap();
    assert!(fake.calls().iter().any(|call| call == "install cli"));
}

struct RecordingSeams {
    calls: Mutex<Vec<String>>,
    code: u8,
}

impl RecordingSeams {
    fn new() -> Self {
        RecordingSeams {
            calls: Mutex::new(Vec::new()),
            code: 7,
        }
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
}

impl Seams for RecordingSeams {
    fn internal(&self, args: &[String]) -> ExitCode {
        self.record(format!("internal {}", args.join(" ")));
        ExitCode::from(self.code)
    }

    fn public_cli(&self, args: &[String]) -> ExitCode {
        self.record(format!("cli {}", args.join(" ")));
        ExitCode::from(self.code)
    }

    fn gui(&self, background: bool) -> ExitCode {
        self.record(format!("gui {background}"));
        ExitCode::from(self.code)
    }
}

#[test]
fn the_binary_dispatches_like_go_main() {
    let seams = RecordingSeams::new();
    assert_eq!(
        launch::run(&["--dev".to_string(), "serve".to_string()], &seams),
        ExitCode::from(7)
    );
    assert_eq!(
        launch::run(&["status".to_string(), "--json".to_string()], &seams),
        ExitCode::from(7)
    );
    assert_eq!(launch::run(&[], &seams), ExitCode::from(7));
    assert_eq!(
        launch::run(&["gui".to_string(), "status".to_string()], &seams),
        ExitCode::from(7)
    );
    assert_eq!(
        launch::run(&["--background".to_string()], &seams),
        ExitCode::from(7)
    );
    assert_eq!(
        launch::run(&["-psn_0_42".to_string(), "version".to_string()], &seams),
        ExitCode::from(7)
    );
    assert_eq!(
        seams.calls(),
        [
            "internal serve",
            "cli status --json",
            "gui false",
            "gui false",
            "gui true",
            "cli version",
        ]
    );
}

#[test]
fn a_dev_home_style_argv_is_not_mistaken_for_the_cli() {
    let seams = RecordingSeams::new();
    let _ = launch::run(
        &[
            "-NSFoo".to_string(),
            "YES".to_string(),
            "--background".to_string(),
        ],
        &seams,
    );
    let _ = launch::run(&["open".to_string()], &seams);
    assert_eq!(seams.calls(), ["gui true", "gui false"]);
}

#[test]
fn copying_ai_agent_instructions_and_mcp_config_populates_clipboard() {
    let fake = FakeSettings::new(HelperBehaviour::Normal);
    let (_turn, mut app) = start(fake);

    app.wait_for("id:settings-copy-agent-prompt").unwrap();
    app.press("id:settings-copy-agent-prompt").unwrap();
    app.wait_for("Copied AI agent instructions").unwrap();
    let prompt = app.screen().clipboard().expect("clipboard populated");
    assert!(prompt.contains("devbase schema"), "{prompt}");
    assert!(prompt.contains("--json"), "{prompt}");
    assert!(prompt.contains("--password-stdin"), "{prompt}");
    assert!(prompt.contains("--token-stdin"), "{prompt}");
    assert!(prompt.contains("devbase mcp"), "{prompt}");

    app.wait_for("id:settings-copy-mcp-config").unwrap();
    app.press("id:settings-copy-mcp-config").unwrap();
    app.wait_for("Copied MCP config").unwrap();
    let mcp_json = app.screen().clipboard().expect("clipboard populated");
    let parsed: serde_json::Value = serde_json::from_str(mcp_json).expect("valid MCP JSON");
    assert_eq!(
        parsed["mcpServers"]["devbase"]["command"],
        "/tmp/devbase-tests/bin/devbase"
    );
    assert_eq!(
        parsed["mcpServers"]["devbase"]["args"],
        serde_json::json!(["mcp"])
    );
}

struct FakeUpdater {
    feed: UpdateFeedInfo,
    check_result: UpdateState,
    install_result: Result<(), String>,
    installed_versions: Mutex<Vec<String>>,
}

impl FakeUpdater {
    fn new(feed: UpdateFeedInfo, check_result: UpdateState) -> Arc<Self> {
        Arc::new(FakeUpdater {
            feed,
            check_result,
            install_result: Ok(()),
            installed_versions: Mutex::new(Vec::new()),
        })
    }

    fn installed(&self) -> Vec<String> {
        self.installed_versions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl UpdateProvider for FakeUpdater {
    fn feed_info(&self) -> UpdateFeedInfo {
        self.feed.clone()
    }

    fn check(&self) -> UpdateState {
        self.check_result.clone()
    }

    fn install(&self, update: &AvailableUpdate) -> Result<(), String> {
        self.installed_versions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(update.version.clone());
        self.install_result.clone()
    }
}

#[test]
fn updates_card_reports_not_configured_honestly() {
    let fake = FakeSettings::new(HelperBehaviour::Normal);
    let updater = FakeUpdater::new(UpdateFeedInfo::not_configured(), UpdateState::NotConfigured);
    let (_turn, mut app) = start_with_updater(fake, Some(updater));

    app.wait_for("id:settings-updates-card").unwrap();
    app.wait_for("No signed update feed is bundled with this build.")
        .unwrap();
    app.press("id:settings-check-updates").unwrap();
    app.wait_for("No signed update feed is bundled with this build.")
        .unwrap();
}

#[test]
fn updates_card_reports_up_to_date() {
    let fake = FakeSettings::new(HelperBehaviour::Normal);
    let updater = FakeUpdater::new(
        UpdateFeedInfo {
            configured: true,
            channel: "Stable (signed)".to_string(),
            source: Some("https://releases.devbase.example/latest.json".to_string()),
        },
        UpdateState::UpToDate,
    );
    let (_turn, mut app) = start_with_updater(fake, Some(updater));

    app.wait_for("id:settings-updates-card").unwrap();
    app.wait_for("Stable (signed)").unwrap();
    app.press("id:settings-check-updates").unwrap();
    app.wait_for("Up to date").unwrap();
    app.wait_for("Devbase 0.1.0 is the latest verified version.")
        .unwrap();
}

#[test]
fn updates_card_shows_available_update_notes_and_installs() {
    let fake = FakeSettings::new(HelperBehaviour::Normal);
    let updater = FakeUpdater::new(
        UpdateFeedInfo {
            configured: true,
            channel: "Stable (signed)".to_string(),
            source: Some("https://releases.devbase.example/latest.json".to_string()),
        },
        UpdateState::Available(AvailableUpdate {
            version: "0.2.0".to_string(),
            notes: Some("- Built-in stdio MCP server\n- Signed updater".to_string()),
            url: "https://releases.devbase.example/Devbase-0.2.0-macos-arm64.tar.gz".to_string(),
            sha256: Some("deadbeef".to_string()),
            size: Some(4096),
            can_install: true,
        }),
    );
    let (_turn, mut app) = start_with_updater(fake, Some(updater.clone()));

    app.wait_for("id:settings-updates-card").unwrap();
    app.press("id:settings-check-updates").unwrap();
    app.wait_for("Update available").unwrap();
    app.wait_for("Version 0.2.0 is available and verified.")
        .unwrap();
    app.wait_for("id:settings-update-notes").unwrap();
    app.wait_for("id:settings-install-update").unwrap();

    app.press("id:settings-install-update").unwrap();
    app.wait_for("Devbase 0.2.0 is installed. Restart to apply the update.")
        .unwrap();
    app.wait_for("id:settings-restart-update").unwrap();
    assert_eq!(updater.installed(), vec!["0.2.0".to_string()]);
}

#[test]
fn updates_card_surfaces_verification_errors_verbatim() {
    let fake = FakeSettings::new(HelperBehaviour::Normal);
    let updater = FakeUpdater::new(
        UpdateFeedInfo {
            configured: true,
            channel: "Stable (signed)".to_string(),
            source: Some("https://releases.devbase.example/latest.json".to_string()),
        },
        UpdateState::Error(
            "update manifest: the signature does not match the app's publicKey".to_string(),
        ),
    );
    let (_turn, mut app) = start_with_updater(fake, Some(updater));

    app.wait_for("id:settings-updates-card").unwrap();
    app.press("id:settings-check-updates").unwrap();
    app.wait_for("update manifest: the signature does not match the app's publicKey")
        .unwrap();
}
