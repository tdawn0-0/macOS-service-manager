//! Optional, reversible local CA environment variable integration for zsh,
//! Bash and fish.
//!
//! Node.js, Python (`requests` / OpenSSL) and `curl` do not automatically read
//! macOS Keychain trust roots. When explicitly enabled and Devbase's local CA
//! certificate (`<root>/proxy/ca.pem`) exists, this module writes a dedicated
//! marked block into detected shell startup files and `bash-env.sh`.
//!
//! Disabled (`ca_env_enabled = false`) by default so existing shell startup
//! files and runtime shell integration stay untouched until the developer opts
//! in.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    Shell, ShellConfiguration, ShellError, ShellStartupFile, fish_quote, read_shell_configuration,
    shell_quote, update_shell_block,
};

/// Fixed marker lines wrapping Devbase's Local CA environment block.
pub const CA_ENV_BLOCK_BEGIN: &str = "# >>> Devbase Local CA >>>";
pub const CA_ENV_BLOCK_END: &str = "# <<< Devbase Local CA <<<";

/// Persisted shell-integration preference for local CA environment exports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCaEnvConfig {
    #[serde(default)]
    pub ca_env_enabled: bool,
}

/// POSIX shell (`zsh` / `bash`) export lines for the local CA certificate.
pub fn ca_env_exports_posix(cert_path: &Path) -> String {
    let quoted = shell_quote(&cert_path.to_string_lossy());
    format!(
        "export NODE_EXTRA_CA_CERTS={quoted}\n\
         export SSL_CERT_FILE={quoted}\n\
         export REQUESTS_CA_BUNDLE={quoted}\n\
         export CURL_CA_BUNDLE={quoted}\n"
    )
}

/// Fish shell export lines for the local CA certificate.
pub fn ca_env_exports_fish(cert_path: &Path) -> String {
    let quoted = fish_quote(&cert_path.to_string_lossy());
    format!(
        "set -gx NODE_EXTRA_CA_CERTS {quoted}\n\
         set -gx SSL_CERT_FILE {quoted}\n\
         set -gx REQUESTS_CA_BUNDLE {quoted}\n\
         set -gx CURL_CA_BUNDLE {quoted}\n"
    )
}

/// Marked POSIX shell block (`zsh` / `bash`) guarded on the CA file being readable.
pub fn ca_env_block_posix(cert_path: &Path) -> String {
    let quoted = shell_quote(&cert_path.to_string_lossy());
    let body = ca_env_exports_posix(cert_path)
        .lines()
        .map(|line| format!("  {line}\n"))
        .collect::<String>();
    format!(
        "{CA_ENV_BLOCK_BEGIN}\n\
         if [ -r {quoted} ]; then\n\
         {body}\
         fi\n\
         {CA_ENV_BLOCK_END}\n"
    )
}

/// Marked fish shell block guarded on the CA file being readable.
pub fn ca_env_block_fish(cert_path: &Path) -> String {
    let quoted = fish_quote(&cert_path.to_string_lossy());
    let body = ca_env_exports_fish(cert_path)
        .lines()
        .map(|line| format!("  {line}\n"))
        .collect::<String>();
    format!(
        "{CA_ENV_BLOCK_BEGIN}\n\
         if test -r {quoted}\n\
         {body}\
         end\n\
         {CA_ENV_BLOCK_END}\n"
    )
}

impl Shell {
    /// Public alias path for Devbase's active local CA certificate
    /// (`<root>/proxy/ca.pem`), matching `devproxy::ca::AuthorityStore`.
    pub fn local_ca_cert_path(&self) -> PathBuf {
        self.store.root().join("proxy").join("ca.pem")
    }

    /// Whether Devbase's active local CA certificate exists and resolves to a
    /// regular file (following the `proxy/ca.pem` -> `ca-current/ca.pem` link).
    pub fn local_ca_cert_exists(&self) -> bool {
        std::fs::metadata(self.local_ca_cert_path()).is_ok_and(|meta| meta.is_file())
    }

    /// Path to the shell CA environment configuration JSON file.
    pub fn ca_env_config_path(&self) -> PathBuf {
        self.store.root().join("shell").join("ca-env.json")
    }

