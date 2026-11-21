//! Dual-platform (macOS + Linux) host target, path, desktop, and login-item abstractions.
//!
//! Both [`macos`] and [`linux`] submodules compile on every platform so unit
//! tests can verify cross-platform path, terminal, Trash, and autostart logic
//! regardless of host OS. The active target's modules are re-exported at
//! [`paths`], [`desktop`], and [`login_item`].

pub mod linux;
pub mod macos;

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::service::Source;

#[cfg(target_os = "macos")]
pub use macos::{desktop, login_item, paths};

#[cfg(not(target_os = "macos"))]
pub use linux::{desktop, login_item, paths};

/// Supported operating systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostOs {
    MacOs,
    Linux,
}

/// Supported CPU architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostArch {
    Arm64,
    Amd64,
}

/// Describes a target OS + CPU architecture pair and maps it to ecosystem-specific
/// download/catalog identifiers (Node.js, JetBrains/JDK, uv, Devbase model arch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HostTarget {
    pub os: HostOs,
    pub arch: HostArch,
}

impl HostTarget {
    /// Apple Silicon macOS target.
    pub const MACOS_ARM64: HostTarget = HostTarget {
        os: HostOs::MacOs,
        arch: HostArch::Arm64,
    };

    /// Linux x86_64 target.
    pub const LINUX_AMD64: HostTarget = HostTarget {
        os: HostOs::Linux,
        arch: HostArch::Amd64,
    };

    /// Linux aarch64 target.
    pub const LINUX_ARM64: HostTarget = HostTarget {
        os: HostOs::Linux,
        arch: HostArch::Arm64,
    };

    /// Constructs an explicit target (useful in tests and cross-target catalog logic).
    pub const fn new(os: HostOs, arch: HostArch) -> Self {
        HostTarget { os, arch }
    }

    /// Detects the compiled host target.
    pub const fn current() -> Self {
        let os = if cfg!(target_os = "macos") {
            HostOs::MacOs
        } else {
            HostOs::Linux
        };
        let arch = if cfg!(target_arch = "aarch64") {
            HostArch::Arm64
        } else {
            HostArch::Amd64
        };
        HostTarget { os, arch }
    }

    pub const fn is_macos(self) -> bool {
        matches!(self.os, HostOs::MacOs)
    }

    pub const fn is_linux(self) -> bool {
        matches!(self.os, HostOs::Linux)
    }

