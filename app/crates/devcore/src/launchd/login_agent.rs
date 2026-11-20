//! The LaunchAgent that starts Devbase quietly at login.

use std::io::Write;
use std::path::{Path, PathBuf};

use plist::Value;

use super::error::LaunchdError;
use super::host::{Host, ProcessRunner};
use super::spec::Spec;

/// The LaunchAgent label used when Devbase starts quietly in the menu bar at
/// login.
pub const LOGIN_AGENT_LABEL: &str = "xyz.jyeu.devbase.login";

/// The property list path inside `dir` for the login LaunchAgent.
pub fn login_agent_plist_path(dir: &Path) -> Option<PathBuf> {
    if dir.as_os_str().is_empty() {
        return None;
    }
    Some(dir.join(format!("{LOGIN_AGENT_LABEL}.plist")))
}

/// The `gui/<uid>/<label>` launchd domain target.
pub fn login_agent_target(uid: u32) -> String {
    format!("gui/{uid}/{LOGIN_AGENT_LABEL}")
}

/// The executable path recorded in the login LaunchAgent.
pub fn resolve_login_executable(
    host: &dyn Host,
    app_path: Option<&Path>,
    override_path: Option<&Path>,
) -> Result<PathBuf, LaunchdError> {
    if let Some(override_path) = override_path.filter(|path| !path.as_os_str().is_empty()) {
        if !override_path.is_absolute() {
            return Err(LaunchdError::validation("executable path must be absolute"));
        }
        return Ok(override_path.to_path_buf());
    }
    if let Some(app_path) = app_path.filter(|path| !path.as_os_str().is_empty()) {
        let info_path = app_path.join("Contents").join("Info.plist");
        if let Ok(data) = std::fs::read(&info_path)
            && let Ok(Value::Dictionary(info)) = Value::from_reader(std::io::Cursor::new(data))
            && let Some(Value::String(executable)) = info.get("CFBundleExecutable")
            && !executable.is_empty()
        {
            let candidate = app_path.join("Contents").join("MacOS").join(executable);
            if let Ok(metadata) = std::fs::metadata(&candidate)
                && metadata.is_file()
            {
                return Ok(candidate);
            }
        }
    }
    let exe = host.current_exe()?;
    let exe = host
        .eval_symlinks(&exe)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(exe);
    if !exe.is_absolute() {
        return Err(LaunchdError::validation("executable path must be absolute"));
    }
    Ok(exe)
}

/// Whether the login LaunchAgent property list exists in `dir`.
pub fn launch_at_login_enabled(dir: &Path) -> bool {
    let Some(path) = login_agent_plist_path(dir) else {
        return false;
    };
    std::fs::metadata(path).is_ok_and(|info| info.is_file())
}

/// Writes the login LaunchAgent property list in `dir` for `exe` with
/// `--background`.
pub fn enable_launch_at_login(
    dir: &Path,
    exe: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(), LaunchdError> {
    let path = login_agent_plist_path(dir)
        .ok_or_else(|| LaunchdError::validation("login agent directory is not configured"))?;
    if exe.as_os_str().is_empty() || !exe.is_absolute() {
        return Err(LaunchdError::validation("executable path must be absolute"));
    }
    let spec = Spec {
        label: LOGIN_AGENT_LABEL.to_string(),
        args: vec![
            exe.to_string_lossy().into_owned(),
            "--background".to_string(),
        ],
        run_at_load: true,
        ..Spec::default()
    };
    let data = spec.marshal(None)?;
    std::fs::create_dir_all(dir).map_err(|error| LaunchdError::io(error.to_string()))?;
    let tmp = unique_login_path(dir);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|error| LaunchdError::io(error.to_string()))?;
    if let Err(error) = file.write_all(&data) {
        let _ = std::fs::remove_file(&tmp);
        return Err(LaunchdError::io(error.to_string()));
    }
    drop(file);
    if let Err(error) = set_mode(&tmp, 0o644) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(LaunchdError::io(error.to_string()));
    }
    // Launchd may not know of the job yet; that is not a failure here.
    let target = login_agent_target(uid());
    let _ = runner.run("launchctl", &["enable", &target]);
    Ok(())
}

/// Removes the login LaunchAgent property list from `dir` and boots it out via
/// the runner.
pub fn disable_launch_at_login(dir: &Path, runner: &dyn ProcessRunner) -> Result<(), LaunchdError> {
    let path = login_agent_plist_path(dir)
        .ok_or_else(|| LaunchdError::validation("login agent directory is not configured"))?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(LaunchdError::io(error.to_string())),
    }
    let target = login_agent_target(uid());
    let _ = runner.run("launchctl", &["bootout", &target]);
    Ok(())
}

fn uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: getuid has no preconditions and cannot fail.
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn unique_login_path(dir: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".devbase-login-{}-{n}.plist", std::process::id()));
        if std::fs::symlink_metadata(&path).is_err() {
            return path;
        }
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), LaunchdError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|error| LaunchdError::io(error.to_string()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}
