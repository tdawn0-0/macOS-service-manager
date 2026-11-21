//! Storage locations under the development home, ported from the Go
//! `storage_paths.go` and `DefaultRoot`.
//!
//! `MSM_DEV_HOME` wins over the user's Application Support directory. An
//! explicit root is self-contained: downloads, the catalog cache and the Java
//! directory stay inside it, so disposable CLI environments never write into
//! the user's shared runtime folders. Only the default root uses
//! `~/Library/Caches` and `~/Library/Java`.

use std::path::{Path, PathBuf};

use crate::model::{Package, java_folder_base};

/// The environment variable that replaces the development home.
pub const DEV_HOME_ENV: &str = "MSM_DEV_HOME";

/// The directory-name marker proving a Java bundle belongs to this install.
pub const JAVA_OWNER_FILE: &str = ".devbase-java-owner";

/// The production Homebrew Cellar for the host platform.
pub const DEFAULT_BREW_CELLAR: &str = crate::platform::paths::DEFAULT_BREW_CELLAR;

/// The default user storage root: `~/Library/Application Support/Devbase/Development`
/// on macOS, or `$XDG_DATA_HOME/devbase` (`~/.local/share/devbase`) on Linux.
pub fn default_storage_root(home: &Path) -> PathBuf {
    crate::platform::paths::default_storage_root(home)
}

/// The root the app opens by default: `$MSM_DEV_HOME`, else the storage root
/// under the current user's home.
pub fn default_root() -> PathBuf {
    if let Some(root) = std::env::var_os(DEV_HOME_ENV)
        && !root.is_empty()
    {
        return PathBuf::from(root);
    }
    default_storage_root(&home_from_environment())
}

/// The current user's home directory, as `os.UserHomeDir` reads it.
pub fn home_from_environment() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Whether the root is the shared user storage location.
pub fn is_user_storage(root: &Path, home: &Path) -> bool {
    crate::fs::clean_path(root) == default_storage_root(home)
}

/// The catalog/download cache directory.
pub fn cache_directory(root: &Path, home: &Path) -> PathBuf {
    if is_user_storage(root, home) {
        return crate::platform::paths::user_cache_directory(home);
    }
    root.join("cache")
}

/// Where downloaded archives live.
pub fn download_directory(root: &Path, home: &Path) -> PathBuf {
    if is_user_storage(root, home) {
        return cache_directory(root, home).join("downloads");
    }
    root.join("downloads")
}

/// The user-level JVM directory, or the private one under an explicit root.
pub fn java_directory(root: &Path, home: &Path) -> PathBuf {
    if is_user_storage(root, home) {
        return crate::platform::paths::user_java_directory(home);
    }
    root.join("JavaVirtualMachines")
}

/// The install directory for a package.
pub fn package_directory(
    root: &Path,
    home: &Path,
    brew_cellar: Option<&Path>,
    package: &Package,
) -> PathBuf {
    if !package.formula.is_empty() {
        let cellar = brew_cellar.unwrap_or_else(|| Path::new(DEFAULT_BREW_CELLAR));
        let formula = Path::new(&package.formula)
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&package.formula));
        return cellar.join(formula).join(&package.version);
    }
    if package.name == "java" {
        let folder = if package.java_folder.is_empty() {
            java_folder_base(package)
        } else {
            package.java_folder.clone()
        };
        return java_directory(root, home).join(folder);
    }
    root.join("packages").join(package.id())
}
