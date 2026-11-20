//! The managed `devbase` wrapper script, ported from `install_path.go`.
//!
//! The marker, the script text, the 0755 atomic rename, the foreign-file
//! refusal, and the PATH comparison are all contract.

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use devcore::launchd::SystemHost;
use devcore::launchd::resolve_login_executable;
use serde::Serialize;

use crate::error::{CliError, CliResult};

pub const CLI_MANAGED_MARKER: &str = "# Managed by Devbase CLI";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CLIInstallStatus {
    pub installed: bool,
    #[serde(rename = "wrapperPath")]
    pub wrapper_path: String,
    #[serde(rename = "targetExecutable", skip_serializing_if = "String::is_empty")]
    pub target_executable: String,
    #[serde(rename = "inPath")]
    pub in_path: bool,
    #[serde(rename = "managedByDevbase")]
    pub managed_by_devbase: bool,
}

/// `DefaultCLIBinDir`: `$MSM_DEV_HOME/bin`, else `~/.local/bin`.
pub fn default_cli_bin_dir() -> PathBuf {
    if let Ok(home) = std::env::var("MSM_DEV_HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home).join("bin");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".local").join("bin")
}

pub fn cli_shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn app_bundle_for_executable(exe: &Path) -> String {
    let Some(dir) = exe.parent() else {
        return String::new();
    };
    if dir.file_name().and_then(|name| name.to_str()) != Some("MacOS") {
        return String::new();
    }
    let Some(contents) = dir.parent() else {
        return String::new();
    };
    if contents.file_name().and_then(|name| name.to_str()) != Some("Contents")
        || fs::metadata(contents.join("Info.plist")).is_err()
    {
        return String::new();
    }
    contents
        .parent()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_dir_in_path(bin_dir: &Path) -> bool {
    if bin_dir.as_os_str().is_empty() {
        return false;
    }
    let clean_target = clean_path(bin_dir);
    let resolved_target = fs::canonicalize(&clean_target).ok();
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for entry in std::env::split_paths(&path) {
        if entry.as_os_str().is_empty() {
            continue;
        }
        let clean_entry = clean_path(&entry);
        if clean_entry == clean_target {
            return true;
        }
        if let Some(resolved_target) = &resolved_target
            && fs::canonicalize(&clean_entry).ok().as_ref() == Some(resolved_target)
        {
            return true;
        }
    }
    false
}

/// Go's `filepath.Clean` for the shapes this file compares.
pub fn clean_path(path: &Path) -> PathBuf {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    let absolute = path.is_absolute();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::RootDir | Component::Prefix(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|part| part != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..".into());
                }
            }
            Component::Normal(part) => parts.push(part.to_os_string()),
        }
    }
    let mut out = if absolute {
        PathBuf::from("/")
    } else if parts.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::new()
    };
    for part in parts {
        out.push(part);
    }
    out
}

fn read_bounded_regular_file(path: &Path, limit: u64) -> Result<(Vec<u8>, fs::Metadata), String> {
    let info = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !info.file_type().is_file() {
        return Err("not a regular file".into());
    }
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    if data.len() as u64 > limit {
        return Err("file exceeds limit".into());
    }
    Ok((data, info))
}

/// `ResolveLoginExecutable(appPath, override)` with the production host.
pub fn resolve_target_executable(app_path: &str, override_path: &str) -> CliResult<PathBuf> {
    let host = SystemHost::new();
    let app = if app_path.is_empty() {
        None
    } else {
        Some(Path::new(app_path))
    };
    let override_value = if override_path.is_empty() {
        None
    } else {
        Some(Path::new(override_path))
    };
    resolve_login_executable(&host, app, override_value)
        .map_err(|error| CliError::operation(error.to_string()))
}

/// `CLIStatus`.
pub fn cli_status(bin_dir: &str, app_path: &str, exe_override: &str) -> CLIInstallStatus {
    let bin_dir = if bin_dir.is_empty() {
        default_cli_bin_dir()
    } else {
        PathBuf::from(bin_dir)
    };
    let wrapper_path = bin_dir.join("devbase");
    let target_exe = resolve_target_executable(app_path, exe_override)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut status = CLIInstallStatus {
        installed: false,
        wrapper_path: wrapper_path.to_string_lossy().into_owned(),
        target_executable: target_exe,
        in_path: is_dir_in_path(&bin_dir),
        managed_by_devbase: false,
    };
    if let Ok((data, _)) = read_bounded_regular_file(&wrapper_path, 64 << 10) {
        status.managed_by_devbase = String::from_utf8_lossy(&data).contains(CLI_MANAGED_MARKER);
    }
    if let Ok(info) = fs::metadata(&wrapper_path)
        && info.file_type().is_file()
        && info.permissions().mode() & 0o111 != 0
    {
        status.installed = true;
    }
    status
}

