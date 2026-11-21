//! The shell's first frame, navigation, and macOS desktop chrome, headless:
//! every store runs on fake providers so tests never touch real launchd,
//! Homebrew, devcore state, or host preferences.

use std::sync::{Mutex, MutexGuard};

use nsh_app_devbase::testing::{
    use_fake_databases, use_fake_dev_logs, use_fake_development, use_fake_editor, use_fake_guide,
    use_fake_providers, use_fake_proxy, use_fake_software, use_fake_system,
};
use nsh_core::{
    desktop::{WindowMode, WindowState},
    platform::Os,
    prelude::*,
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn install_all_fakes() {
    use_fake_providers();
    use_fake_software();
    use_fake_development();
    use_fake_databases();
    use_fake_proxy();
    use_fake_system();
    use_fake_guide();
    use_fake_dev_logs();
    use_fake_editor();
}

fn start() -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    install_all_fakes();
    let mut app = Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.wait_for("Services").unwrap();
    (turn, app)
}

fn start_macos() -> (MutexGuard<'static, ()>, Headless) {
    let turn = serial();
    install_all_fakes();
    let mut config = EngineConfig::new(Root::of::<nsh_app_devbase::App>());
    config.environment.os = Os::MacOs;
    config.environment.title_bar.top = px(52.0);
    let mut app = Headless::start(config).unwrap();
    app.set_viewport(Size::new(px(1100.0), px(720.0))).unwrap();
    app.wait_for("Services").unwrap();
    (turn, app)
}

#[test]
fn sidebar_lists_the_six_sections() {
    let (_turn, app) = start();
    for section in [
        "Services",
        "Development",
        "Databases",
        "HTTPS Proxy",
        "Diagnostics",
        "Settings",
    ] {
        assert!(
            app.screen().shows(section),
            "the sidebar lacks the {section} row"
        );
    }
}

#[test]
fn choosing_a_section_switches_the_workspace() {
    let (_turn, mut app) = start();
    app.wait_for("launchd jobs, their status and lifecycle controls.")
        .unwrap();

    app.press("role:button[Databases]").unwrap();
    app.wait_for("Database instances, ports, connection profiles and backups.")
        .unwrap();

    app.press("role:button[Settings]").unwrap();
    app.wait_for("Helper installation, CLI, login items and workspace preferences.")
        .unwrap();

    app.press("role:button[Services]").unwrap();
    app.wait_for("launchd jobs, their status and lifecycle controls.")
        .unwrap();
}

#[test]
fn macos_sidebar_uses_liquid_glass_and_falls_back_in_full_screen() {
    let (_turn, mut app) = start_macos();

    // Outside full screen on macOS, the Liquid Glass backdrop layer and
    // hidden title-bar traffic-light reservation strip are both mounted.
    app.wait_for("id:sidebar-glass").unwrap();
    app.wait_for("id:sidebar-title-bar").unwrap();
    assert_eq!(app.screen().count("id:sidebar-glass").unwrap(), 1);
    assert_eq!(app.screen().count("id:sidebar-title-bar").unwrap(), 1);

    // Entering full screen removes the glass backdrop and falls back to solid sidebar fill.
    let mut full = WindowState::default();
    full.mode = WindowMode::FullScreen;
    app.set_environment("enter full screen", |env| {
        env.window_state = Some(full);
        env.title_bar.top = px(0.0);
    })
    .unwrap();
    app.wait_for_gone("id:sidebar-glass", std::time::Duration::from_secs(2))
        .unwrap();

    // Leaving full screen restores Liquid Glass and the title-bar strip.
    let mut normal = WindowState::default();
    normal.mode = WindowMode::Normal;
    app.set_environment("leave full screen", |env| {
        env.window_state = Some(normal);
        env.title_bar.top = px(52.0);
    })
    .unwrap();
    app.wait_for("id:sidebar-glass").unwrap();
    app.wait_for("id:sidebar-title-bar").unwrap();
}

#[test]
fn services_lists_all_five_sources_and_sidebar_toggles_log_drawer() {
    let (_turn, mut app) = start();
    for source in [
        "Homebrew",
        "Login Items",
        "User Agents",
        "System Agents",
        "Daemons",
    ] {
        assert!(
            app.screen().shows(source),
            "Services rail lacks the {source} source"
        );
    }

    assert_eq!(app.screen().count("id:dev-log-drawer").unwrap(), 0);
    app.press("id:sidebar-logs-toggle").unwrap();
    app.wait_for("id:dev-log-drawer").unwrap();
    assert_eq!(app.screen().count("id:dev-log-drawer").unwrap(), 1);

    app.press("id:sidebar-logs-toggle").unwrap();
    app.wait_for_gone("id:dev-log-drawer", std::time::Duration::from_secs(2))
        .unwrap();
}
