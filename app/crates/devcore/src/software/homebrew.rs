//! Homebrew packages, ported from the Go `homebrew.go:20-244`.
//!
//! Formula metadata comes from `brew info --json=v2 --formula`; only an exact
//! stable version plus revision from `homebrew/core` is accepted. Installation
//! never replaces an existing keg or adopts another tap. Registration happens
//! after the keg's receipt and binaries verify, inside a state transaction.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::catalog;
use super::{BREW_PROGRAM, COMMAND_TIMEOUT, Cancelled, Software, SoftwareError, file_digest};
use crate::engine::CommandRunner;
use crate::model::{Package, homebrew_binaries, homebrew_formula_for};
use crate::platform::HostTarget;
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
    /// Resolve a package through Homebrew (or Linux system binaries), Node's
    /// SHASUMS, or the pinned catalog. This is the Go `Manager.ResolvePackage`.
    pub fn resolve_package(&self, name: &str, version: &str) -> Result<Package, SoftwareError> {
        match name {
            "postgresql" | "redis" => {
                if HostTarget::current().is_linux() {
                    match self.resolve_system_package(name, version) {
                        Ok(package) => Ok(package),
                        Err(sys_err) => {
                            if Path::new(BREW_PROGRAM).is_file() {
                                self.resolve_homebrew(name, version).or(Err(sys_err))
                            } else {
                                Err(sys_err)
                            }
                        }
                    }
                } else {
                    self.resolve_homebrew(name, version)
                }
            }
            "node" => catalog::resolve_node(self.http(), version),
            _ => catalog::resolve(name, version),
        }
    }

    /// Auto-discover system-installed PostgreSQL or Redis binaries on Linux
    /// (without requiring Homebrew or root).
    pub fn resolve_system_package(
        &self,
        name: &str,
        requested: &str,
    ) -> Result<Package, SoftwareError> {
        let search_dirs = system_binary_search_dirs(name, requested);
        discover_system_package_in_dirs(
            name,
            requested,
            HostTarget::current(),
            &search_dirs,
            self.runner(),
        )
    }

    /// Register a discovered Linux system package (`format == "system"`) after
    /// verifying all required executables exist and match the host architecture.
    pub(crate) fn install_system(
        &self,
        package: &Package,
        cancelled: Cancelled<'_>,
    ) -> Result<(), SoftwareError> {
        package.validate()?;
        let _lease = self
            .store()
            .package_lease_cancellable(&package.id(), true, cancelled)?;
        let state = self.store().read()?;
        if let Some(registered) = state.packages.get(&package.id())
            && registered != package
        {
            return Err(SoftwareError::invalid(
                "installed package source differs; review the existing version",
            ));
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
                        "package registration changed during system package registration",
                    ));
                }
                state.packages.insert(package.id(), package.clone());
                self.store()
                    .emit(&format!("Registered system package {}", package.id()));
                Ok(crate::state::Tx::new(()))
            })?;
        Ok(())
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
            arch: HostTarget::current().arch_name().into(),
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
    runner
        .run("/usr/bin/env", &full, COMMAND_TIMEOUT)
        .map_err(|error| {
            let hint = if HostTarget::current().is_macos() {
                format!("Homebrew packages require Apple Silicon Homebrew at {BREW_PROGRAM}")
            } else {
                format!("Homebrew packages require Linuxbrew at {BREW_PROGRAM}")
            };
            SoftwareError::invalid(format!(
                "Homebrew {}: {error}. {hint}",
                args.first().copied().unwrap_or_default()
            ))
        })
}

