//! uv discovery and managed Python actions, ported from `uv_python.go`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::{Shell, ShellError};
use crate::platform::HostTarget;

static UV_PYTHON_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(cpython|pypy|graalpy)-[0-9][A-Za-z0-9.+_-]*$").unwrap());

/// One row from `uv python list --output-format json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UVPython {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub implementation: String,
    #[serde(default)]
    pub arch: String,
    #[serde(rename = "os", default)]
    pub os_name: String,
}

/// A filesystem candidate list, matching `exec.LookPath("uv")` followed by the
/// fixed fallbacks (`~/.local/bin/uv`, `~/.cargo/bin/uv`, and platform paths).
fn uv_candidates(shell: &Shell) -> Vec<PathBuf> {
    if let Some(override_paths) = shell.uv_candidate_override() {
        return override_paths.to_vec();
    }
    let mut candidates = Vec::new();
    if let Some(path) = shell
        .path_dirs()
        .iter()
        .map(|dir| dir.join("uv"))
        .find(|path| path.exists())
    {
        candidates.push(path);
    }
    candidates.push(shell.home().join(".local").join("bin").join("uv"));
    candidates.push(shell.home().join(".cargo").join("bin").join("uv"));
    if HostTarget::current().is_macos() {
        candidates.push(PathBuf::from("/opt/homebrew/bin/uv"));
    } else {
        candidates.push(PathBuf::from("/usr/bin/uv"));
        candidates.push(PathBuf::from("/usr/local/bin/uv"));
        candidates.push(PathBuf::from("/home/linuxbrew/.linuxbrew/bin/uv"));
    }
    candidates
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

impl Shell {
    /// The selected or discovered `uv` executable.
    pub fn uv_path(&self) -> Result<PathBuf, ShellError> {
        let state = self.store().read()?;
        if !state.uv_executable.is_empty() {
            let path = PathBuf::from(&state.uv_executable);
            if path.is_absolute() && is_executable(&path) {
                return Ok(path);
            }
            return Err(ShellError::invalid(
                "selected uv executable is unavailable; select an installed uv executable",
            ));
        }
        for path in uv_candidates(self) {
            if path.is_absolute() && is_executable(&path) {
                return Ok(path);
            }
        }
        Err(ShellError::invalid(
            "uv was not found; select an installed uv executable",
        ))
    }

    /// Validate and remember an absolute uv executable path.
    pub fn set_uv_path(&self, path: &Path) -> Result<(), ShellError> {
        if !path.is_absolute() {
            return Err(ShellError::invalid("choose an absolute uv executable path"));
        }
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(ShellError::invalid("uv path is not executable"));
        }
        let output = self.runner().run(
            &path.to_string_lossy(),
            &["--version"],
            crate::software::COMMAND_TIMEOUT,
        )?;
        if !output.starts_with("uv ") {
            return Err(ShellError::invalid("executable is not uv"));
        }
        let path = path.to_path_buf();
        self.store().with_state(|state| {
            state.uv_executable = path.to_string_lossy().into_owned();
            Ok(())
        })?;
        Ok(())
    }

    fn uv_run(&self, args: &[&str]) -> Result<String, ShellError> {
        let path = self.uv_path()?;
        let mut full = vec!["--no-progress", "--directory", "/tmp"];
        full.extend_from_slice(args);
        self.runner()
            .run(
                &path.to_string_lossy(),
                &full,
                crate::software::COMMAND_TIMEOUT,
            )
            .map_err(ShellError::Command)
    }

    /// List managed Python interpreters (installed or available).
    pub fn uv_pythons(&self, installed: bool) -> Result<Vec<UVPython>, ShellError> {
        let mut args = vec![
            "python",
            "list",
            "--managed-python",
            "--output-format",
            "json",
        ];
        if installed {
            args.push("--only-installed");
        } else {
            args.push("--only-downloads");
            args.push("--all-versions");
        }
        let output = self.uv_run(&args)?;
        let rows: Vec<UVPython> = serde_json::from_str(&output).map_err(|_| {
            ShellError::invalid("uv must support python list --output-format json; update uv")
        })?;
        for row in &rows {
            if !UV_PYTHON_KEY.is_match(&row.key)
                || row.version.is_empty()
                || (installed && !Path::new(&row.path).is_absolute())
            {
                return Err(ShellError::invalid("invalid uv version entry"));
            }
        }
        Ok(rows)
    }

    /// Install an available managed Python.
    pub fn install_uv_python(&self, key: &str) -> Result<(), ShellError> {
        let target = HostTarget::current();
        let rows = self.uv_pythons(false)?;
        let found = rows
            .iter()
            .any(|row| row.key == key && target.matches_uv(&row.os_name, &row.arch));
        if !found {
            return Err(ShellError::invalid(if target.is_macos() {
                "select an available macOS arm64 Python version".to_string()
            } else {
                format!("select an available {} Python version", target.cli_arch())
            }));
        }
        self.uv_run(&["python", "install", "--no-bin", key])?;
        Ok(())
    }

    /// Uninstall or globally pin an installed managed Python.
    pub fn uv_python_action(&self, key: &str, action: &str) -> Result<(), ShellError> {
        let rows = self.uv_pythons(true)?;
        if !rows
            .iter()
            .any(|row| row.key == key && !row.path.is_empty())
        {
            return Err(ShellError::invalid(
                "selected uv-managed Python is no longer installed",
            ));
        }
        match action {
            "uninstall" => {
                self.uv_run(&["python", "uninstall", key])?;
            }
            "default" => {
                self.uv_run(&["python", "pin", "--global", "--no-project", key])?;
            }
            _ => return Err(ShellError::invalid("unsupported Python action")),
        }
        Ok(())
    }

    /// `<uv --version> · <path>`.
    pub fn uv_description(&self) -> Result<String, ShellError> {
        let path = self.uv_path()?;
        let version = self.uv_run(&["--version"])?;
        if !version.starts_with("uv ") {
            return Err(ShellError::invalid("selected executable is not uv"));
        }
        Ok(format!("{} · {}", version.trim(), path.display()))
    }

    /// uv's global Python pin, without resolving or installing anything.
    pub fn uv_global_pin(&self) -> Result<String, ShellError> {
        match self.uv_run(&["python", "pin", "--global", "--no-project"]) {
            Ok(output) => Ok(output.trim().to_string()),
            Err(error) if error.to_string().contains("No Python version file found") => {
                Ok(String::new())
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod uv_candidate_tests {
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::*;
    use crate::engine::{CommandRunner, SystemCommandRunner};
    use crate::software::{CurlTransport, Software};
    use crate::state::Store;

    #[test]
    fn uv_candidates_include_local_and_cargo_bins() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path().join("root"));
        let software = Software::new(store.clone());
        let runner: Arc<dyn CommandRunner> = Arc::new(SystemCommandRunner::new());
        let http = Arc::new(CurlTransport::new());
        let shell = Shell::with_seams(store, software, runner, http).with_home(dir.path());
        let candidates = uv_candidates(&shell);
        assert!(candidates.contains(&dir.path().join(".local/bin/uv")));
        assert!(candidates.contains(&dir.path().join(".cargo/bin/uv")));
    }
}
