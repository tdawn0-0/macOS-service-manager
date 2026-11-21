//! macOS platform paths, desktop helpers, and login item delegation.

use std::io;
use std::path::Path;

/// macOS storage and runtime paths.
pub mod paths {
    use std::path::{Path, PathBuf};

    /// The production Homebrew Cellar on Apple Silicon.
    pub const DEFAULT_BREW_CELLAR: &str = "/opt/homebrew/Cellar";

    /// Fallback Devbase executable path inside `/Applications`.
    pub const FALLBACK_PROGRAM_PATH: &str = "/Applications/Devbase.app/Contents/MacOS/Devbase";

    /// `~/Library/Application Support/Devbase/Development`.
    pub fn default_storage_root(home: &Path) -> PathBuf {
        home.join("Library")
            .join("Application Support")
            .join("Devbase")
            .join("Development")
    }

    /// `~/Library/Caches/Devbase/Development`.
    pub fn user_cache_directory(home: &Path) -> PathBuf {
        home.join("Library")
            .join("Caches")
            .join("Devbase")
            .join("Development")
    }

    /// `~/Library/Java/JavaVirtualMachines`.
    pub fn user_java_directory(home: &Path) -> PathBuf {
        home.join("Library")
            .join("Java")
            .join("JavaVirtualMachines")
    }

    /// `~/Library/LaunchAgents`.
    pub fn autostart_directory(home: &Path) -> PathBuf {
        home.join("Library").join("LaunchAgents")
    }

    /// Default Devbase executable path: current executable, else `/Applications/Devbase.app/...`.
    pub fn default_program_path() -> PathBuf {
        std::env::current_exe()
            .ok()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from(FALLBACK_PROGRAM_PATH))
    }
}

/// macOS desktop integration helpers (`open`, Finder, Console, Terminal, `~/.Trash`).
pub mod desktop {
    use std::ffi::OsStr;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    pub fn open_url(url: &str) -> io::Result<()> {
        spawn_detached("/usr/bin/open", &[OsStr::new(url)])
    }

    pub fn open_path(path: &Path) -> io::Result<()> {
        spawn_detached("/usr/bin/open", &[path.as_os_str()])
    }

    pub fn reveal_path(path: &Path) -> io::Result<()> {
        spawn_detached("/usr/bin/open", &[OsStr::new("-R"), path.as_os_str()])
    }

    pub fn open_log_file(path: &Path) -> io::Result<()> {
        spawn_detached(
            "/usr/bin/open",
            &[OsStr::new("-a"), OsStr::new("Console"), path.as_os_str()],
        )
    }

    pub fn open_in_terminal(script_path: &Path) -> io::Result<()> {
        spawn_detached(
            "/usr/bin/open",
            &[
                OsStr::new("-a"),
                OsStr::new("Terminal"),
                script_path.as_os_str(),
            ],
        )
    }

    pub fn trash_path(path: &Path) -> io::Result<()> {
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        trash_path_in_home(path, &home)
    }

    /// Moves `path` into `<home>/.Trash`, appending `" HH.MM.SS"` on name collision.
    pub fn trash_path_in_home(path: &Path, home: &Path) -> io::Result<()> {
        let trash = home.join(".Trash");
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the file has no name"))?;
        let mut destination = trash.join(name);
        if std::fs::symlink_metadata(&destination).is_ok() {
            destination = with_time_suffix(&destination, &local_hms());
        }
        std::fs::rename(path, &destination)
    }

    fn spawn_detached(program: &str, args: &[&OsStr]) -> io::Result<()> {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    }

    fn with_time_suffix(path: &Path, suffix: &str) -> PathBuf {
        let name = path
            .file_name()
            .unwrap_or_else(|| OsStr::new(""))
            .to_string_lossy();
        let (stem, extension) = match name.rfind('.') {
            Some(index) => name.split_at(index),
            None => (name.as_ref(), ""),
        };
        let file = format!("{stem} {suffix}{extension}");
        match path.parent() {
            Some(parent) => parent.join(file),
            None => PathBuf::from(file),
        }
    }

    #[cfg(unix)]
    fn local_hms() -> String {
        // SAFETY: `now` and `tm` are valid pointers for `localtime_r`.
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return "00.00.00".to_string();
            }
            format!("{:02}.{:02}.{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
        }
    }

    #[cfg(not(unix))]
    fn local_hms() -> String {
        "00.00.00".to_string()
    }
}

/// macOS login item management, delegating directly to `crate::launchd::login_agent`.
pub mod login_item {
    use std::path::{Path, PathBuf};

    use crate::launchd::{self, Host, LaunchdError, ProcessRunner};

    /// The default directory storing Devbase's login LaunchAgent (`~/Library/LaunchAgents`).
    pub fn default_dir(home: &Path) -> PathBuf {
        super::paths::autostart_directory(home)
    }

    /// The plist path inside `dir`.
    pub fn entry_path(dir: &Path) -> Option<PathBuf> {
        launchd::login_agent_plist_path(dir)
    }

    /// Whether the login LaunchAgent is present in `dir`.
    pub fn is_enabled(dir: &Path) -> bool {
        launchd::launch_at_login_enabled(dir)
    }

    /// Writes the login LaunchAgent in `dir` and enables it in launchd.
    pub fn enable(dir: &Path, exe: &Path, runner: &dyn ProcessRunner) -> Result<(), LaunchdError> {
        launchd::enable_launch_at_login(dir, exe, runner)
    }

    /// Removes the login LaunchAgent from `dir` and boots it out of launchd.
    pub fn disable(dir: &Path, runner: &dyn ProcessRunner) -> Result<(), LaunchdError> {
        launchd::disable_launch_at_login(dir, runner)
    }

    /// Resolves the executable recorded in the login LaunchAgent.
    pub fn resolve_executable(
        host: &dyn Host,
        app_path: Option<&Path>,
        override_path: Option<&Path>,
    ) -> Result<PathBuf, LaunchdError> {
        launchd::resolve_login_executable(host, app_path, override_path)
    }
}

/// Convenience wrapper for `desktop::trash_path_in_home`.
pub fn trash_path_in_home(path: &Path, home: &Path) -> io::Result<()> {
    desktop::trash_path_in_home(path, home)
}
