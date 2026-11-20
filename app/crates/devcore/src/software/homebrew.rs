//! Homebrew packages, ported from the Go `homebrew.go:20-244`.
//!
//! Formula metadata comes from `brew info --json=v2 --formula`; only an exact
//! stable version plus revision from `homebrew/core` is accepted. Installation
//! never replaces an existing keg or adopts another tap. Registration happens
//! after the keg's receipt and binaries verify, inside a state transaction.

use std::path::Path;

use serde::Deserialize;

use super::catalog;
use super::{BREW_PROGRAM, COMMAND_TIMEOUT, Cancelled, Software, SoftwareError};
use crate::model::{Package, homebrew_binaries, homebrew_formula_for};
use crate::state::PackageLease;

const BREW_ENV: [&str; 4] = [
    "HOMEBREW_NO_AUTO_UPDATE=1",
    "HOMEBREW_NO_INSTALL_CLEANUP=1",
    "HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1",
    BREW_PROGRAM,
];

#[derive(Debug, Deserialize)]
struct FormulaMetadata {
    #[serde(default)]
    name: String,
    #[serde(default)]
    full_name: String,
    #[serde(default)]
    license: String,
    #[serde(default)]
    revision: i64,
    #[serde(default)]
    versions: FormulaVersions,
    #[serde(default)]
    ruby_source_checksum: FormulaChecksum,
}

#[derive(Debug, Default, Deserialize)]
struct FormulaVersions {
    #[serde(default)]
    stable: String,
}

#[derive(Debug, Default, Deserialize)]
struct FormulaChecksum {
    #[serde(default)]
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct InfoResult {
    #[serde(default)]
    formulae: Vec<FormulaMetadata>,
}

#[derive(Debug, Deserialize)]
struct Receipt {
    source: ReceiptSource,
}

#[derive(Debug, Deserialize)]
struct ReceiptSource {
    #[serde(default)]
    tap: String,
    #[serde(default)]
    versions: ReceiptVersions,
}

#[derive(Debug, Default, Deserialize)]
struct ReceiptVersions {
    #[serde(default)]
    stable: String,
}

/// The formula name for `(name, version)`, with the Go error messages.
pub fn formula_for(name: &str, version: &str) -> Result<String, SoftwareError> {
    match homebrew_formula_for(name, version) {
        Some(formula) => Ok(formula),
        None => Err(SoftwareError::invalid(match name {
            "postgresql" => {
                "choose a PostgreSQL major version (for example 17) or an exact Homebrew version"
            }
            "redis" => "choose Redis 8.2 or an exact 8.2.x Homebrew version",
            _ => "Homebrew packages are only supported for PostgreSQL and Redis",
        })),
    }
}

impl Software {
    /// Resolve a package through Homebrew, Node's SHASUMS, or the pinned
    /// catalog. This is the Go `Manager.ResolvePackage`.
    pub fn resolve_package(&self, name: &str, version: &str) -> Result<Package, SoftwareError> {
        match name {
            "postgresql" | "redis" => self.resolve_homebrew(name, version),
            "node" => catalog::resolve_node(self.http(), version),
            _ => catalog::resolve(name, version),
        }
    }