/// The exact wrapper script (`install_path.go:150-181`).
pub fn wrapper_script(target_exe: &str, bundle_path: &str) -> String {
    format!(
        r#"#!/bin/sh
{marker}
set -eu
TARGET={target}
BUNDLE={bundle}
if [ "$#" -eq 0 ]; then
  set -- help
fi
if [ "$1" = "gui" ] || [ "$1" = "open" ]; then
  shift
  if [ -n "$BUNDLE" ] && [ -d "$BUNDLE" ]; then
    if [ "$#" -gt 0 ]; then
      exec /usr/bin/open "$BUNDLE" --args "$@"
    fi
    exec /usr/bin/open "$BUNDLE"
  fi
  if [ -z "${{MSM_DEV_HOME:-}}" ] && [ -d "/Applications/Devbase.app" ]; then
    if [ "$#" -gt 0 ]; then
      exec /usr/bin/open "/Applications/Devbase.app" --args "$@"
    fi
    exec /usr/bin/open "/Applications/Devbase.app"
  fi
fi
if [ -x "$TARGET" ]; then
  exec "$TARGET" "$@"
fi
if [ -z "${{MSM_DEV_HOME:-}}" ] && [ -x "/Applications/Devbase.app/Contents/MacOS/Devbase" ]; then
  exec "/Applications/Devbase.app/Contents/MacOS/Devbase" "$@"
fi
echo "Devbase executable not found at $TARGET" >&2
exit 1
"#,
        marker = CLI_MANAGED_MARKER,
        target = cli_shell_quote(target_exe),
        bundle = cli_shell_quote(bundle_path),
    )
}

/// `InstallCLIWrapper`.
pub fn install_cli_wrapper(
    bin_dir: &str,
    app_path: &str,
    exe_override: &str,
) -> CliResult<CLIInstallStatus> {
    let bin_dir = if bin_dir.is_empty() {
        default_cli_bin_dir()
    } else {
        PathBuf::from(bin_dir)
    };
    if !bin_dir.is_absolute() {
        return Err(CliError::operation(
            "CLI directory must be an absolute path",
        ));
    }
    let target_exe = resolve_target_executable(app_path, exe_override)?;
    let target_exe = target_exe.to_string_lossy().into_owned();
    let wrapper_path = bin_dir.join("devbase");
    match read_bounded_regular_file(&wrapper_path, 64 << 10) {
        Ok((data, _)) => {
            if !String::from_utf8_lossy(&data).contains(CLI_MANAGED_MARKER) {
                return Err(CliError::operation(format!(
                    "{} already exists and is not managed by Devbase",
                    wrapper_path.display()
                )));
            }
        }
        Err(_) if fs::symlink_metadata(&wrapper_path).is_err() => {}
        Err(error) => {
            if let Ok(info) = fs::symlink_metadata(&wrapper_path)
                && !info.file_type().is_file()
            {
                return Err(CliError::operation(format!(
                    "{} already exists and is not a regular file",
                    wrapper_path.display()
                )));
            }
            return Err(CliError::operation(format!(
                "cannot inspect existing {}: {error}",
                wrapper_path.display()
            )));
        }
    }

    let bundle_path = if app_path.is_empty() {
        app_bundle_for_executable(Path::new(&target_exe))
    } else {
        app_path.to_string()
    };
    let script = wrapper_script(&target_exe, &bundle_path);

    fs::create_dir_all(&bin_dir).map_err(|error| CliError::operation(error.to_string()))?;
    let tmp_path = bin_dir.join(format!(
        ".devbase-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let write = || -> std::io::Result<()> {
        fs::write(&tmp_path, script.as_bytes())?;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755))?;
        fs::rename(&tmp_path, &wrapper_path)
    };
    if let Err(error) = write() {
        let _ = fs::remove_file(&tmp_path);
        return Err(CliError::operation(error.to_string()));
    }
    Ok(cli_status(
        &bin_dir.to_string_lossy(),
        app_path,
        exe_override,
    ))
}

/// `UninstallCLIWrapper`.
pub fn uninstall_cli_wrapper(bin_dir: &str) -> CliResult<()> {
    let bin_dir = if bin_dir.is_empty() {
        default_cli_bin_dir()
    } else {
        PathBuf::from(bin_dir)
    };
    let wrapper_path = bin_dir.join("devbase");
    let data = match read_bounded_regular_file(&wrapper_path, 64 << 10) {
        Ok((data, _)) => data,
        Err(_) if fs::symlink_metadata(&wrapper_path).is_err() => return Ok(()),
        Err(error) => {
            if let Ok(info) = fs::symlink_metadata(&wrapper_path)
                && !info.file_type().is_file()
            {
                return Err(CliError::operation(format!(
                    "{} is not managed by Devbase",
                    wrapper_path.display()
                )));
            }
            return Err(CliError::operation(format!(
                "cannot inspect {}: {error}",
                wrapper_path.display()
            )));
        }
    };
    if !String::from_utf8_lossy(&data).contains(CLI_MANAGED_MARKER) {
        return Err(CliError::operation(format!(
            "{} is not managed by Devbase",
            wrapper_path.display()
        )));
    }
    fs::remove_file(&wrapper_path).map_err(|error| CliError::operation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_matches_the_go_text() {
        let script = wrapper_script(
            "/Applications/Devbase.app/Contents/MacOS/Devbase",
            "/Applications/Devbase.app",
        );
        assert!(script.starts_with("#!/bin/sh\n# Managed by Devbase CLI\nset -eu\n"));
        assert!(script.contains("TARGET='/Applications/Devbase.app/Contents/MacOS/Devbase'"));
        assert!(script.contains("BUNDLE='/Applications/Devbase.app'"));
        assert!(script.contains(r#"echo "Devbase executable not found at $TARGET" >&2"#));
        assert!(script.ends_with("exit 1\n"));
    }

    #[test]
    fn quoting_escapes_single_quotes() {
        assert_eq!(cli_shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn install_refuses_foreign_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("devbase"), "#!/bin/sh\n").unwrap();
        let err = install_cli_wrapper(&bin.to_string_lossy(), "", "").unwrap_err();
        assert!(err.message().contains("not managed by Devbase"));
    }
}
