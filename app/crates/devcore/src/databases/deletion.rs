//! Deletion staging and preview, ported from
//! `instance_deletion.go:144-230` and the staging half of
//! `DeleteInstanceData`.
//!
//! Permanent erasure applies to internal data only. The preview refuses an
//! external `DataDirectory` outright (`CanDelete = false`) and the operation
//! never stages it. Internal data is moved to `deletions/<random>/data` with
//! a `rename` inside the state transaction, then erased entry by entry after
//! the registration is durably gone, re-checking the directory identity at
//! every step.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fs::{
    PRIVATE_DIR, PRIVATE_FILE, atomic_write, clean_path, read_regular_file, sync_directory,
};
use crate::model::{Instance, State};
use crate::software::random_id;
use crate::state::{ResourceKey, Tx};

use super::locations::{database_data_directory, deletion_identity};
use super::{Cancelled, DatabaseError, Databases};

const MAX_PREVIEW_ENTRIES: u64 = 1_000_000;

/// A reviewed deletion.
#[derive(Debug, Clone, Serialize)]
pub struct InstanceDeletionPreview {
    #[serde(skip)]
    pub identity: String,
    #[serde(skip)]
    pub tree_digest: String,
    #[serde(rename = "InstanceID")]
    pub instance_id: String,
    #[serde(rename = "Kind")]
    pub kind: String,
    #[serde(rename = "Package")]
    pub package: String,
    #[serde(rename = "Path")]
    pub path: PathBuf,
    #[serde(rename = "Fingerprint")]
    pub fingerprint: String,
    #[serde(rename = "Detail")]
    pub detail: String,
    #[serde(rename = "Entries")]
    pub entries: i64,
    #[serde(rename = "Bytes")]
    pub bytes: i64,
    #[serde(rename = "CanDelete")]
    pub can_delete: bool,
}

/// The staged deletion record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceDeletionRecord {
    #[serde(rename = "Committed")]
    pub committed: bool,
    #[serde(rename = "Schema")]
    pub schema: i64,
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Instance")]
    pub instance: Instance,
    #[serde(rename = "Identity")]
    pub identity: String,
    #[serde(rename = "CreatedAt")]
    pub created_at: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DeletionTree {
    pub identity: String,
    pub digest: String,
    pub entries: i64,
    pub bytes: i64,
    pub paths: Vec<String>,
}

#[derive(Serialize)]
struct TreeEntry<'a> {
    #[serde(rename = "Path")]
    path: &'a str,
    #[serde(rename = "Link")]
    link: String,
    #[serde(rename = "Mode")]
    mode: u32,
    #[serde(rename = "Size")]
    size: i64,
    #[serde(rename = "Modified")]
    modified: i64,
    #[serde(rename = "Inode")]
    inode: u64,
}

#[derive(Serialize)]
struct PreviewFingerprint<'a> {
    #[serde(rename = "Root")]
    root: String,
    #[serde(rename = "Instance")]
    instance: &'a Instance,
    #[serde(rename = "Preview")]
    preview: &'a InstanceDeletionPreview,
    #[serde(rename = "Tree")]
    tree: String,
}

/// Inspect a data tree without following links. Every entry must belong to the
/// current user and the root's device, and no special files are allowed.
pub(crate) fn scan_deletion_tree(
    path: &Path,
    cancelled: Cancelled<'_>,
) -> Result<DeletionTree, DatabaseError> {
    let info = fs::symlink_metadata(path)?;
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(DatabaseError::invalid(
            "database data must be a plain directory",
        ));
    }
    let mut tree = DeletionTree {
        identity: deletion_identity(&info)?,
        ..DeletionTree::default()
    };
    let opened = fs::symlink_metadata(path)?;
    let device = opened.dev();
    let uid = unsafe { libc::getuid() } as u32;
    let mut hasher = Sha256::new();
    walk_tree(path, ".", device, uid, &mut tree, &mut hasher, cancelled)?;
    tree.digest = crate::fs::encode_hex(&hasher.finalize());
    Ok(tree)
}

