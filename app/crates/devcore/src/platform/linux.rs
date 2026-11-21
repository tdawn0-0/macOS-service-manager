//! Linux platform paths (XDG Base Directory Specification), desktop helpers
//! (`xdg-open`, terminal detection, Freedesktop Trash), and XDG autostart login
//! item management.

use std::io;
use std::path::Path;

/// Linux XDG-compliant storage and runtime paths.
pub mod paths {
    use std::path::{Path, PathBuf};

    /// The default Linuxbrew Cellar location.
    pub const DEFAULT_BREW_CELLAR: &str = "/home/linuxbrew/.linuxbrew/Cellar";

    /// Fallback executable name when `current_exe()` is unavailable.
    pub const FALLBACK_PROGRAM_PATH: &str = "devbase";

    /// Resolves `$XDG_DATA_HOME` (if set and absolute) or `<home>/.local/share`.
    pub fn xdg_data_home(home: &Path) -> PathBuf {
        xdg_dir_from_env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"))
    }

    /// Resolves `$XDG_CACHE_HOME` (if set and absolute) or `<home>/.cache`.
    pub fn xdg_cache_home(home: &Path) -> PathBuf {
        xdg_dir_from_env("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"))
    }

    /// Resolves `$XDG_CONFIG_HOME` (if set and absolute) or `<home>/.config`.
    pub fn xdg_config_home(home: &Path) -> PathBuf {
        xdg_dir_from_env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"))
    }

    /// `$XDG_DATA_HOME/devbase` (default `~/.local/share/devbase`).
    pub fn default_storage_root(home: &Path) -> PathBuf {
        default_storage_root_with_xdg(home, xdg_dir_from_env("XDG_DATA_HOME").as_deref())
    }

    /// Pure testable form of [`default_storage_root`].
    pub fn default_storage_root_with_xdg(home: &Path, xdg_data_home: Option<&Path>) -> PathBuf {
        let base = xdg_data_home
            .filter(|dir| dir.is_absolute())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".local").join("share"));
        base.join("devbase")
    }

    /// `$XDG_CACHE_HOME/devbase` (default `~/.cache/devbase`).
    pub fn user_cache_directory(home: &Path) -> PathBuf {
        user_cache_directory_with_xdg(home, xdg_dir_from_env("XDG_CACHE_HOME").as_deref())
    }

    /// Pure testable form of [`user_cache_directory`].
    pub fn user_cache_directory_with_xdg(home: &Path, xdg_cache_home: Option<&Path>) -> PathBuf {
        let base = xdg_cache_home
            .filter(|dir| dir.is_absolute())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".cache"));
        base.join("devbase")
    }

    /// `$XDG_DATA_HOME/devbase/jdks` (default `~/.local/share/devbase/jdks`).
    pub fn user_java_directory(home: &Path) -> PathBuf {
        default_storage_root(home).join("jdks")
    }

    /// Pure testable form of [`user_java_directory`].
    pub fn user_java_directory_with_xdg(home: &Path, xdg_data_home: Option<&Path>) -> PathBuf {
        default_storage_root_with_xdg(home, xdg_data_home).join("jdks")
    }

    /// `$XDG_CONFIG_HOME/autostart` (default `~/.config/autostart`).
    pub fn autostart_directory(home: &Path) -> PathBuf {
        xdg_config_home(home).join("autostart")
    }

    /// `$XDG_CONFIG_HOME/systemd/user` (default `~/.config/systemd/user`).
    pub fn systemd_user_unit_dir(home: &Path) -> PathBuf {
        xdg_config_home(home).join("systemd").join("user")
    }

    /// Default Devbase executable path: current executable, else `devbase`.
    pub fn default_program_path() -> PathBuf {
        std::env::current_exe()
            .ok()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from(FALLBACK_PROGRAM_PATH))
    }

    fn xdg_dir_from_env(var: &str) -> Option<PathBuf> {
        let value = std::env::var_os(var)?;
        if value.is_empty() {
            return None;
        }
        let path = PathBuf::from(value);
        path.is_absolute().then_some(path)
    }
}