/// Ordered candidate binary directories checked when discovering system-installed
/// PostgreSQL or Redis on Linux.
pub fn system_binary_search_dirs(name: &str, requested: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let requested = requested
        .trim()
        .strip_prefix('v')
        .unwrap_or(requested.trim());
    let requested_major = requested.split('.').next().unwrap_or_default();

    if name == "postgresql" {
        // Debian/Ubuntu versioned directories: /usr/lib/postgresql/<major>/bin
        let mut pg_versioned = scan_versioned_bin_dirs(Path::new("/usr/lib/postgresql"), None);
        // RHEL/Fedora PGDG directories: /usr/pgsql-<major>/bin
        pg_versioned.extend(scan_versioned_bin_dirs(Path::new("/usr"), Some("pgsql-")));
        // Prioritize requested major version first, then descending by version.
        pg_versioned.sort_by(|(v_a, _), (v_b, _)| {
            let match_a = !requested_major.is_empty() && v_a == requested_major;
            let match_b = !requested_major.is_empty() && v_b == requested_major;
            match_b
                .cmp(&match_a)
                .then_with(|| parse_major_num(v_b).cmp(&parse_major_num(v_a)))
                .then_with(|| v_b.cmp(v_a))
        });
        for (_, bin_dir) in pg_versioned {
            push_unique_dir(&mut dirs, bin_dir);
        }
        push_unique_dir(&mut dirs, PathBuf::from("/usr/bin"));
        push_unique_dir(&mut dirs, PathBuf::from("/usr/local/bin"));
        if !requested_major.is_empty() && requested_major.bytes().all(|b| b.is_ascii_digit()) {
            push_unique_dir(
                &mut dirs,
                PathBuf::from(format!(
                    "/home/linuxbrew/.linuxbrew/opt/postgresql@{requested_major}/bin"
                )),
            );
        }
        push_unique_dir(&mut dirs, PathBuf::from("/home/linuxbrew/.linuxbrew/bin"));
    } else if name == "redis" {
        push_unique_dir(&mut dirs, PathBuf::from("/usr/bin"));
        push_unique_dir(&mut dirs, PathBuf::from("/usr/local/bin"));
        push_unique_dir(
            &mut dirs,
            PathBuf::from("/home/linuxbrew/.linuxbrew/opt/redis@8.2/bin"),
        );
        push_unique_dir(
            &mut dirs,
            PathBuf::from("/home/linuxbrew/.linuxbrew/opt/redis/bin"),
        );
        push_unique_dir(&mut dirs, PathBuf::from("/home/linuxbrew/.linuxbrew/bin"));
    }

    if let Some(path_env) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_env) {
            if dir.is_absolute() {
                push_unique_dir(&mut dirs, dir);
            }
        }
    }
    dirs
}