fn walk_tree(
    root: &Path,
    relative: &str,
    device: u64,
    uid: u32,
    tree: &mut DeletionTree,
    hasher: &mut Sha256,
    cancelled: Cancelled<'_>,
) -> Result<(), DatabaseError> {
    if cancelled() {
        return Err(DatabaseError::Cancelled);
    }
    let absolute = if relative == "." {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    let info = fs::symlink_metadata(&absolute)?;
    if info.dev() != device || info.uid() != uid {
        return Err(DatabaseError::invalid(
            "data contains a foreign owner or mounted filesystem; inspect it before deletion",
        ));
    }
    let link = if info.file_type().is_symlink() {
        fs::read_link(&absolute)?.to_string_lossy().into_owned()
    } else {
        String::new()
    };
    if !info.is_dir() && !info.is_file() && !info.file_type().is_symlink() {
        return Err(DatabaseError::invalid(
            "data contains a special file; inspect it before deletion",
        ));
    }
    let entry = TreeEntry {
        path: relative,
        link,
        mode: info.mode(),
        size: info.size() as i64,
        modified: info.mtime() as i64 * 1_000_000_000 + info.mtime_nsec(),
        inode: info.ino(),
    };
    hasher.update(serde_json::to_vec(&entry)?);
    hasher.update(b"\n");
    tree.entries += 1;
    if info.is_file() {
        tree.bytes += info.size() as i64;
    }
    if tree.entries > MAX_PREVIEW_ENTRIES as i64 {
        return Err(DatabaseError::invalid(
            "data tree exceeds the safe preview limit",
        ));
    }
    tree.paths.push(relative.to_string());
    if info.is_dir() {
        let mut names = Vec::new();
        for entry in fs::read_dir(&absolute)? {
            names.push(entry?.file_name());
        }
        names.sort();
        for name in names {
            let child = if relative == "." {
                name.to_string_lossy().into_owned()
            } else {
                format!("{relative}/{}", name.to_string_lossy())
            };
            walk_tree(root, &child, device, uid, tree, hasher, cancelled)?;
        }
    }
    Ok(())
}

impl Databases {
    /// `PreviewInstanceDeletion`: bind the tree identity and digest to the
    /// exact instance record so a later delete cannot race a change.
    pub fn preview_instance_deletion(
        &self,
        id: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<InstanceDeletionPreview, DatabaseError> {
        let state = self.store().read()?;
        self.instance_deletion_preview(&state, id, cancelled)
    }

    pub(crate) fn instance_deletion_preview(
        &self,
        state: &State,
        id: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<InstanceDeletionPreview, DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        self.validate_deletion_store()?;
        let instance = state
            .instances
            .get(id)
            .cloned()
            .ok_or_else(|| DatabaseError::invalid("database instance not found"))?;
        let mut preview = InstanceDeletionPreview {
            identity: String::new(),
            tree_digest: String::new(),
            instance_id: id.to_string(),
            kind: instance.kind.clone(),
            package: instance.package.clone(),
            path: self.store().root().join("instances").join(id),
            fingerprint: String::new(),
            detail: "Permanently removes this instance record, data, local configuration and credentials. Installed software and backups outside this path are retained.".to_string(),
            entries: 0,
            bytes: 0,
            can_delete: true,
        };
        if !instance.data_directory.is_empty() {
            preview.can_delete = false;
            preview.path = database_data_directory(self.store().root(), &instance, None);
            preview.detail = "External data is retained. Remove the instance registration to keep this data location; permanent erasure is not supported for external data.".to_string();
            return Ok(preview);
        }
        match self.job_loaded(&format!("db-{id}"), cancelled) {
            Ok(false) => {}
            Ok(true) => {
                preview.can_delete = false;
                preview.detail = "Stop this instance before deleting its data.".to_string();
            }
            Err(_) => {
                preview.can_delete = false;
                preview.detail = "Cannot confirm that the database job is stopped. Refresh after checking its status.".to_string();
            }
        }
        let tree = scan_deletion_tree(&preview.path, cancelled)?;
        preview.entries = tree.entries;
        preview.bytes = tree.bytes;
        preview.identity = tree.identity.clone();
        preview.tree_digest = tree.digest.clone();
        let bytes = serde_json::to_vec(&PreviewFingerprint {
            root: clean_path(self.store().root()).display().to_string(),
            instance: &instance,
            preview: &preview,
            tree: format!("{}:{}", tree.digest, tree.identity),
        })?;
        preview.fingerprint = crate::fs::digest_bytes(&bytes);
        Ok(preview)
    }

    /// `DeleteInstanceData`: confirmed permanent erasure of internal data.
    ///
    /// The typed instance ID is required, the reviewed fingerprint must still
    /// match, and the staged rename happens inside the state transaction.
    /// External data never reaches this path.
    pub fn delete_instance_data(
        &self,
        id: &str,
        fingerprint: &str,
        confirmation: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if confirmation != id {
            return Err(DatabaseError::invalid(
                "type the exact instance ID to confirm permanent data deletion",
            ));
        }
        self.init()?;
        let _lease = self
            .store()
            .resource_leases_cancellable(&[ResourceKey::new("database instance", id)], cancelled)?;
        let deletion_id = random_id();
        let deletion_dir = self.store().root().join("deletions").join(&deletion_id);
        fs::DirBuilder::new()
            .mode(PRIVATE_DIR)
            .create(&deletion_dir)?;
        let owner = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(PRIVATE_FILE)
            .custom_flags(libc::O_NOFOLLOW)
            .open(deletion_dir.join("owner.lock"))?;
        let locked = unsafe {
            libc::flock(
                std::os::unix::io::AsRawFd::as_raw_fd(&owner),
                libc::LOCK_EX | libc::LOCK_NB,
            )
        };
        if locked != 0 {
            return Err(DatabaseError::Io(std::io::Error::last_os_error()));
        }
        let record_path = deletion_dir.join("record.json");
        let staged = deletion_dir.join("data");
        let commit_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let approved: Arc<Mutex<Option<DeletionTree>>> = Arc::new(Mutex::new(None));

        let commit_error_hook = commit_error.clone();
        let approved_hook = approved.clone();
        let record_for_commit = record_path.clone();
        let store = self.store().clone();
        self.store()
            .with_state_transaction_cancellable(cancelled, |state| {
                let preview = self.instance_deletion_preview(state, id, cancelled)?;
                if preview.fingerprint != fingerprint {
                    return Err(crate::state::StateError::invalid(
                        "database or data changed; refresh the deletion preview",
                    ));
                }
                if !preview.can_delete {
                    return Err(crate::state::StateError::invalid(preview.detail));
                }
                let tree = scan_deletion_tree(&preview.path, cancelled)
                    .map_err(crate::state::StateError::from)?;
                if tree.identity != preview.identity || tree.digest != preview.tree_digest {
                    return Err(crate::state::StateError::invalid(
                        "database data changed after review; refresh before deleting",
                    ));
                }
                let instance = state.instances[id].clone();
                let record = InstanceDeletionRecord {
                    committed: false,
                    schema: 1,
                    id: deletion_id.clone(),
                    instance: instance.clone(),
                    identity: tree.identity.clone(),
                    created_at: crate::software::utc_now_rfc3339(),
                };
                let mut bytes = serde_json::to_vec_pretty(&record)?;
                bytes.push(b'\n');
                atomic_write(&record_path, &bytes, PRIVATE_FILE)?;
                sync_directory(&deletion_dir)?;
                sync_directory(&self.store().root().join("deletions"))?;
                let from = self.store().root().join("instances").join(id);
                if std::fs::rename(&from, &staged).is_err() {
                    return Err(crate::state::StateError::invalid(
                        "could not stage database data for deletion",
                    ));
                }
                let undo_from = from.clone();
                let undo_to = staged.clone();
                let undo = Box::new(move || -> Result<(), crate::state::StateError> {
                    if fs::symlink_metadata(&undo_from).is_ok() {
                        return Err(crate::state::StateError::invalid(
                            "cannot restore deletion data over an existing path",
                        ));
                    }
                    std::fs::rename(&undo_to, &undo_from)?;
                    Ok(())
                });
                sync_directory(&self.store().root().join("instances"))?;
                sync_directory(&deletion_dir)?;
                self.store().emit("Database data staged for deletion");
                state.instances.remove(id);
                super::lifecycle::remove_instance_profiles(state, id);
                *approved_hook.lock().unwrap() = Some(tree);
                let record_for_commit = record_for_commit.clone();
                let commit_error_hook = commit_error_hook.clone();
                Ok(Tx::new(()).with_undo(undo).with_commit(move || {
                    if let Err(error) = sync_directory(store.root()) {
                        *commit_error_hook.lock().unwrap() = Some(error.to_string());
                        return;
                    }
                    match read_deletion_record(&record_for_commit) {
                        Ok(mut record) => {
                            record.committed = true;
                            let write = (|| -> std::io::Result<()> {
                                let mut bytes = serde_json::to_vec_pretty(&record)
                                    .map_err(std::io::Error::other)?;
                                bytes.push(b'\n');
                                atomic_write(&record_for_commit, &bytes, PRIVATE_FILE)?;
                                sync_directory(
                                    record_for_commit.parent().unwrap_or_else(|| Path::new(".")),
                                )
                            })();
                            if let Err(error) = write {
                                *commit_error_hook.lock().unwrap() = Some(error.to_string());
                            } else {
                                store.emit("Database deletion committed");
                            }
                        }
                        Err(error) => {
                            *commit_error_hook.lock().unwrap() = Some(error.to_string());
                        }
                    }
                }))
            })?;
        if let Some(error) = commit_error.lock().unwrap().clone() {
            return Err(DatabaseError::invalid(error));
        }
        let tree = approved
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| DatabaseError::invalid("deletion was not staged"))?;
        self.erase_instance_deletion(&record_path, &staged, &tree, cancelled)?;
        Ok(())
    }

    fn erase_instance_deletion(
        &self,
        record_path: &Path,
        staged: &Path,
        approved: &DeletionTree,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        let record = read_deletion_record(record_path)?;
        let state = self.store().read()?;
        if state.instances.contains_key(&record.instance.id) {
            return Err(DatabaseError::invalid(
                "instance removal is not committed; keep the staged data",
            ));
        }
        match self.job_loaded(&format!("db-{}", record.instance.id), cancelled) {
            Ok(false) => {}
            _ => {
                return Err(DatabaseError::invalid(
                    "a database job is still registered; stop it before erasing staged data",
                ));
            }
        }
        let plist = self
            .engine()
            .job_path(&format!("db-{}", record.instance.id));
        match fs::remove_file(&plist) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(DatabaseError::Io(error)),
        }
        let tree = scan_deletion_tree(staged, cancelled)?;
        if tree.identity != record.identity || tree.digest != approved.digest {
            return Err(DatabaseError::invalid(
                "staged directory changed; refresh before erasing",
            ));
        }
        self.store().emit("Deleting staged database data");
        for relative in tree.paths.iter().rev() {
            if relative == "." {
                continue;
            }
            if cancelled() {
                return Err(DatabaseError::Cancelled);
            }
            let absolute = staged.join(relative);
            let info = fs::symlink_metadata(&absolute)?;
            if info.is_dir() && !info.file_type().is_symlink() {
                fs::remove_dir(&absolute)?;
            } else {
                fs::remove_file(&absolute)?;
            }
        }
        let info = fs::symlink_metadata(staged)?;
        if deletion_identity(&info)? != record.identity {
            return Err(DatabaseError::invalid(
                "staged directory changed during erasure",
            ));
        }
        fs::remove_dir(staged)?;
        fs::remove_file(record_path)?;
        sync_directory(record_path.parent().unwrap_or_else(|| Path::new(".")))?;
        Ok(())
    }

    fn validate_deletion_store(&self) -> Result<(), DatabaseError> {
        if !self.store().root().is_absolute() {
            return Err(DatabaseError::invalid("storage root must be absolute"));
        }
        for name in ["instances", "deletions", "jobs"] {
            let path = self.store().root().join(name);
            match fs::symlink_metadata(&path) {
                Ok(info) if info.is_dir() && !info.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(DatabaseError::invalid(
                        "deletion storage is not a plain directory",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(DatabaseError::Io(error)),
            }
        }
        Ok(())
    }

    pub(crate) fn job_loaded(
        &self,
        job: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<bool, DatabaseError> {
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        match self.engine().job_state(job) {
            Ok(Some(state)) => Ok(state.loaded || state.running),
            Ok(None) => Ok(false),
            Err(error) => Err(DatabaseError::invalid(format!(
                "could not verify whether the package is in use: {error}"
            ))),
        }
    }
}

fn read_deletion_record(path: &Path) -> Result<InstanceDeletionRecord, DatabaseError> {
    let bytes = read_regular_file(
        path.parent().unwrap_or_else(|| Path::new(".")),
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
        65536,
    )?;
    let record: InstanceDeletionRecord = serde_json::from_slice(&bytes)?;
    if record.schema != 1
        || !crate::model::valid_id(&record.instance.id)
        || record.identity.is_empty()
    {
        return Err(DatabaseError::invalid("deletion record identity mismatch"));
    }
    Ok(record)
}

/// Clean up a partially staged deletion directory left by a crash. Not used by
/// the normal path, but keeps the store honest after an interrupted delete.
pub fn pending_deletion_paths(root: &Path) -> Result<Vec<PathBuf>, DatabaseError> {
    let dir = root.join("deletions");
    let mut out = Vec::new();
    match fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries {
                out.push(entry?.path());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(DatabaseError::Io(error)),
    }
    out.sort();
    Ok(out)
}

/// A staged or uncommitted deletion discovered at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct InstanceDeletionInfo {
    #[serde(skip)]
    pub tree_digest: String,
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "InstanceID")]
    pub instance_id: String,
    #[serde(rename = "Kind")]
    pub kind: String,
    #[serde(rename = "OriginalPath")]
    pub original_path: String,
    #[serde(rename = "StagedPath")]
    pub staged_path: String,
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "Detail")]
    pub detail: String,
    #[serde(rename = "Revision")]
    pub revision: String,
    #[serde(rename = "Entries")]
    pub entries: i64,
    #[serde(rename = "Bytes")]
    pub bytes: i64,
    #[serde(rename = "CanErase")]
    pub can_erase: bool,
    #[serde(rename = "CanRestore")]
    pub can_restore: bool,
    #[serde(rename = "Busy")]
    pub busy: bool,
}