    /// Short OS token (`"darwin"` or `"linux"`).
    pub const fn os_name(self) -> &'static str {
        match self.os {
            HostOs::MacOs => "darwin",
            HostOs::Linux => "linux",
        }
    }

    /// Canonical model architecture token (`"arm64"` or `"amd64"`).
    pub const fn arch_name(self) -> &'static str {
        match self.arch {
            HostArch::Arm64 => "arm64",
            HostArch::Amd64 => "amd64",
        }
    }

    /// CLI schema architecture string (`"darwin/arm64"`, `"linux/amd64"`, `"linux/arm64"`).
    pub const fn cli_arch(self) -> &'static str {
        match (self.os, self.arch) {
            (HostOs::MacOs, HostArch::Arm64) => "darwin/arm64",
            (HostOs::MacOs, HostArch::Amd64) => "darwin/amd64",
            (HostOs::Linux, HostArch::Amd64) => "linux/amd64",
            (HostOs::Linux, HostArch::Arm64) => "linux/arm64",
        }
    }

    /// Node.js `index.json` `files` tag (`"osx-arm64-tar"`, `"linux-x64"`, `"linux-arm64"`).
    pub const fn node_dist_tag(self) -> &'static str {
        match (self.os, self.arch) {
            (HostOs::MacOs, HostArch::Arm64) => "osx-arm64-tar",
            (HostOs::MacOs, HostArch::Amd64) => "osx-x64-tar",
            (HostOs::Linux, HostArch::Amd64) => "linux-x64",
            (HostOs::Linux, HostArch::Arm64) => "linux-arm64",
        }
    }

    /// Node.js download filename platform suffix (`"darwin-arm64"`, `"linux-x64"`, `"linux-arm64"`).
    pub const fn node_archive_platform(self) -> &'static str {
        match (self.os, self.arch) {
            (HostOs::MacOs, HostArch::Arm64) => "darwin-arm64",
            (HostOs::MacOs, HostArch::Amd64) => "darwin-x64",
            (HostOs::Linux, HostArch::Amd64) => "linux-x64",
            (HostOs::Linux, HostArch::Arm64) => "linux-arm64",
        }
    }

    /// JetBrains JDK catalog OS identifier (`"macOS"` or `"linux"`).
    pub const fn jetbrains_os(self) -> &'static str {
        match self.os {
            HostOs::MacOs => "macOS",
            HostOs::Linux => "linux",
        }
    }

    /// JetBrains JDK catalog architecture identifier (`"aarch64"` or `"x86_64"`).
    pub const fn jetbrains_arch(self) -> &'static str {
        match self.arch {
            HostArch::Arm64 => "aarch64",
            HostArch::Amd64 => "x86_64",
        }
    }

    /// `uv python list --output-format json` OS identifier (`"macos"` or `"linux"`).
    pub const fn uv_os(self) -> &'static str {
        match self.os {
            HostOs::MacOs => "macos",
            HostOs::Linux => "linux",
        }
    }

    /// `uv python list --output-format json` architecture identifier (`"aarch64"` or `"x86_64"`).
    pub const fn uv_arch(self) -> &'static str {
        match self.arch {
            HostArch::Arm64 => "aarch64",
            HostArch::Amd64 => "x86_64",
        }
    }

    /// Whether a `uv` Python release entry matches this host target.
    pub fn matches_uv(self, os_name: &str, arch: &str) -> bool {
        os_name.eq_ignore_ascii_case(self.uv_os()) && arch.eq_ignore_ascii_case(self.uv_arch())
    }

    /// Whether a `devcore::model::Package.arch` value is accepted on this host target.
    pub fn accepts_model_arch(self, arch: &str) -> bool {
        match (self.os, self.arch) {
            (HostOs::MacOs, HostArch::Arm64) => matches!(arch, "arm64" | "universal"),
            (HostOs::MacOs, HostArch::Amd64) => matches!(arch, "amd64" | "x86_64" | "universal"),
            (HostOs::Linux, HostArch::Amd64) => matches!(arch, "amd64" | "x86_64"),
            (HostOs::Linux, HostArch::Arm64) => matches!(arch, "arm64" | "aarch64"),
        }
    }

    /// The default user-scoped service source for this target.
    pub const fn default_user_source(self) -> Source {
        match self.os {
            HostOs::MacOs => Source::UserAgent,
            HostOs::Linux => Source::SystemdUser,
        }
    }
}

/// Resolves `curl` from `PATH`, falling back to `/usr/bin/curl`.
pub fn resolve_curl() -> PathBuf {
    resolve_program_in_path("curl", std::env::var_os("PATH").as_deref())
        .unwrap_or_else(|| PathBuf::from("/usr/bin/curl"))
}

/// Resolves an executable name against an explicit `PATH` value.
pub fn resolve_program_in_path(name: &str, path_env: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let candidate = Path::new(name);
    if candidate.is_absolute() {
        return std::fs::metadata(candidate)
            .is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111) != 0)
            .then(|| candidate.to_path_buf());
    }
    let path_env = path_env?;
    for dir in std::env::split_paths(path_env) {
        let full = dir.join(name);
        if std::fs::metadata(&full)
            .is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111) != 0)
        {
            return Some(full);
        }
    }
    None
}

