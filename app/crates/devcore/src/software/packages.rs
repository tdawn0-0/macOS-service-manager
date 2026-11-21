//! Package install, uninstall and binary resolution, ported from the Go
//! `packages.go:43-370`.
//!
//! The pipeline is: download to `downloads/<sha256>`, stage beside the
//! destination, verify (SHA-256, archive safety, arm64 slices), publish with
//! `renamex_np`, and register inside the state transaction. A failed
//! publication restores the previous tree and index; installation and repair
//! hold only the package lease while staging, then re-check references inside
//! the short state transaction.

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::archive::extract;
use super::integrity::{encode_package_tree, scan_package_tree, write_index};
use super::macho::require_host_executable;
use super::swap::{exchange_package_directories, publish_new_package_directory};
use super::{
    COMMAND_TIMEOUT, Cancelled, MAX_DOWNLOAD, Software, SoftwareError, canonical_inside,
    file_digest, has_digest_file, integrity_path, random_id, version_less,
};
use crate::engine::Engine;
use crate::fs::{PRIVATE_FILE, atomic_write, create_private_dir_all};
use crate::model::{Package, State, java_folder_base};
use crate::platform::HostTarget;
use crate::state::StateError;

/// The directory-name marker proving a Java bundle belongs to this install.
pub const JAVA_OWNER_FILE: &str = ".devbase-java-owner";

/// A staged, verified installation waiting for publication.
pub(crate) struct StagedPackage {
    pub(crate) dir: PathBuf,
    pub(crate) index: Vec<u8>,
    pub(crate) digest: String,
    /// Set once publication owns rollback/cleanup; dropping must not erase a
    /// tree retained after a failed rollback.
    keep: bool,
}

impl Drop for StagedPackage {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl Software {
    /// Resolve an executable from an installed package.
    pub fn binary(&self, state: &State, id: &str, name: &str) -> Result<PathBuf, SoftwareError> {
        let package = state.packages.get(id).ok_or_else(|| {
            SoftwareError::invalid(format!(
                "package {id} is not installed; install it before starting"
            ))
        })?;
        let target = HostTarget::current();
        if !target.accepts_model_arch(&package.arch) {
            return Err(SoftwareError::invalid(if target.is_macos() {
                "managed software requires Apple Silicon (arm64)"
            } else {
                "package architecture does not match this Linux host"
            }));
        }
        if package.id() != id {
            return Err(SoftwareError::invalid(
                "installed package identity mismatch",
            ));
        }
        if !package.formula.is_empty()
            || package.format == "system"
            || package.name == "postgresql"
            || package.name == "redis"
            || package.name == "java"
        {
            package.validate()?;
        }
        if !package.formula.is_empty() {
            self.check_homebrew_receipt(package)?;
        }
        self.check_java_ownership(package)?;
        let rel = package
            .binaries
            .get(name)
            .ok_or_else(|| SoftwareError::invalid(format!("{id} does not provide {name}")))?;
        let path = if package.format == "system" {
            PathBuf::from(rel)
        } else {
            self.store().package_directory(package).join(rel)
        };
        let metadata = std::fs::metadata(&path)?;
        if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
            return Err(SoftwareError::invalid(format!(
                "{} is not executable",
                path.display()
            )));
        }
        require_host_executable(&path)?;
        Ok(path)
    }