fn deletion_paths(root: &Path, id: &str) -> Result<(PathBuf, PathBuf, PathBuf), DatabaseError> {
    if !crate::model::valid_id(id) {
        return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
    }
    if !root.is_absolute() {
        return Err(DatabaseError::invalid("storage root must be absolute"));
    }
    let dir = root.join("deletions").join(id);
    Ok((dir.clone(), dir.join("record.json"), dir.join("data")))
}

fn read_instance_deletion(
    path: &Path,
    id: &str,
) -> Result<(InstanceDeletionRecord, Vec<u8>), DatabaseError> {
    let bytes = read_regular_file(
        path.parent().unwrap_or_else(|| Path::new(".")),
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
        65536,
    )?;
    let record: InstanceDeletionRecord = serde_json::from_slice(&bytes)?;
    if record.schema != 1
        || record.id != id
        || !crate::model::valid_id(&record.instance.id)
        || record.identity.is_empty()
    {
        return Err(DatabaseError::invalid("deletion record identity mismatch"));
    }
    Ok((record, bytes))
}

fn open_deletion_lease(dir: &Path) -> Result<fs::File, DatabaseError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(false)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join("owner.lock"))?;
    if !file.metadata()?.is_file() {
        return Err(DatabaseError::invalid(
            "deletion owner lease is not a regular file",
        ));
    }
    let result = unsafe {
        libc::flock(
            std::os::unix::io::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    if result == 0 {
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Err(DatabaseError::invalid(
            "database deletion is still owned by another caller",
        ));
    }
    Err(error.into())
}

fn deletion_info(
    databases: &Databases,
    root: &Path,
    state: &State,
    record: &InstanceDeletionRecord,
    record_bytes: &[u8],
    staged: &Path,
) -> Result<InstanceDeletionInfo, DatabaseError> {
    let original_path = root.join("instances").join(&record.instance.id);
    let mut info = InstanceDeletionInfo {
        id: record.id.clone(),
        instance_id: record.instance.id.clone(),
        kind: record.instance.kind.clone(),
        original_path: original_path.display().to_string(),
        staged_path: staged.display().to_string(),
        ..InstanceDeletionInfo::default()
    };
    let original = match fs::symlink_metadata(&original_path) {
        Ok(info) => Some(info),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let original_exists = original.is_some();
    let current = state.instances.get(&record.instance.id);
    let registered = current.is_some();
    let tree = match scan_deletion_tree(staged, &|| false) {
        Ok(tree) => Some(tree),
        Err(DatabaseError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    match tree {
        None => {
            if original_exists && registered && current == Some(&record.instance) {
                let identity = super::deletion_identity(original.as_ref().unwrap())?;
                if identity != record.identity {
                    return Err(DatabaseError::invalid("original data identity changed"));
                }
                info.can_restore = true;
                info.status = "Not moved".into();
                info.detail = "Deletion did not commit. The registered database and original data are retained.".into();
            } else if !registered {
                info.status = "Finishing".into();
                info.can_erase = true;
                info.detail = "Staged data is gone. Finish to clear this deletion record.".into();
            } else {
                return Err(DatabaseError::invalid(
                    "data is missing but its instance is still registered; restore from backup before further changes",
                ));
            }
        }
        Some(tree) => {
            if tree.identity != record.identity {
                return Err(DatabaseError::invalid(
                    "staged data identity differs from the confirmed directory",
                ));
            }
            info.entries = tree.entries;
            info.bytes = tree.bytes;
            if registered && !record.committed {
                if original_exists || current != Some(&record.instance) {
                    info.status = "Restore blocked".into();
                    info.detail = "Instance removal did not commit, but the original path or instance changed. Keep the staged data; resolve the original path before restoring.".into();
                } else {
                    let loaded =
                        databases.job_loaded(&format!("db-{}", record.instance.id), &|| false);
                    match loaded {
                        Ok(false) => {
                            info.status = "Restore required".into();
                            info.can_restore = true;
                            info.detail = "Data was moved but instance removal did not commit. Restore the data folder; do not erase it.".into();
                        }
                        _ => {
                            info.status = "Stop required".into();
                            info.detail = "Confirm the original database job is stopped before restoring its data folder.".into();
                        }
                    }
                }
            } else if !registered || original_exists {
                if let Some(original) = &original
                    && let Ok(identity) = super::deletion_identity(original)
                    && identity == record.identity
                {
                    return Err(DatabaseError::invalid(
                        "original and staged paths unexpectedly refer to the same data",
                    ));
                }
                info.status = "Ready to erase".into();
                info.can_erase = true;
                info.detail = "The original instance removal committed. Finish permanently erases only this staged data. Other instances and backups are retained.".into();
            } else {
                info.status = "Restore blocked".into();
                info.detail = "A registered instance has no original data path. Keep the staged data and inspect the current instance before continuing.".into();
            }
            let bytes = serde_json::to_vec(&serde_json::json!({
                "Record": crate::fs::digest_bytes(record_bytes),
                "Info": info,
                "Tree": tree.digest,
                "Registered": registered,
                "Current": current,
            }))?;
            info.revision = crate::fs::digest_bytes(&bytes);
            info.tree_digest = tree.digest;
        }
    }
    Ok(info)
}

impl Databases {
    /// `PendingInstanceDeletions`: every staged deletion and its recoverability.
    pub fn pending_instance_deletions(
        &self,
        cancelled: Cancelled<'_>,
    ) -> Result<Vec<InstanceDeletionInfo>, DatabaseError> {
        self.validate_deletion_store()?;
        let root = self.store().root();
        let entries = match fs::read_dir(root.join("deletions")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let state = self.store().read()?;
        let mut out = Vec::new();
        for entry in entries {
            if cancelled() {
                return Err(DatabaseError::Cancelled);
            }
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let (dir, path, staged) = deletion_paths(root, &id)?;
            let info = fs::symlink_metadata(&dir)?;
            if !info.is_dir() || info.file_type().is_symlink() {
                return Err(DatabaseError::invalid(
                    "deletion storage is not a plain directory",
                ));
            }
            let (record, bytes) = match read_instance_deletion(&path, &id) {
                Ok(value) => value,
                Err(DatabaseError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(error) => return Err(error),
            };
            let mut info = deletion_info(self, root, &state, &record, &bytes, &staged)?;
            match open_deletion_lease(&dir) {
                Ok(_) => {}
                Err(DatabaseError::Invalid(message))
                    if message.contains("owned by another caller") =>
                {
                    info.busy = true;
                    info.can_erase = false;
                    info.can_restore = false;
                }
                Err(error) => return Err(error),
            }
            out.push(info);
        }
        Ok(out)
    }

    /// `RestoreInstanceDeletion`: move staged data back when removal did not
    /// commit, or clear a "Not moved" record.
    pub fn restore_instance_deletion(
        &self,
        id: &str,
        revision: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        let root = self.store().root();
        let (dir, path, staged) = deletion_paths(root, id)?;
        let _lease = open_deletion_lease(&dir)?;
        let (record, _) = read_instance_deletion(&path, id)?;
        let _instance = self.store().resource_leases_cancellable(
            &[ResourceKey::new("database instance", &record.instance.id)],
            cancelled,
        )?;
        self.store()
            .with_state_transaction_cancellable(cancelled, |state| {
                let (record, bytes) =
                    read_instance_deletion(&path, id).map_err(crate::state::StateError::from)?;
                let info = deletion_info(self, root, state, &record, &bytes, &staged)
                    .map_err(crate::state::StateError::from)?;
                if info.revision != revision || !info.can_restore {
                    return Err(crate::state::StateError::invalid(
                        "deletion changed or cannot be restored; refresh its status",
                    ));
                }
                if info.status == "Not moved" {
                    fs::remove_file(&path)?;
                    sync_directory(&dir)?;
                    return Ok(Tx::new(()));
                }
                let target = root.join("instances").join(&record.instance.id);
                match fs::symlink_metadata(&target) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => {
                        return Err(crate::state::StateError::invalid(
                            "original instance path is occupied",
                        ));
                    }
                }
                fs::rename(&staged, &target)?;
                let undo_from = staged.clone();
                let undo_to = target.clone();
                let undo = Box::new(move || -> Result<(), crate::state::StateError> {
                    fs::rename(&undo_to, &undo_from)?;
                    Ok(())
                });
                Ok(Tx::new(()).with_undo(undo).with_commit(move || {
                    let _ = fs::remove_file(&path);
                    let _ = sync_directory(&dir);
                }))
            })?;
        Ok(())
    }

    /// `FinishInstanceDeletion`: erase only the reviewed, digest-verified
    /// staged data. The typed original ID is required.
    pub fn finish_instance_deletion(
        &self,
        id: &str,
        revision: &str,
        confirmation: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        let root = self.store().root();
        let (dir, path, staged) = deletion_paths(root, id)?;
        let _lease = open_deletion_lease(&dir)?;
        let (record, bytes) = read_instance_deletion(&path, id)?;
        if confirmation != record.instance.id {
            return Err(DatabaseError::invalid(
                "type the original instance ID to confirm permanent erasure",
            ));
        }
        let state = self.store().read()?;
        let info = deletion_info(self, root, &state, &record, &bytes, &staged)?;
        if info.revision != revision {
            return Err(DatabaseError::invalid(
                "deletion changed; refresh before continuing",
            ));
        }
        if !info.can_erase {
            return Err(DatabaseError::invalid(
                "deletion has not committed; restore its data folder first",
            ));
        }
        match scan_deletion_tree(&staged, cancelled) {
            Ok(tree) => self.erase_instance_deletion(&path, &staged, &tree, cancelled),
            Err(DatabaseError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::remove_file(&path)?;
                sync_directory(&dir)?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}