/// Wraps a reader in an XZ decompression stream (used for Linux `.tar.xz` archives).
pub fn xz_decoder<R: Read>(reader: R) -> xz2::read::XzDecoder<R> {
    xz2::read::XzDecoder::new(reader)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;
    use crate::launchd::{CommandError, Host, LaunchdError, ProcessRunner};

    #[test]
    fn host_target_mappings_cover_macos_and_linux() {
        let mac = HostTarget::MACOS_ARM64;
        assert!(mac.is_macos());
        assert!(!mac.is_linux());
        assert_eq!(mac.cli_arch(), "darwin/arm64");
        assert_eq!(mac.node_dist_tag(), "osx-arm64-tar");
        assert_eq!(mac.jetbrains_os(), "macOS");
        assert_eq!(mac.jetbrains_arch(), "aarch64");
        assert_eq!(mac.uv_os(), "macos");
        assert_eq!(mac.uv_arch(), "aarch64");
        assert!(mac.accepts_model_arch("arm64"));
        assert!(mac.accepts_model_arch("universal"));
        assert!(!mac.accepts_model_arch("amd64"));
        assert!(mac.matches_uv("macos", "aarch64"));
        assert!(!mac.matches_uv("linux", "x86_64"));

        let lx64 = HostTarget::LINUX_AMD64;
        assert!(lx64.is_linux());
        assert!(!lx64.is_macos());
        assert_eq!(lx64.cli_arch(), "linux/amd64");
        assert_eq!(lx64.node_dist_tag(), "linux-x64");
        assert_eq!(lx64.jetbrains_os(), "linux");
        assert_eq!(lx64.jetbrains_arch(), "x86_64");
        assert_eq!(lx64.uv_os(), "linux");
        assert_eq!(lx64.uv_arch(), "x86_64");
        assert!(lx64.accepts_model_arch("amd64"));
        assert!(lx64.accepts_model_arch("x86_64"));
        assert!(!lx64.accepts_model_arch("arm64"));
        assert!(lx64.matches_uv("linux", "x86_64"));

        let larm = HostTarget::LINUX_ARM64;
        assert_eq!(larm.cli_arch(), "linux/arm64");
        assert_eq!(larm.node_dist_tag(), "linux-arm64");
        assert_eq!(larm.jetbrains_arch(), "aarch64");
        assert_eq!(larm.uv_arch(), "aarch64");
        assert!(larm.accepts_model_arch("arm64"));
        assert!(larm.accepts_model_arch("aarch64"));
        assert!(!larm.accepts_model_arch("amd64"));
    }

    #[test]
    fn macos_and_linux_paths_follow_platform_conventions() {
        let home = Path::new("/home/alice");
        assert_eq!(
            macos::paths::default_storage_root(home),
            PathBuf::from("/home/alice/Library/Application Support/Devbase/Development")
        );
        assert_eq!(
            macos::paths::user_cache_directory(home),
            PathBuf::from("/home/alice/Library/Caches/Devbase/Development")
        );
        assert_eq!(
            macos::paths::user_java_directory(home),
            PathBuf::from("/home/alice/Library/Java/JavaVirtualMachines")
        );
        assert_eq!(macos::paths::DEFAULT_BREW_CELLAR, "/opt/homebrew/Cellar");

        assert_eq!(
            linux::paths::default_storage_root_with_xdg(home, None),
            PathBuf::from("/home/alice/.local/share/devbase")
        );
        assert_eq!(
            linux::paths::default_storage_root_with_xdg(home, Some(Path::new("/srv/data"))),
            PathBuf::from("/srv/data/devbase")
        );
        assert_eq!(
            linux::paths::user_cache_directory_with_xdg(home, None),
            PathBuf::from("/home/alice/.cache/devbase")
        );
        assert_eq!(
            linux::paths::user_cache_directory_with_xdg(home, Some(Path::new("/srv/cache"))),
            PathBuf::from("/srv/cache/devbase")
        );
        assert_eq!(
            linux::paths::user_java_directory_with_xdg(home, None),
            PathBuf::from("/home/alice/.local/share/devbase/jdks")
        );
        assert_eq!(
            linux::paths::DEFAULT_BREW_CELLAR,
            "/home/linuxbrew/.linuxbrew/Cellar"
        );
    }

    #[test]
    fn linux_terminal_resolution_respects_env_and_fallback_order() {
        let temp = TempDir::new().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in ["konsole", "gnome-terminal", "kitty"] {
            let exe = bin.join(name);
            std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let script = Path::new("/tmp/open-cli.sh");

        // $TERMINAL wins when present and executable.
        let (prog, args) =
            linux::desktop::resolve_terminal_command(Some("kitty"), Some(bin.as_os_str()), script)
                .unwrap();
        assert_eq!(prog, bin.join("kitty"));
        assert_eq!(args, vec![OsStr::new("/tmp/open-cli.sh")]);

        // Without $TERMINAL, konsole beats gnome-terminal in fallback order.
        let (prog, args) =
            linux::desktop::resolve_terminal_command(None, Some(bin.as_os_str()), script).unwrap();
        assert_eq!(prog, bin.join("konsole"));
        assert_eq!(args, vec![OsStr::new("-e"), OsStr::new("/tmp/open-cli.sh")]);

        // gnome-terminal uses `--`.
        let args = linux::desktop::terminal_exec_args(Path::new("/usr/bin/gnome-terminal"), script);
        assert_eq!(args, vec![OsStr::new("--"), OsStr::new("/tmp/open-cli.sh")]);
    }

    #[test]
    fn linux_xdg_trash_writes_trashinfo_and_handles_collisions() {
        let temp = TempDir::new().unwrap();
        let trash_dir = temp.path().join("Trash");
        let first = temp.path().join("sample.service");
        std::fs::write(&first, b"v1").unwrap();
        linux::desktop::trash_path_into(&first, &trash_dir).unwrap();
        assert!(!first.exists());
        assert_eq!(
            std::fs::read(trash_dir.join("files/sample.service")).unwrap(),
            b"v1"
        );
        let info =
            std::fs::read_to_string(trash_dir.join("info/sample.service.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="));
        assert!(info.contains("DeletionDate="));

        // Second file with same name gets a `.1` suffix without overwriting.
        std::fs::write(&first, b"v2").unwrap();
        linux::desktop::trash_path_into(&first, &trash_dir).unwrap();
        assert_eq!(
            std::fs::read(trash_dir.join("files/sample.1.service")).unwrap(),
            b"v2"
        );
        assert!(trash_dir.join("info/sample.1.service.trashinfo").exists());
    }

    #[test]
    fn linux_login_item_enables_queries_and_disables_atomically() {
        struct NoopRunner;
        impl ProcessRunner for NoopRunner {
            fn run(&self, _: &str, _: &[&str]) -> Result<String, CommandError> {
                Ok(String::new())
            }
        }
        struct StubHost(PathBuf);
        impl Host for StubHost {
            fn uid(&self) -> u32 {
                1000
            }
            fn home_dir(&self) -> Result<PathBuf, LaunchdError> {
                Ok(self.0.clone())
            }
            fn temp_dir(&self) -> PathBuf {
                self.0.clone()
            }
            fn current_exe(&self) -> Result<PathBuf, LaunchdError> {
                Ok(PathBuf::from("/usr/bin/devbase"))
            }
            fn eval_symlinks(&self, path: &Path) -> Option<PathBuf> {
                Some(path.to_path_buf())
            }
            fn trash(&self, _: &Path) -> Result<(), LaunchdError> {
                Ok(())
            }
        }

        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("autostart");
        assert!(!linux::login_item::is_enabled(&dir));

        let exe =
            linux::login_item::resolve_executable(&StubHost(temp.path().to_path_buf()), None, None)
                .unwrap();
        assert_eq!(exe, PathBuf::from("/usr/bin/devbase"));

        linux::login_item::enable(&dir, &exe, &NoopRunner).unwrap();
        assert!(linux::login_item::is_enabled(&dir));
        let path = linux::login_item::entry_path(&dir).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("Exec=/usr/bin/devbase --background"));
        assert!(body.contains("Hidden=false"));

        linux::login_item::disable(&dir, &NoopRunner).unwrap();
        assert!(!linux::login_item::is_enabled(&dir));
        assert!(!path.exists());
    }

    #[test]
    fn xz_decoder_round_trips_compressed_bytes() {
        let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 1);
        encoder.write_all(b"hello linux mysql tar.xz").unwrap();
        let compressed = encoder.finish().unwrap();

        let mut out = String::new();
        xz_decoder(compressed.as_slice())
            .read_to_string(&mut out)
            .unwrap();
        assert_eq!(out, "hello linux mysql tar.xz");
    }
}