/// Linux desktop integration helpers (`xdg-open`, terminal detection, Freedesktop Trash).
pub mod desktop {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    /// Ordered fallback list of Linux terminal emulators checked after `$TERMINAL`.
    pub const TERMINAL_CANDIDATES: [&str; 7] = [
        "x-terminal-emulator",
        "konsole",
        "gnome-terminal",
        "kitty",
        "alacritty",
        "foot",
        "xterm",
    ];

    pub fn open_url(url: &str) -> io::Result<()> {
        spawn_detached("xdg-open", &[OsStr::new(url)])
    }

    pub fn open_path(path: &Path) -> io::Result<()> {
        spawn_detached("xdg-open", &[path.as_os_str()])
    }

    /// Reveals `path` in the user's file manager. Attempts the Freedesktop
    /// `org.freedesktop.FileManager1.ShowItems` D-Bus call first, falling back
    /// to opening the parent directory with `xdg-open`.
    pub fn reveal_path(path: &Path) -> io::Result<()> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let uri = file_uri(&canonical);
        let dbus_ok = Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{uri}"),
                "string:",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if dbus_ok {
            return Ok(());
        }
        let target = if canonical.is_dir() {
            canonical.as_path()
        } else {
            canonical.parent().unwrap_or(canonical.as_path())
        };
        open_path(target)
    }

    pub fn open_log_file(path: &Path) -> io::Result<()> {
        open_path(path)
    }

    /// Launches `script_path` in the first available Linux terminal emulator.
    pub fn open_in_terminal(script_path: &Path) -> io::Result<()> {
        let terminal_env = std::env::var("TERMINAL").ok();
        let path_env = std::env::var_os("PATH");
        let Some((program, args)) =
            resolve_terminal_command(terminal_env.as_deref(), path_env.as_deref(), script_path)
        else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no supported terminal emulator found in $TERMINAL or PATH",
            ));
        };
        let os_args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
        spawn_detached(&program.to_string_lossy(), &os_args)
    }

    /// Resolves `(program, args)` for opening `script_path` in a Linux terminal.
    pub fn resolve_terminal_command(
        terminal_env: Option<&str>,
        path_env: Option<&OsStr>,
        script_path: &Path,
    ) -> Option<(PathBuf, Vec<OsString>)> {
        let mut candidates: Vec<String> = Vec::with_capacity(TERMINAL_CANDIDATES.len() + 1);
        if let Some(term) = terminal_env.map(str::trim).filter(|t| !t.is_empty()) {
            candidates.push(term.to_string());
        }
        for name in TERMINAL_CANDIDATES {
            if !candidates.iter().any(|c| c == name) {
                candidates.push(name.to_string());
            }
        }
        for candidate in candidates {
            if let Some(resolved) = find_executable(&candidate, path_env) {
                let args = terminal_exec_args(&resolved, script_path);
                return Some((resolved, args));
            }
        }
        None
    }

    /// Builds the terminal-specific argument vector to execute `script_path`.
    pub fn terminal_exec_args(terminal: &Path, script_path: &Path) -> Vec<OsString> {
        let name = terminal
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let script = script_path.as_os_str().to_os_string();
        match name.as_str() {
            "gnome-terminal" => vec![OsString::from("--"), script],
            "kitty" | "foot" => vec![script],
            // konsole, x-terminal-emulator, alacritty, xterm and generic terminals use `-e`
            _ => vec![OsString::from("-e"), script],
        }
    }

    /// Moves `path` to the Freedesktop XDG Trash (`$XDG_DATA_HOME/Trash`).
    pub fn trash_path(path: &Path) -> io::Result<()> {
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        let trash_dir = super::paths::xdg_data_home(&home).join("Trash");
        trash_path_into(path, &trash_dir)
    }

    /// Moves `path` into `<home>/.local/share/Trash` (or `$XDG_DATA_HOME/Trash`
    /// if set).
    pub fn trash_path_in_home(path: &Path, home: &Path) -> io::Result<()> {
        let trash_dir = super::paths::xdg_data_home(home).join("Trash");
        trash_path_into(path, &trash_dir)
    }

    /// Moves `path` into an explicit Freedesktop Trash directory (`files/` + `info/*.trashinfo`).
    pub fn trash_path_into(path: &Path, trash_dir: &Path) -> io::Result<()> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the file has no name"))?
            .to_string_lossy()
            .into_owned();
        let files_dir = trash_dir.join("files");
        let info_dir = trash_dir.join("info");
        std::fs::create_dir_all(&files_dir)?;
        std::fs::create_dir_all(&info_dir)?;

        let original_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let encoded_path = percent_encode_trash_path(&original_path.to_string_lossy());
        let deletion_date = local_iso8601();

        let (dest_name, info_path) = reserve_trash_name(&files_dir, &info_dir, &name)?;
        let info_content =
            format!("[Trash Info]\nPath={encoded_path}\nDeletionDate={deletion_date}\n");
        if let Err(error) = std::fs::write(&info_path, info_content.as_bytes()) {
            let _ = std::fs::remove_file(&info_path);
            return Err(error);
        }

        let dest_file = files_dir.join(&dest_name);
        if let Err(error) = move_entry(path, &dest_file) {
            let _ = std::fs::remove_file(&info_path);
            return Err(error);
        }
        Ok(())
    }

    fn reserve_trash_name(
        files_dir: &Path,
        info_dir: &Path,
        base_name: &str,
    ) -> io::Result<(String, PathBuf)> {
        let (stem, ext) = match base_name.rfind('.') {
            Some(idx) if idx > 0 => (&base_name[..idx], &base_name[idx..]),
            _ => (base_name, ""),
        };
        for attempt in 0u32..10_000 {
            let candidate = if attempt == 0 {
                base_name.to_string()
            } else {
                format!("{stem}.{attempt}{ext}")
            };
            let file_dest = files_dir.join(&candidate);
            let info_dest = info_dir.join(format!("{candidate}.trashinfo"));
            if std::fs::symlink_metadata(&file_dest).is_ok() {
                continue;
            }
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&info_dest)
            {
                Ok(_) => return Ok((candidate, info_dest)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate unique trash filename",
        ))
    }

    fn move_entry(source: &Path, destination: &Path) -> io::Result<()> {
        match std::fs::rename(source, destination) {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
                let meta = std::fs::symlink_metadata(source)?;
                if meta.is_file() {
                    std::fs::copy(source, destination)?;
                    let _ = std::fs::set_permissions(destination, meta.permissions());
                    std::fs::remove_file(source)
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Percent-encodes a path according to the Freedesktop Trash specification
    /// (preserving `/`, ASCII alphanumerics, and `-._~`).
    pub fn percent_encode_trash_path(path: &str) -> String {
        let mut out = String::with_capacity(path.len());
        for byte in path.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
                out.push(byte as char);
            } else {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
        out
    }

    fn file_uri(path: &Path) -> String {
        format!(
            "file://{}",
            percent_encode_trash_path(&path.to_string_lossy())
        )
    }

    fn find_executable(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
        let candidate = Path::new(name);
        if candidate.is_absolute() {
            return is_executable_file(candidate).then(|| candidate.to_path_buf());
        }
        let path_env = path_env?;
        for dir in std::env::split_paths(path_env) {
            let full = dir.join(name);
            if is_executable_file(&full) {
                return Some(full);
            }
        }
        None
    }

    fn is_executable_file(path: &Path) -> bool {
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111) != 0)
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

    #[cfg(unix)]
    fn local_iso8601() -> String {
        // SAFETY: `now` and `tm` are valid pointers for `localtime_r`.
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return "1970-01-01T00:00:00".to_string();
            }
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min,
                tm.tm_sec
            )
        }
    }

    #[cfg(not(unix))]
    fn local_iso8601() -> String {
        "1970-01-01T00:00:00".to_string()
    }
}