/// Discovers a system-installed PostgreSQL or Redis binary set inside `search_dirs`
/// and builds a validated `format == "system"` [`Package`] for `target`.
pub fn discover_system_package_in_dirs(
    name: &str,
    requested: &str,
    target: HostTarget,
    search_dirs: &[PathBuf],
    runner: &dyn CommandRunner,
) -> Result<Package, SoftwareError> {
    if !matches!(name, "postgresql" | "redis") {
        return Err(SoftwareError::invalid(
            "system package discovery is only supported for PostgreSQL and Redis",
        ));
    }
    let primary_name = if name == "postgresql" {
        "postgres"
    } else {
        "redis-server"
    };
    let required: Vec<String> = homebrew_binaries(name).into_keys().collect();
    let requested_clean = requested
        .trim()
        .strip_prefix('v')
        .unwrap_or(requested.trim());

    let mut mismatch_versions: Vec<String> = Vec::new();
    let mut seen_primaries: Vec<PathBuf> = Vec::new();

    for dir in search_dirs {
        let primary = dir.join(primary_name);
        if !is_executable_file(&primary) {
            continue;
        }
        let canonical = std::fs::canonicalize(&primary).unwrap_or_else(|_| primary.clone());
        if seen_primaries.contains(&canonical) {
            continue;
        }
        seen_primaries.push(canonical);

        // Resolve every required binary, preferring the same directory as the primary server.
        let mut resolved = BTreeMap::new();
        let mut complete = true;
        for bin in &required {
            let local = dir.join(bin);
            if is_executable_file(&local) {
                resolved.insert(bin.clone(), local.to_string_lossy().into_owned());
            } else if let Some(found) = search_dirs
                .iter()
                .map(|d| d.join(bin))
                .find(|p| is_executable_file(p))
            {
                resolved.insert(bin.clone(), found.to_string_lossy().into_owned());
            } else {
                complete = false;
                break;
            }
        }
        if !complete {
            continue;
        }

        let version_output = runner.run(
            &primary.to_string_lossy(),
            &["--version"],
            std::time::Duration::from_secs(10),
        )?;
        let Some(version) = parse_system_db_version(name, &version_output) else {
            continue;
        };

        let version_matches = requested_clean.is_empty()
            || requested_clean.eq_ignore_ascii_case("system")
            || version == requested_clean
            || version.starts_with(&format!("{requested_clean}."));
        if !version_matches {
            mismatch_versions.push(version);
            continue;
        }

        let sha256 = file_digest(&primary, &|| false).ok_or_else(|| {
            SoftwareError::invalid(format!(
                "could not compute checksum for {}",
                primary.display()
            ))
        })?;
        let license = if name == "postgresql" {
            "PostgreSQL"
        } else {
            "Redis / Valkey (system package)"
        };
        let package = Package {
            name: name.to_string(),
            version,
            arch: target.arch_name().into(),
            url: String::new(),
            sha256,
            format: "system".into(),
            strip: 0,
            binaries: resolved,
            license: license.into(),
            ..Package::default()
        };
        package.validate_for_target(target)?;
        return Ok(package);
    }

    if !mismatch_versions.is_empty() {
        return Err(SoftwareError::invalid(format!(
            "system {name} provides {}, not {requested_clean}; select {} or \"system\"",
            mismatch_versions.join(", "),
            mismatch_versions[0]
        )));
    }
    let install_hint = if name == "postgresql" {
        "install PostgreSQL with your package manager (e.g. `pacman -S postgresql`, `apt install postgresql`, or `dnf install postgresql-server`)"
    } else {
        "install Redis with your package manager (e.g. `pacman -S redis`, `apt install redis-server`, or `dnf install redis`)"
    };
    Err(SoftwareError::invalid(format!(
        "no system {name} installation found; {install_hint}"
    )))
}