    /// Stage a package beside its destination and return its verified index.
    pub(crate) fn stage_package(
        &self,
        package: &Package,
        cancelled: Cancelled<'_>,
    ) -> Result<StagedPackage, SoftwareError> {
        package.validate()?;
        if HostTarget::current().is_macos() && !package.min_macos.is_empty() {
            let output =
                self.runner()
                    .run("/usr/bin/sw_vers", &["-productVersion"], COMMAND_TIMEOUT)?;
            if version_less(output.trim(), &package.min_macos) {
                return Err(SoftwareError::invalid(format!(
                    "{} requires macOS {} or newer",
                    package.id(),
                    package.min_macos
                )));
            }
        }
        self.store().emit(&format!("Downloading {}", package.id()));
        let cache = self.ensure_download(package, cancelled)?;
        let parent = self
            .store()
            .package_directory(package)
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| SoftwareError::invalid("package destination has no parent"))?;
        create_private_dir_all(&parent)?;
        match std::fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            _ => {
                return Err(SoftwareError::invalid(
                    "package parent must be a plain directory",
                ));
            }
        }
        let tmp = parent.join(format!(".install-{}", random_id()));
        std::fs::create_dir(&tmp)?;
        let mut staged = StagedPackage {
            dir: tmp.clone(),
            index: Vec::new(),
            digest: String::new(),
            keep: false,
        };
        self.store()
            .emit(&format!("Verifying and extracting {}", package.id()));
        extract(&cache, &tmp, &package.format, package.strip, cancelled)?;
        if cancelled() {
            return Err(cancelled_error());
        }
        if package.name == "java" {
            validate_java_bundle(&tmp, package)?;
            atomic_write(
                &tmp.join(JAVA_OWNER_FILE),
                self.java_owner(package).as_bytes(),
                PRIVATE_FILE,
            )?;
        }
        for rel in package.binaries.values() {
            let path = tmp.join(rel);
            let real = std::fs::canonicalize(&path)?;
            if !canonical_inside(&tmp, &real) {
                return Err(SoftwareError::invalid("binary escapes package directory"));
            }
            let metadata = std::fs::metadata(&real)?;
            if !metadata.is_file() {
                return Err(SoftwareError::invalid("binary is not a regular file"));
            }
            require_host_executable(&real)?;
            let mut permissions = metadata.permissions();
            permissions.set_mode(metadata.mode() | 0o100);
            std::fs::set_permissions(&real, permissions)?;
        }
        let mut manifest = serde_json::to_vec_pretty(package)?;
        manifest.push(b'\n');
        atomic_write(&tmp.join(".package.json"), &manifest, PRIVATE_FILE)?;
        let tree = scan_package_tree(self, cancelled, &tmp, package)?;
        let (index, digest) = encode_package_tree(&tree)?;
        staged.index = index;
        staged.digest = digest;
        Ok(staged)
    }

    /// Install a package (or verify an already installed identical one).
    pub fn install(
        &self,
        package: &Package,
        cancelled: Cancelled<'_>,
    ) -> Result<(), SoftwareError> {
        package.validate()?;
        self.store().init()?;
        if package.format == "system" {
            return self.install_system(package, cancelled);
        }
        if !package.formula.is_empty() {
            return self.install_homebrew(package, cancelled);
        }
        let state = self.store().read()?;
        let installed = state.packages.contains_key(&package.id());
        let mut _lease = Some(self.store().package_lease_cancellable(
            &package.id(),
            !installed,
            cancelled,
        )?);
        let state = self.store().read()?;
        if let Some(current) = state.packages.get(&package.id()) {
            if current.id() != package.id() {
                return Err(SoftwareError::invalid(
                    "installed package identity mismatch",
                ));
            }
            if current.sha256 != package.sha256 {
                return Err(SoftwareError::invalid(
                    "installed package identity has a different checksum",
                ));
            }
            self.store()
                .emit(&format!("Checking installed package {}", package.id()));
            let report = self.verify_package_in_state(cancelled, &state, current)?;
            if report.status == "Needs repair" {
                return Err(SoftwareError::invalid(
                    "installed package is incomplete or changed; use Repair this version",
                ));
            }
            return Ok(());
        }
        if installed {
            _lease.take();
            _lease = Some(self.store().package_lease_cancellable(
                &package.id(),
                true,
                cancelled,
            )?);
            let state = self.store().read()?;
            if state.packages.contains_key(&package.id()) {
                return Err(SoftwareError::invalid(
                    "package registration changed; review it again",
                ));
            }
        }
        let state = self.store().read()?;
        self.require_package_idle(cancelled, &state, package)?;
        let mut package = package.clone();
        if package.name == "java" {
            package.java_folder = self.next_java_folder(&package)?;
            self.store().emit(&format!(
                "Installing Java into {}",
                self.store().package_directory(&package).display()
            ));
        }
        self.prepare_package_for_publication(cancelled, &package, false)
    }

    /// Uninstall a package; external Homebrew kegs are never deleted.
    pub fn uninstall(&self, id: &str, cancelled: Cancelled<'_>) -> Result<(), SoftwareError> {
        self.store()
            .with_state_transaction_cancellable(cancelled, |state| {
                let _lease = self
                    .store()
                    .package_lease_cancellable(id, true, cancelled)?;
                let package = state
                    .packages
                    .get(id)
                    .cloned()
                    .ok_or_else(|| StateError::invalid("package not installed"))?;
                if package.id() != id {
                    return Err(StateError::invalid("installed package identity mismatch"));
                }
                for instance in state.instances.values() {
                    if instance.package == id {
                        return Err(StateError::invalid(format!(
                            "package is used by instance {}",
                            instance.id
                        )));
                    }
                }
                let engine = Engine::new(self.store().root(), self.runner.clone());
                let referenced_err = || {
                    StateError::invalid(if HostTarget::current().is_macos() {
                        "package is referenced by a launchd job"
                    } else {
                        "package is referenced by a managed service job"
                    })
                };
                if package.format == "system" {
                    for bin_path in package.binaries.values() {
                        if !engine
                            .jobs_referencing(Path::new(bin_path))
                            .map_err(StateError::from)?
                            .is_empty()
                        {
                            return Err(referenced_err());
                        }
                    }
                } else {
                    let directory = self.store().package_directory(&package);
                    if !engine
                        .jobs_referencing(&directory)
                        .map_err(StateError::from)?
                        .is_empty()
                    {
                        return Err(referenced_err());
                    }
                    let jobs = self
                        .store()
                        .root()
                        .join("jobs")
                        .read_dir()
                        .map(|entries| {
                            entries
                                .filter_map(Result::ok)
                                .map(|entry| entry.path())
                                .filter(|path| path.extension() == Some(OsStr::new("plist")))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    for path in jobs {
                        let value = plist::Value::from_file(&path)
                            .map_err(|error| StateError::invalid(error.to_string()))?;
                        if value_references(&value, &directory) {
                            return Err(referenced_err());
                        }
                    }
                }
                if package.formula.is_empty() && package.format != "system" {
                    if package.name == "java" {
                        package.validate()?;
                    }
                    self.check_java_ownership(&package)
                        .map_err(software_to_state)?;
                    remove_all_if_present(&self.store().package_directory(&package))
                        .map_err(StateError::Io)?;
                }
                if package.name == "node" {
                    if state.node_default == id {
                        state.node_default.clear();
                    }
                    let prefix = self.node_global_prefix(&package.version);
                    remove_all_if_present(&prefix).map_err(StateError::Io)?;
                }
                let runtime_match = state.runtime_defaults.get(&package.name)
                    == Some(&id.to_string())
                    || (package.name == "java"
                        && state.runtime_defaults.get("java")
                            == Some(&java_installation_id(
                                &self.store().package_directory(&package),
                            )));
                if runtime_match {
                    state.runtime_defaults.remove(&package.name);
                }
                state.packages.remove(id);
                state.package_integrity.remove(id);
                if package.name == "node" {
                    self.link_node_default(state).map_err(software_to_state)?;
                }
                match std::fs::remove_file(integrity_path(self.store(), id)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(StateError::Io(error)),
                }
                Ok(crate::state::Tx::new(()))
            })?;
        Ok(())
    }

    /// Re-read state, refuse a package used by a database or a launchd job,
    /// then stage and publish. Repair additionally proves the record is
    /// unchanged.
    pub(crate) fn prepare_package_for_publication(
        &self,
        cancelled: Cancelled<'_>,
        package: &Package,
        repair: bool,
    ) -> Result<(), SoftwareError> {
        let staged = self.stage_package(package, cancelled)?;
        let store = self.store().clone();
        let software = self.clone();
        let package = package.clone();
        let staged = std::cell::RefCell::new(Some(staged));
        let result = store.with_state_transaction_cancellable(cancelled, |state| {
            let current = state.packages.get(&package.id());
            if repair {
                if current != Some(&package) {
                    return Err(StateError::invalid(
                        "installed package record changed while preparing repair; review it again",
                    ));
                }
            } else if current.is_some() {
                return Err(StateError::invalid(
                    "package was registered while preparing installation; review it again",
                ));
            }
            software
                .require_package_idle(cancelled, state, &package)
                .map_err(software_to_state)?;
            let mut staged = staged
                .borrow_mut()
                .take()
                .ok_or_else(|| StateError::invalid("package staging was already published"))?;
            software.publish_package(cancelled, state, &package, &mut staged, repair)
        });
        result.map_err(SoftwareError::State)
    }

    /// Publish a staged tree and register it. Errors after a filesystem side
    /// effect roll back before returning, matching the Go transaction hooks.
    fn publish_package(
        &self,
        cancelled: Cancelled<'_>,
        state: &mut State,
        package: &Package,
        staged: &mut StagedPackage,
        repair: bool,
    ) -> Result<crate::state::Tx<()>, StateError> {
        staged.keep = true;
        let dest = self.store().package_directory(package);
        let index = integrity_path(self.store(), &package.id());
        let old_index = self
            .store()
            .root()
            .join("integrity")
            .join(format!(".previous-{}.json", random_id()));
        let mut flags = PublishFlags::default();
        let result = (|| -> Result<(), SoftwareError> {
            let exists = match std::fs::symlink_metadata(&dest) {
                Ok(metadata) => {
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(SoftwareError::invalid(
                            "installation path is not a plain directory; reveal it for inspection before repair",
                        ));
                    }
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            if exists {
                self.check_java_ownership(package)?;
            }
            if exists && !repair {
                let bytes = std::fs::read(dest.join(".package.json"))?;
                let prior: Package = serde_json::from_slice(&bytes)?;
                if prior.id() != package.id() || prior.sha256 != package.sha256 {
                    return Err(SoftwareError::invalid(
                        "an unregistered package directory exists; inspect it before retrying",
                    ));
                }
            }
            match std::fs::symlink_metadata(&index) {
                Ok(metadata) => {
                    if !metadata.is_file() && !metadata.file_type().is_symlink() {
                        return Err(SoftwareError::invalid("integrity index path is not a file"));
                    }
                    std::fs::rename(&index, &old_index)?;
                    flags.index_moved = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if exists {
                exchange_package_directories(&staged.dir, &dest)?;
                flags.swapped = true;
            } else {
                publish_new_package_directory(&staged.dir, &dest)?;
                flags.renamed = true;
            }
            write_index(&index, &staged.index)?;
            flags.index_written = true;
            if cancelled() {
                return Err(SoftwareError::State(StateError::Cancelled));
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Err(rollback_error) = rollback(&flags, &staged.dir, &dest, &index, &old_index) {
                return Err(StateError::invalid(format!(
                    "{error}; rollback failed: {rollback_error}"
                )));
            }
            return Err(software_to_state(error));
        }
        state
            .package_integrity
            .insert(package.id(), staged.digest.clone());
        state.packages.insert(package.id(), package.clone());
        self.store()
            .emit(&format!("Committing package files for {}", package.id()));
        let stage_dir = staged.dir.clone();
        let dest_clone = dest.clone();
        let index_clone = index.clone();
        let old_clone = old_index.clone();
        let undo_flags = flags.clone();
        let undo = move || -> Result<(), StateError> {
            rollback(
                &undo_flags,
                &stage_dir,
                &dest_clone,
                &index_clone,
                &old_clone,
            )
            .map_err(software_to_state)
        };
        let store = self.store().clone();
        let id = package.id();
        let verb = if repair { "Repaired " } else { "Installed " };
        let stage_dir = staged.dir.clone();
        let old_index = old_index.clone();
        let index_moved = flags.index_moved;
        let commit = move || {
            if std::fs::remove_dir_all(&stage_dir).is_err() && stage_dir.exists() {
                store.emit(&format!(
                    "Old package files retained for inspection: {}",
                    stage_dir.display()
                ));
            }
            if index_moved {
                let _ = std::fs::remove_file(&old_index);
            }
            store.emit(&format!("{verb}{id}"));
        };
        Ok(crate::state::Tx::new(())
            .with_undo(undo)
            .with_commit(commit))
    }

    /// Check every database instance and managed job that names the package.
    pub(crate) fn require_package_idle(
        &self,
        cancelled: Cancelled<'_>,
        state: &State,
        package: &Package,
    ) -> Result<(), SoftwareError> {
        let engine = Engine::new(self.store().root(), self.runner.clone());
        for instance in state.instances.values() {
            if instance.package != package.id() {
                continue;
            }
            let loaded = self.package_job_loaded(&engine, &format!("db-{}", instance.id))?;
            if loaded {
                return Err(SoftwareError::invalid(format!(
                    "stop database {} before repairing its software",
                    instance.id
                )));
            }
        }
        let stop_job_error = || {
            SoftwareError::invalid(if HostTarget::current().is_macos() {
                "stop the launchd job using this package before repairing it"
            } else {
                "stop the managed service job using this package before repairing it"
            })
        };
        let directory = self.store().package_directory(package);
        let referenced_ids = engine
            .jobs_referencing(&directory)
            .map_err(|error| SoftwareError::invalid(error.to_string()))?;
        for job_id in &referenced_ids {
            if cancelled() {
                return Err(cancelled_error());
            }
            if self.package_job_loaded(&engine, job_id)? {
                return Err(stop_job_error());
            }
        }
        let jobs = self.store().root().join("jobs");
        let mut files: Vec<PathBuf> = match std::fs::read_dir(&jobs) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension() == Some(OsStr::new("plist")))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        files.sort();
        let label_prefix = engine.label("");
        for file in files {
            if cancelled() {
                return Err(cancelled_error());
            }
            let mut bytes = Vec::new();
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&file)?
                .take((1 << 20) + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 1 << 20 {
                return Err(SoftwareError::invalid(
                    "launchd job is too large to inspect safely",
                ));
            }
            let value = plist::Value::from_reader(std::io::Cursor::new(&bytes))
                .map_err(|error| SoftwareError::invalid(error.to_string()))?;
            if !value_references(&value, &directory) {
                continue;
            }
            let label = value
                .as_dictionary()
                .and_then(|dictionary| dictionary.get("Label"))
                .and_then(plist::Value::as_string)
                .unwrap_or_default();
            let Some(job_id) = label.strip_prefix(&label_prefix) else {
                return Err(SoftwareError::invalid(
                    "a job outside this workspace references the package; inspect it before repair",
                ));
            };
            if self.package_job_loaded(&engine, job_id)? {
                return Err(stop_job_error());
            }
        }
        if cancelled() {
            return Err(cancelled_error());
        }
        Ok(())
    }

    fn package_job_loaded(&self, engine: &Engine, job_id: &str) -> Result<bool, SoftwareError> {
        match engine.job_state(job_id) {
            Ok(Some(state)) => Ok(state.loaded || state.running),
            Ok(None) => Ok(false),
            Err(error) => Err(SoftwareError::invalid(format!(
                "could not verify whether the package is in use: {error}"
            ))),
        }
    }

    fn ensure_download(
        &self,
        package: &Package,
        cancelled: Cancelled<'_>,
    ) -> Result<PathBuf, SoftwareError> {
        let directory = self.store().download_directory();
        create_private_dir_all(&directory)?;
        let cache = directory.join(&package.sha256);
        if has_digest_file(&cache, &package.sha256, cancelled) {
            return Ok(cache);
        }
        if cancelled() {
            return Err(cancelled_error());
        }
        let temporary = directory.join(format!(".download-{}", random_id()));
        let result = (|| -> Result<(), SoftwareError> {
            self.http().download(&package.url, &temporary)?;
            let size = std::fs::metadata(&temporary)?.len() as i64;
            if size > MAX_DOWNLOAD {
                return Err(SoftwareError::invalid("download exceeds limit"));
            }
            if file_digest(&temporary, cancelled).as_deref() != Some(&package.sha256) {
                return Err(SoftwareError::invalid("download checksum mismatch"));
            }
            std::fs::rename(&temporary, &cache)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
        Ok(cache)
    }

    fn java_owner(&self, package: &Package) -> String {
        format!(
            "{}\n{}\n{}\n",
            crate::fs::clean_path(self.store().root()).display(),
            package.id(),
            package.sha256
        )
    }

    fn check_java_ownership(&self, package: &Package) -> Result<(), SoftwareError> {
        if package.name != "java" || !package.formula.is_empty() {
            return Ok(());
        }
        let directory = self.store().package_directory(package);
        let parent = directory
            .parent()
            .ok_or_else(|| SoftwareError::invalid("Java installation has no parent"))?;
        for path in [parent, directory.as_path()] {
            let metadata = std::fs::symlink_metadata(path)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(SoftwareError::invalid(format!(
                    "Java installation path is not a plain directory: {}",
                    path.display()
                )));
            }
        }
        let marker = directory.join(JAVA_OWNER_FILE);
        let metadata = std::fs::symlink_metadata(&marker).map_err(|_| {
            SoftwareError::invalid(format!(
                "Java installation ownership is missing or invalid; inspect {}",
                directory.display()
            ))
        })?;
        if !metadata.is_file() {
            return Err(SoftwareError::invalid(format!(
                "Java installation ownership is missing or invalid; inspect {}",
                directory.display()
            )));
        }
        let bytes = std::fs::read(&marker)?;
        if bytes != self.java_owner(package).as_bytes() {
            return Err(SoftwareError::invalid(format!(
                "Java installation belongs to another installation; inspect {}",
                directory.display()
            )));
        }
        Ok(())
    }

    fn next_java_folder(&self, package: &Package) -> Result<String, SoftwareError> {
        let base = if package.java_folder.is_empty() {
            java_folder_base(package)
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

    fn node_global_prefix(&self, version: &str) -> PathBuf {
        self.store()
            .root()
            .join("node")
            .join("global")
            .join(version)
    }

    fn link_node_default(&self, state: &State) -> Result<(), SoftwareError> {
        let home = self.store().root().join("node");
        create_private_dir_all(&home)?;
        let link = home.join("default");
        match std::fs::remove_file(&link) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(package) = state.packages.get(&state.node_default) {
            std::os::unix::fs::symlink(self.store().package_directory(package), link)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct PublishFlags {
    swapped: bool,
    renamed: bool,
    index_moved: bool,
    index_written: bool,
}

fn rollback(
    flags: &PublishFlags,
    stage: &Path,
    dest: &Path,
    index: &Path,
    old_index: &Path,
) -> Result<(), SoftwareError> {
    if flags.swapped {
        exchange_package_directories(stage, dest).map_err(|error| {
            SoftwareError::invalid(format!(
                "repair rollback needs inspection; old files retained at {}: {error}",
                stage.display()
            ))
        })?;
    } else if flags.renamed {
        std::fs::rename(dest, stage).map_err(|error| {
            SoftwareError::invalid(format!(
                "repair rollback needs inspection; old files retained at {}: {error}",
                stage.display()
            ))
        })?;
    }
    if flags.index_written {
        match std::fs::remove_file(index) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if flags.index_moved {
        std::fs::rename(old_index, index)?;
    }
    std::fs::remove_dir_all(stage)?;
    Ok(())
}

fn remove_all_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn value_references(value: &plist::Value, path: &Path) -> bool {
    let text = path.to_string_lossy();
    match value {
        plist::Value::String(string) => string.contains(text.as_ref()),
        plist::Value::Array(items) => items.iter().any(|item| value_references(item, path)),
        plist::Value::Dictionary(dictionary) => {
            dictionary.values().any(|item| value_references(item, path))
        }
        _ => false,
    }
}

fn validate_java_bundle(root: &Path, package: &Package) -> Result<(), SoftwareError> {
    let layout = package.binaries.get("java").map(String::as_str);
    match layout {
        Some("Contents/Home/bin/java") => {
            let java = root.join("Contents/Home/bin/java");
            if !java.is_file() {
                return Err(SoftwareError::invalid("unsupported Java archive layout"));
            }
            let release = root.join("Contents/Home/release");
            let metadata = read_java_metadata(&release)?;
            if !metadata.contains("JAVA_VERSION") {
                return Err(SoftwareError::invalid(
                    "JDK archive must contain valid release metadata",
                ));
            }
        }
        Some("bin/java") => {
            let metadata = read_java_metadata(&root.join("release"))?;
            if !metadata.contains("JAVA_VERSION") {
                return Err(SoftwareError::invalid(
                    "JDK archive must contain valid release metadata",
                ));
            }
        }
        _ => return Err(SoftwareError::invalid("unsupported Java archive layout")),
    }
    Ok(())
}

fn read_java_metadata(path: &Path) -> Result<String, SoftwareError> {
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .and_then(|file| {
            if !file.metadata()?.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "JDK metadata is not a regular file",
                ));
            }
            file.take((1 << 20) + 1).read_to_end(&mut bytes)
        })?;
    if bytes.len() > 1 << 20 {
        return Err(SoftwareError::invalid("JDK metadata is too large"));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn java_installation_id(path: &Path) -> String {
    let digest = crate::fs::digest_bytes(crate::fs::clean_path(path).to_string_lossy().as_bytes());
    format!("java-local-{digest}")
}

pub(crate) fn software_to_state(error: SoftwareError) -> StateError {
    match error {
        SoftwareError::State(error) => error,
        SoftwareError::Io(error) => StateError::Io(error),
        SoftwareError::Json(error) => StateError::Json(error),
        other => StateError::invalid(other.to_string()),
    }
}

fn cancelled_error() -> SoftwareError {
    SoftwareError::State(StateError::Cancelled)
}
