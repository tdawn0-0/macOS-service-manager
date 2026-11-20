//! The shell's first frame and navigation, headless: the sidebar lists all
//! six sections and choosing one switches the workspace.

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

fn start() -> Headless {
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<nsh_app_devbase::App>())).unwrap();
    app.wait_for("Services").unwrap();
    app
}

#[test]
fn sidebar_lists_the_six_sections() {
    let app = start();
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
    let mut app = start();
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