/// Linux "Start at Login" management via `$XDG_CONFIG_HOME/autostart/xyz.jyeu.devbase.desktop`.
pub mod login_item {
    use std::path::{Path, PathBuf};

    use crate::autostart::desktop::{DesktopEntry, parse_desktop, quote_exec_arg};
    use crate::fs::atomic_write;
    use crate::launchd::{Host, LaunchdError, ProcessRunner};

    /// The XDG autostart `.desktop` filename for Devbase.
    pub const DESKTOP_FILENAME: &str = "xyz.jyeu.devbase.desktop";

    /// The default XDG autostart directory (`$XDG_CONFIG_HOME/autostart`).
    pub fn default_dir(home: &Path) -> PathBuf {
        super::paths::autostart_directory(home)
    }

    /// The `.desktop` path inside `dir`.
    pub fn entry_path(dir: &Path) -> Option<PathBuf> {
        if dir.as_os_str().is_empty() {
            return None;
        }
        Some(dir.join(DESKTOP_FILENAME))
    }

    /// Whether Devbase's XDG autostart entry exists and is enabled in `dir`.
    pub fn is_enabled(dir: &Path) -> bool {
        let Some(path) = entry_path(dir) else {
            return false;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            return false;
        };
        parse_desktop(&content).is_ok_and(|entry| entry.is_enabled())
    }