/// Parses the version string from `postgres --version` or `redis-server --version`.
pub fn parse_system_db_version(name: &str, output: &str) -> Option<String> {
    match name {
        "postgresql" => {
            // e.g. "postgres (PostgreSQL) 17.4" or "postgres (PostgreSQL) 16.8 (Ubuntu ...)"
            for token in output.split_whitespace() {
                let candidate = token.trim_end_matches([',', ';', ')']);
                if candidate.bytes().next().is_some_and(|b| b.is_ascii_digit())
                    && candidate.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                    && candidate.contains('.')
                {
                    return Some(candidate.to_string());
                }
            }
            // Also accept a standalone major version like "17" if no dot appears
            output
                .split_whitespace()
                .find(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
                .map(ToString::to_string)
        }
        "redis" => {
            // e.g. "Redis server v=7.4.2 sha=00000000:0 malloc=jemalloc ..."
            // or "Valkey server v=8.0.2 sha=..."
            for token in output.split_whitespace() {
                if let Some(ver) = token.strip_prefix("v=") {
                    let clean: String = ver
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.')
                        .collect();
                    if !clean.is_empty() && clean.contains('.') {
                        return Some(clean);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn scan_versioned_bin_dirs(parent: &Path, prefix: Option<&str>) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let version = match prefix {
            Some(p) => match name.strip_prefix(p) {
                Some(rest) if !rest.is_empty() => rest,
                _ => continue,
            },
            None => name,
        };
        let bin_dir = entry.path().join("bin");
        if bin_dir.is_dir() {
            out.push((version.to_string(), bin_dir));
        }
    }
    out
}

fn parse_major_num(value: &str) -> u32 {
    value
        .split('.')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111) != 0)
}

#[cfg(test)]
mod system_tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::launchd::CommandError;

    struct StubRunner(String);
    impl CommandRunner for StubRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _timeout: Duration,
        ) -> Result<String, CommandError> {
            Ok(self.0.clone())
        }
    }

    fn write_exec(dir: &Path, name: &str) {
        let path = dir.join(name);
        fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn parses_postgres_and_redis_versions() {
        assert_eq!(
            parse_system_db_version("postgresql", "postgres (PostgreSQL) 17.4\n"),
            Some("17.4".into())
        );
        assert_eq!(
            parse_system_db_version(
                "postgresql",
                "postgres (PostgreSQL) 16.8 (Ubuntu 16.8-1.pgdg24.04+1)\n"
            ),
            Some("16.8".into())
        );
        assert_eq!(
            parse_system_db_version(
                "redis",
                "Redis server v=7.4.2 sha=00000000:0 malloc=jemalloc-5.3.0 bits=64\n"
            ),
            Some("7.4.2".into())
        );
        assert_eq!(
            parse_system_db_version(
                "redis",
                "Valkey server v=8.0.2 sha=00000000:0 malloc=jemalloc-5.3.0 bits=64\n"
            ),
            Some("8.0.2".into())
        );
    }

    #[test]
    fn discovers_linux_system_postgres_and_redis_without_homebrew() {
        let dir = TempDir::new().unwrap();
        let pg_bin = dir.path().join("pg17/bin");
        fs::create_dir_all(&pg_bin).unwrap();
        for name in [
            "postgres",
            "initdb",
            "pg_dump",
            "pg_restore",
            "psql",
            "createdb",
        ] {
            write_exec(&pg_bin, name);
        }

        let runner = StubRunner("postgres (PostgreSQL) 17.4\n".into());
        let pkg = discover_system_package_in_dirs(
            "postgresql",
            "17",
            HostTarget::LINUX_AMD64,
            std::slice::from_ref(&pg_bin),
            &runner,
        )
        .unwrap();
        assert_eq!(pkg.name, "postgresql");
        assert_eq!(pkg.version, "17.4");
        assert_eq!(pkg.arch, "amd64");
        assert_eq!(pkg.format, "system");
        assert!(pkg.formula.is_empty());
        assert_eq!(
            pkg.binaries["postgres"],
            pg_bin.join("postgres").to_string_lossy()
        );
        assert!(pkg.validate_for_target(HostTarget::LINUX_AMD64).is_ok());

        // Requesting "system" also succeeds
        let pkg_any = discover_system_package_in_dirs(
            "postgresql",
            "system",
            HostTarget::LINUX_AMD64,
            std::slice::from_ref(&pg_bin),
            &runner,
        )
        .unwrap();
        assert_eq!(pkg_any.version, "17.4");

        // Requesting wrong major version gives a clear mismatch message
        let err = discover_system_package_in_dirs(
            "postgresql",
            "16",
            HostTarget::LINUX_AMD64,
            std::slice::from_ref(&pg_bin),
            &runner,
        )
        .unwrap_err();
        assert!(err.to_string().contains("17.4"), "{err}");

        // Redis discovery
        let redis_bin = dir.path().join("redis/bin");
        fs::create_dir_all(&redis_bin).unwrap();
        write_exec(&redis_bin, "redis-server");
        write_exec(&redis_bin, "redis-cli");
        let redis_runner = StubRunner("Redis server v=7.4.2 sha=00000000:0 bits=64\n".into());
        let redis = discover_system_package_in_dirs(
            "redis",
            "system",
            HostTarget::LINUX_ARM64,
            std::slice::from_ref(&redis_bin),
            &redis_runner,
        )
        .unwrap();
        assert_eq!(redis.name, "redis");
        assert_eq!(redis.version, "7.4.2");
        assert_eq!(redis.arch, "arm64");
        assert_eq!(redis.format, "system");
        assert!(redis.validate_for_target(HostTarget::LINUX_ARM64).is_ok());
    }
}