    /// Resolve a Homebrew formula's exact stable keg.
    pub fn resolve_homebrew(&self, name: &str, requested: &str) -> Result<Package, SoftwareError> {
        let formula = formula_for(name, requested)?;
        let output = brew_run(self.runner(), &["info", "--json=v2", "--formula", &formula])?;
        let result: InfoResult = serde_json::from_str(&output)
            .map_err(|_| SoftwareError::invalid("invalid Homebrew formula metadata"))?;
        if result.formulae.len() != 1 {
            return Err(SoftwareError::invalid("invalid Homebrew formula metadata"));
        }
        let metadata = &result.formulae[0];
        let base = Path::new(&metadata.full_name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if metadata.full_name != formula || metadata.name != base || metadata.revision < 0 {
            return Err(SoftwareError::invalid(
                "Homebrew returned a different formula or tap",
            ));
        }
        let mut version = metadata.versions.stable.clone();
        if metadata.revision > 0 {
            version.push_str(&format!("_{}", metadata.revision));
        }
        let requested = requested.strip_prefix('v').unwrap_or(requested);
        let channel = formula
            .strip_prefix("postgresql@")
            .or_else(|| formula.strip_prefix("redis@"))
            .unwrap_or(&formula);
        if requested != channel && requested != version && requested != metadata.versions.stable {
            return Err(SoftwareError::invalid(format!(
                "Homebrew currently provides {formula} {version}, not {requested}; review the available version"
            )));
        }
        if !metadata.versions.stable.starts_with(&format!("{channel}.")) {
            return Err(SoftwareError::invalid(
                "Homebrew formula does not match the requested release series",
            ));
        }
        let package = Package {
            name: name.to_string(),
            version,
            arch: "arm64".into(),
            url: format!("https://formulae.brew.sh/formula/{formula}"),
            sha256: metadata.ruby_source_checksum.sha256.clone(),
            format: "homebrew".into(),
            formula: metadata.full_name.clone(),
            license: metadata.license.clone(),
            binaries: homebrew_binaries(name),
            ..Package::default()
        };
        package.validate()?;
        Ok(package)
    }

    /// Validate the keg's `INSTALL_RECEIPT.json` tap and stable version.
    pub(crate) fn check_homebrew_receipt(&self, package: &Package) -> Result<(), SoftwareError> {
        let path = self
            .store()
            .package_directory(package)
            .join("INSTALL_RECEIPT.json");
        let bytes = std::fs::read(&path).map_err(|error| {
            SoftwareError::invalid(format!(
                "Homebrew keg {} is missing; install it again in Software: {error}",
                package.id()
            ))
        })?;
        let receipt: Receipt = serde_json::from_slice(&bytes)
            .map_err(|_| SoftwareError::invalid("invalid Homebrew installation receipt"))?;
        let stable = package.version.split('_').next().unwrap_or_default();
        if receipt.source.tap != "homebrew/core" || receipt.source.versions.stable != stable {
            return Err(SoftwareError::invalid(format!(
                "Homebrew keg does not match {} {}; existing software from a different tap will not be adopted",
                package.formula, package.version
            )));
        }
        Ok(())
    }

    /// Install (or verify) a Homebrew keg and register it.
    pub(crate) fn install_homebrew(
        &self,
        package: &Package,
        cancelled: Cancelled<'_>,
    ) -> Result<(), SoftwareError> {
        let _lease = self
            .store()
            .package_lease_cancellable(&package.id(), true, cancelled)?;
        let state = self.store().read()?;
        if let Some(registered) = state.packages.get(&package.id()) {
            if registered != package {
                return Err(SoftwareError::invalid(
                    "installed package source differs; review the existing version",
                ));
            }
        } else {
            let current = self.resolve_package(&package.name, &package.version)?;
            if &current != package {
                return Err(SoftwareError::invalid(
                    "Homebrew formula changed since preview; review the package again",
                ));
            }
        }
        if let Err(receipt_error) = self.check_homebrew_receipt(package) {
            let directory = self.store().package_directory(package);
            if std::fs::symlink_metadata(&directory).is_ok() {
                return Err(receipt_error);
            }
            let current = self.resolve_package(&package.name, &package.version)?;
            if &current != package {
                return Err(SoftwareError::invalid(
                    "Homebrew formula changed since preview; review the package again",
                ));
            }
            let mut leases: Vec<PackageLease> = Vec::new();
            for installed in state.packages.values() {
                if installed.formula != package.formula {
                    continue;
                }
                if installed.id() != package.id() {
                    leases.push(self.store().package_lease_cancellable(
                        &installed.id(),
                        true,
                        cancelled,
                    )?);
                }
                self.require_package_idle(cancelled, &state, installed)?;
            }
            self.store()
                .emit(&format!("Installing with Homebrew: {}", package.formula));
            brew_run(
                self.runner(),
                &["install", "--formula", "--force-bottle", &package.formula],
            )?;
            drop(leases);
        }
        let mut installed_state = state.clone();
        installed_state
            .packages
            .insert(package.id(), package.clone());
        for name in package.binaries.keys() {
            self.binary(&installed_state, &package.id(), name)?;
        }
        self.store()
            .with_state_transaction_cancellable(cancelled, |state| {
                if let Some(current) = state.packages.get(&package.id())
                    && current != package
                {
                    return Err(crate::state::StateError::invalid(
                        "package registration changed during Homebrew installation",
                    ));
                }
                state.packages.insert(package.id(), package.clone());
                self.store()
                    .emit(&format!("Registered Homebrew keg {}", package.id()));
                Ok(crate::state::Tx::new(()))
            })?;
        Ok(())
    }
}

fn brew_run(
    runner: &dyn crate::engine::CommandRunner,
    args: &[&str],
) -> Result<String, SoftwareError> {
    let mut full: Vec<&str> = BREW_ENV.to_vec();
    full.extend_from_slice(args);
    runner.run("/usr/bin/env", &full, COMMAND_TIMEOUT).map_err(|error| {
        SoftwareError::invalid(format!(
            "Homebrew {}: {error}. Homebrew packages require Apple Silicon Homebrew at {BREW_PROGRAM}",
            args.first().copied().unwrap_or_default()
        ))
    })
}