    /// Atomically writes `xyz.jyeu.devbase.desktop` in `dir` to launch `exe --background`.
    pub fn enable(dir: &Path, exe: &Path, _runner: &dyn ProcessRunner) -> Result<(), LaunchdError> {
        let path = entry_path(dir)
            .ok_or_else(|| LaunchdError::validation("autostart directory is not configured"))?;
        if exe.as_os_str().is_empty() || !exe.is_absolute() {
            return Err(LaunchdError::validation("executable path must be absolute"));
        }
        let exe_text = exe.to_string_lossy();
        let entry = DesktopEntry {
            name: "Devbase".to_string(),
            exec: format!("{} --background", quote_exec_arg(&exe_text)),
            try_exec: exe_text.into_owned(),
            comment: "Start Devbase quietly in the background at login".to_string(),
            icon: "xyz.jyeu.devbase".to_string(),
            hidden: false,
            gnome_autostart_enabled: Some(true),
            terminal: false,
            no_display: true,
            entry_type: "Application".to_string(),
        };
        std::fs::create_dir_all(dir).map_err(|error| LaunchdError::io(error.to_string()))?;
        atomic_write(&path, entry.render().as_bytes(), 0o644)
            .map_err(|error| LaunchdError::io(error.to_string()))?;
        Ok(())
    }

    /// Removes `xyz.jyeu.devbase.desktop` from `dir` (missing file is success).
    pub fn disable(dir: &Path, _runner: &dyn ProcessRunner) -> Result<(), LaunchdError> {
        let path = entry_path(dir)
            .ok_or_else(|| LaunchdError::validation("autostart directory is not configured"))?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(LaunchdError::io(error.to_string())),
        }
    }

    /// Resolves the executable recorded in the XDG autostart entry.
    pub fn resolve_executable(
        host: &dyn Host,
        _app_path: Option<&Path>,
        override_path: Option<&Path>,
    ) -> Result<PathBuf, LaunchdError> {
        if let Some(override_path) = override_path.filter(|path| !path.as_os_str().is_empty()) {
            if !override_path.is_absolute() {
                return Err(LaunchdError::validation("executable path must be absolute"));
            }
            return Ok(override_path.to_path_buf());
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
}

/// Convenience wrapper for `desktop::trash_path_in_home`.
pub fn trash_path_in_home(path: &Path, home: &Path) -> io::Result<()> {
    desktop::trash_path_in_home(path, home)
}