    /// Read the persisted local CA environment setting; defaults to disabled.
    pub fn ca_env_config(&self) -> ShellCaEnvConfig {
        let Ok(bytes) = std::fs::read(self.ca_env_config_path()) else {
            return ShellCaEnvConfig::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    /// Preview of the POSIX (`zsh` / `bash`) environment exports.
    pub fn ca_env_preview(&self) -> String {
        ca_env_exports_posix(&self.local_ca_cert_path())
            .trim_end()
            .to_string()
    }

    /// Preview of the fish environment exports.
    pub fn ca_env_preview_fish(&self) -> String {
        ca_env_exports_fish(&self.local_ca_cert_path())
            .trim_end()
            .to_string()
    }

    /// Whether Local CA environment integration is enabled in config and every
    /// detected shell startup file plus `bash-env.sh` carries its marked block.
    pub fn ca_env_shell_enabled(&self) -> bool {
        if !self.ca_env_config().ca_env_enabled {
            return false;
        }
        let Ok(mut files) = self.shell_startup_files(true) else {
            return false;
        };
        if files.is_empty() {
            return false;
        }
        files.push(ShellStartupFile::new(self.bash_env_path()));
        for file in files {
            let Ok(bytes) = std::fs::read(&file.path) else {
                return false;
            };
            if !contains(&bytes, CA_ENV_BLOCK_BEGIN.as_bytes())
                || !contains(&bytes, CA_ENV_BLOCK_END.as_bytes())
            {
                return false;
            }
        }
        true
    }

    /// Enable the Local CA environment block in detected zsh, Bash and fish
    /// startup files plus `bash-env.sh`.
    pub fn enable_ca_env_shell(&self) -> Result<String, ShellError> {
        self.configure_ca_env_shell(true)
    }

    /// Remove only the Local CA environment block from all supported shells
    /// and persist `ca_env_enabled = false`.
    pub fn disable_ca_env_shell(&self) -> Result<String, ShellError> {
        self.configure_ca_env_shell(false)
    }

    /// Add or remove the reversible Local CA environment block across detected
    /// shells and `bash-env.sh`.
    pub fn configure_ca_env_shell(&self, enable: bool) -> Result<String, ShellError> {
        let cert_path = self.local_ca_cert_path();
        if enable && !self.local_ca_cert_exists() {
            return Err(ShellError::Missing(format!(
                "Devbase local CA certificate was not found at {}. Create or trust the local CA in HTTPS Proxy first.",
                cert_path.display()
            )));
        }

        let mut files = self.shell_startup_files(enable)?;
        if enable && files.is_empty() {
            return Err(ShellError::invalid("no supported shell found"));
        }

        let locations_path = self
            .store
            .root()
            .join("shell")
            .join("ca-env-locations.json");
        let locations = read_shell_configuration(&locations_path)?;
        let remembered: Vec<String> = if locations.data.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice(&locations.data)?
        };
        if !enable {
            for path in &remembered {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(ShellError::invalid(
                        "invalid recorded shell configuration path",
                    ));
                }
                let fish = path
                    .extension()
                    .is_some_and(|extension| extension == "fish");
                let file = if fish {
                    ShellStartupFile::new(path).fish()
                } else {
                    ShellStartupFile::new(path)
                };
                files.push(file);
            }
        }
        files.push(ShellStartupFile::new(self.bash_env_path()));
        let paths: Vec<String> = files
            .iter()
            .map(|file| file.path.to_string_lossy().into_owned())
            .collect();
        let label = paths.join(", ");

        let posix_block = ca_env_block_posix(&cert_path);
        let fish_block = ca_env_block_fish(&cert_path);

        struct Change {
            config: ShellConfiguration,
            data: Vec<u8>,
        }
        let mut changes: Vec<Change> = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();
        for file in &files {
            let config = read_shell_configuration(&file.path)?;
            if seen.contains(&config.target) {
                continue;
            }
            seen.push(config.target.clone());
            let mut content = config.data.clone();
            let new_block = if file.fish {
                &fish_block
            } else if file.path == self.bash_env_path() {
                if content.is_empty() && enable {
                    content = self.bash_env_header().into_bytes();
                }
                &posix_block
            } else {
                &posix_block
            };
            let data = update_shell_block(
                &content,
                CA_ENV_BLOCK_BEGIN,
                CA_ENV_BLOCK_END,
                new_block,
                enable,
            )
            .map_err(|error| {
                ShellError::invalid(format!(
                    "{}: {error}; review this file",
                    file.path.display()
                ))
            })?;
            if config.data != data {
                changes.push(Change { config, data });
            }
        }

        if enable {
            let mut unique: Vec<String> = Vec::new();
            let mut saved: Vec<String> = Vec::new();
            for path in remembered.iter().chain(paths.iter()) {
                if !unique.contains(path) {
                    unique.push(path.clone());
                    saved.push(path.clone());
                }
            }
            let data = serde_json::to_vec(&saved)?;
            if locations.data != data {
                changes.push(Change {
                    config: locations,
                    data,
                });
            }
        }

        for index in 0..changes.len() {
            let error = match changes[index].config.commit(&changes[index].data) {
                Ok(()) => continue,
                Err(error) => error,
            };
            let mut joined = error.to_string();
            for earlier in (0..index).rev() {
                let snapshot = &changes[earlier];
                let rollback = read_shell_configuration(&snapshot.config.rc).and_then(|current| {
                    if current.target != snapshot.config.target || current.data != snapshot.data {
                        return Err(ShellError::invalid("configuration changed during rollback"));
                    }
                    current.commit(&snapshot.config.data)
                });
                if let Err(error) = rollback {
                    joined.push_str(&format!(
                        "; restore {}: {error}",
                        snapshot.config.rc.display()
                    ));
                }
            }
            return Err(ShellError::invalid(joined));
        }

        let ca_config_path = self.ca_env_config_path();
        if enable || ca_config_path.exists() {
            let mut config_bytes = serde_json::to_vec_pretty(&ShellCaEnvConfig {
                ca_env_enabled: enable,
            })?;
            config_bytes.push(b'\n');
            crate::fs::atomic_write(&ca_config_path, &config_bytes, 0o600)?;
        }
        Ok(label)
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}
