use std::path::{Path, PathBuf};
use std::sync::Arc;
use super::test_support::{FakeHost, FakeRunner, TempDir, write_file};
use super::*;
fn host_for(dir: &Path) -> Arc<FakeHost> {
    FakeHost::new(dir, dir)
}

#[test]
fn login_agent_paths_and_target() {
    assert_eq!(login_agent_plist_path(Path::new("")), None);
    assert_eq!(
        login_agent_plist_path(Path::new("/x/LaunchAgents")),
        Some(PathBuf::from(
            "/x/LaunchAgents/xyz.jyeu.devbase.login.plist"
        ))
    );
    assert_eq!(login_agent_target(501), "gui/501/xyz.jyeu.devbase.login");
}
use super::test_support::{FakeHost, FakeRunner, TempDir, write_file};
use super::*;

fn host_for(dir: &Path) -> Arc<FakeHost> {
    FakeHost::new(dir, dir)
}

#[test]
fn login_agent_paths_and_target() {
    assert_eq!(login_agent_plist_path(Path::new("")), None);
    assert_eq!(
        login_agent_plist_path(Path::new("/x/LaunchAgents")),
        Some(PathBuf::from(
            "/x/LaunchAgents/xyz.jyeu.devbase.login.plist"
        ))
    );
    assert_eq!(login_agent_target(501), "gui/501/xyz.jyeu.devbase.login");
}

#[test]
fn resolve_prefers_an_absolute_override() {
    let dir = TempDir::new();
    let host = host_for(dir.path());
    assert_eq!(
        resolve_login_executable(&*host, None, Some(Path::new("/opt/devbase"))).expect("override"),
        PathBuf::from("/opt/devbase")
    );
    let error = resolve_login_executable(&*host, None, Some(Path::new("devbase")))
        .expect_err("relative override");
    assert_eq!(error.message, "executable path must be absolute");
}

#[test]
fn resolve_reads_the_bundle_executable() {
    let dir = TempDir::new();
    let host = host_for(dir.path());
    let app = dir.path().join("Devbase.app");
    write_file(
        &app,
        "Contents/Info.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>Devbase</string></dict></plist>"#,
    );
    let executable = write_file(&app, "Contents/MacOS/Devbase", b"binary");
    assert_eq!(
        resolve_login_executable(&*host, Some(&app), None).expect("bundle"),
        executable
    );

    let bare = dir.path().join("Bare.app");
    std::fs::create_dir_all(bare.join("Contents/MacOS")).expect("bare app");
    write_file(
        &bare,
        "Contents/Info.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>Missing</string></dict></plist>"#,
    );
    assert_eq!(
        resolve_login_executable(&*host, Some(&bare), None).expect("fallback"),
        PathBuf::from("/Applications/Devbase.app/Contents/MacOS/Devbase")
    );
}

#[test]
fn resolve_falls_back_to_the_current_executable_and_its_symlinks() {
    let dir = TempDir::new();
    let host = host_for(dir.path());
    let real = write_file(dir.path(), "real-binary", b"binary");
    let link = dir.path().join("link-binary");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    host.set_exe(&link);
    assert_eq!(
        resolve_login_executable(&*host, None, None).expect("current exe"),
        std::fs::canonicalize(&real).expect("canonical")
    );

    host.set_exe(Path::new("definitely-not-here"));
    let error = resolve_login_executable(&*host, None, None).expect_err("relative");
    assert_eq!(error.message, "executable path must be absolute");
}

#[test]
fn enable_writes_the_login_agent_and_ignores_runner_errors() {
    let dir = TempDir::new();
    let runner = FakeRunner::new("");
    runner.fail("launchctl", "launchctl: no such job");
    let launch_agents = dir.path().join("LaunchAgents");
    let exe = Path::new("/Applications/Devbase.app/Contents/MacOS/Devbase");

    enable_launch_at_login(&launch_agents, exe, &*runner).expect("enable");
    let path = launch_agents.join("xyz.jyeu.devbase.login.plist");
    assert!(path.is_file());
    let spec = Spec::load(&path).expect("load login agent");
    assert_eq!(spec.label, LOGIN_AGENT_LABEL);
    assert_eq!(
        spec.args,
        vec![
            exe.to_string_lossy().into_owned(),
            "--background".to_string()
        ]
    );
    assert!(spec.run_at_load);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
    }

    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][0], "launchctl");
    assert_eq!(calls[0][1], "enable");
    assert!(calls[0][2].starts_with("gui/"));
    assert!(calls[0][2].ends_with(LOGIN_AGENT_LABEL));

    let leftovers: Vec<PathBuf> = std::fs::read_dir(&launch_agents)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".devbase-login-"))
        })
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn enable_rejects_a_missing_directory_or_relative_executable() {
    let dir = TempDir::new();
    let runner = FakeRunner::new("");
    let error = enable_launch_at_login(Path::new(""), Path::new("/Applications/Devbase"), &*runner)
        .expect_err("no directory");
    assert_eq!(error.message, "login agent directory is not configured");

    let error = enable_launch_at_login(dir.path(), Path::new("Devbase"), &*runner)
        .expect_err("relative executable");
    assert_eq!(error.message, "executable path must be absolute");
    assert!(runner.calls().is_empty());
}

#[test]
fn disable_removes_the_file_and_ignores_runner_errors() {
    let dir = TempDir::new();
    let runner = FakeRunner::new("");
    runner.fail("launchctl", "launchctl: not loaded");
    let path = write_file(dir.path(), "xyz.jyeu.devbase.login.plist", b"plist");
    disable_launch_at_login(dir.path(), &*runner).expect("disable");
    assert!(!path.exists());
    // Removing it again is fine.
    disable_launch_at_login(dir.path(), &*runner).expect("disable again");
    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0][1], "bootout");
    assert_eq!(calls[1][1], "bootout");

    let error = disable_launch_at_login(Path::new(""), &*runner).expect_err("no directory");
    assert_eq!(error.message, "login agent directory is not configured");
}

#[test]
fn launch_at_login_enabled_reports_a_regular_file() {
    let dir = TempDir::new();
    assert!(!launch_at_login_enabled(Path::new("")));
    assert!(!launch_at_login_enabled(dir.path()));
    let path = dir.path().join("xyz.jyeu.devbase.login.plist");
    std::fs::create_dir_all(&path).expect("directory at the plist path");
    assert!(!launch_at_login_enabled(dir.path()));
    std::fs::remove_dir(&path).expect("remove directory");
    std::fs::write(&path, b"plist").expect("write");
    assert!(launch_at_login_enabled(dir.path()));
}
