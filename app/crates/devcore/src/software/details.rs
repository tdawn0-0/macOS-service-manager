//! Package preview metadata, ported from the Go `package_details.go:30-192`.
//!
//! Preview is read-only: it validates the manifest, checks macOS and
//! architecture compatibility, reports cached/archive sizes, installed
//! integrity hints, and the instances referencing the package.

use std::path::PathBuf;

use serde::Serialize;

use super::catalog;
use super::{
    COMMAND_TIMEOUT, Cancelled, MAX_DOWNLOAD, Software, SoftwareError, has_digest_file,
    version_less,
};
use crate::model::Package;
use crate::platform::HostTarget;
use crate::state::StateError;

/// The UI's preview of one package version.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageDetails {
    pub package: Package,
    pub archive_bytes: i64,
    pub cached: bool,
    pub installed: bool,
    pub compatible: bool,
    pub compatibility: String,
    pub location: PathBuf,
    pub references: Vec<String>,
    pub integrity: String,
    pub integrity_note: String,
}

impl Software {
    /// Prefer an already installed exact version (works offline), then resolve
    /// through the catalog/Homebrew.
    pub fn resolve_installed_or_catalog(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Package, SoftwareError> {
        let version = version.trim().strip_prefix('v').unwrap_or(version.trim());
        if name == "node" && !catalog::is_exact_runtime_version(version) {
            return Err(SoftwareError::invalid(
                "resolve the Node.js version range to an exact release first",
            ));
        }
        let target = HostTarget::current();
        let state = self.store().read()?;
        for package in state.packages.values() {
            if package.name == name
                && package.version == version
                && target.accepts_model_arch(&package.arch)
            {
                package.validate()?;
                return Ok(package.clone());
            }
        }
        self.resolve_package(name, version)
    }

    /// Build preview metadata for a manifest.
    pub fn package_details(
        &self,
        cancelled: Cancelled<'_>,
        package: &Package,
    ) -> Result<PackageDetails, SoftwareError> {
        let target = HostTarget::current();
        let arch_compatible = target.accepts_model_arch(&package.arch);
        let location = if package.format == "system" {
            system_package_location(package)
                .unwrap_or_else(|| self.store().package_directory(package))
        } else {
            self.store().package_directory(package)
        };
        let mut info = PackageDetails {
            package: package.clone(),
            archive_bytes: 0,
            cached: false,
            installed: false,
            compatible: arch_compatible,
            compatibility: if arch_compatible {
                "Compatible architecture".into()
            } else {
                "This archive is for a different architecture".into()
            },
            location,
            references: Vec::new(),
            integrity: String::new(),
            integrity_note: String::new(),
        };
        package.validate()?;
        if cancelled() {
            return Err(cancelled_error());
        }
        if target.is_macos() && !package.min_macos.is_empty() {
            let version =
                self.runner()
                    .run("/usr/bin/sw_vers", &["-productVersion"], COMMAND_TIMEOUT)?;
            if version_less(version.trim(), &package.min_macos) {
                info.compatible = false;
                info.compatibility = format!("Requires macOS {} or newer", package.min_macos);
            } else if info.compatible {
                info.compatibility = format!(
                    "macOS {} meets the package's system requirement",
                    version.trim()
                );
            }
        }
        let state = self.store().read()?;
        if let Some(installed) = state.packages.get(&package.id()) {
            if installed.sha256 != package.sha256 {
                return Err(SoftwareError::invalid(
                    "installed package has a different checksum",
                ));
            }
            let installed = installed.clone();
            info.package = installed.clone();
            info.location = if installed.format == "system" {
                system_package_location(&installed)
                    .unwrap_or_else(|| self.store().package_directory(&installed))
            } else {
                self.store().package_directory(&installed)
            };
            info.installed = true;
            info.integrity = "Not checked".into();
            info.integrity_note =
                "Run Check files to compare the installed tree with its recorded index.".into();
            if state
                .package_integrity
                .get(&installed.id())
                .is_none_or(String::is_empty)
            {
                info.integrity = "Not recorded".into();
                info.integrity_note =
                    "Repair this version from the verified archive to record an installation index."
                        .into();
            }
            for name in installed.binaries.keys() {
                if let Err(error) = self.binary(&state, &installed.id(), name) {
                    info.integrity = "Needs repair".into();
                    info.integrity_note = error.to_string();
                }
            }
        }
        if !info.installed && info.package.name == "java" {
            let mut preview = info.package.clone();
            preview.java_folder = self.next_java_folder_for(&preview)?;
            info.location = self.store().package_directory(&preview);
        }
        for instance in state.instances.values() {
            if instance.package == info.package.id() {
                info.references.push(format!("Instance: {}", instance.id));
            }
        }
        info.references.sort();
        if info.package.format == "system" {
            info.compatibility =
                "Uses system-installed binaries managed by your OS package manager".into();
            if info.installed {
                info.integrity = "System managed".into();
                info.integrity_note =
                    "Check files validates the required system executables. Your OS package manager manages upgrades.".into();
                for name in info.package.binaries.keys() {
                    if let Err(error) = self.binary(&state, &info.package.id(), name) {
                        info.integrity = "Needs repair".into();
                        info.integrity_note = error.to_string();
                        break;
                    }
                }
            }
            return Ok(info);
        }
        if !info.package.formula.is_empty() {
            info.compatibility = "Requires Apple Silicon Homebrew; dependencies and downloads are managed by Homebrew".into();
            if info.installed {
                info.integrity = "Homebrew managed".into();
                info.integrity_note = "Check files validates the recorded keg's source and required executables. Homebrew manages its dependencies.".into();
                for name in info.package.binaries.keys() {
                    if let Err(error) = self.binary(&state, &info.package.id(), name) {
                        info.integrity = "Needs repair".into();
                        info.integrity_note = error.to_string();
                        break;
                    }
                }
            }
            return Ok(info);
        }
        let cache = self.store().download_directory().join(&info.package.sha256);
        if has_digest_file(&cache, &info.package.sha256, cancelled) {
            if let Ok(metadata) = std::fs::metadata(&cache) {
                info.archive_bytes = metadata.len() as i64;
                info.cached = true;
            }
        } else if info.compatible
            && !info.installed
            && let Ok(Some(size)) = self.http().size(&info.package.url)
        {
            info.archive_bytes = size;
        }
        if info.archive_bytes > MAX_DOWNLOAD {
            info.compatible = false;
            info.compatibility = "Archive exceeds the supported 2 GiB download limit".into();
        }
        if cancelled() {
            return Err(cancelled_error());
        }
        Ok(info)
    }

    fn next_java_folder_for(&self, package: &Package) -> Result<String, SoftwareError> {
        // The install path owns the definitive assignment; preview only needs
        // the first free name.
        let base = if package.java_folder.is_empty() {
            crate::model::java_folder_base(package)
        } else {
            package.java_folder.clone()
        };
        let directory = self.store().java_directory();
        for index in 0..10_000 {
            let name = if index == 0 {
                base.clone()
            } else {
                format!("{base}-{index}")
            };
            match std::fs::symlink_metadata(directory.join(&name)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(name),
                Ok(_) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(SoftwareError::invalid(
            "too many Java installation folders with this version",
        ))
    }
}

fn system_package_location(package: &Package) -> Option<PathBuf> {
    let primary = match package.name.as_str() {
        "postgresql" => package.binaries.get("postgres"),
        "redis" => package.binaries.get("redis-server"),
        _ => package.binaries.values().next(),
    }?;
    std::path::Path::new(primary)
        .parent()
        .map(std::path::Path::to_path_buf)
}

fn cancelled_error() -> SoftwareError {
    SoftwareError::State(StateError::Cancelled)
}
