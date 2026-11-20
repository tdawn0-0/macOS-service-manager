//! Database data locations, ported from the Go `database_locations.go`.
//!
//! An instance either owns its data inside the development home or names an
//! external `DataDirectory`. External locations are validated before creation
//! and verified before every start: the directory must be a real directory
//! (no symlink component), outside application storage, not overlapping
//! another instance, and it must carry the ownership marker written at
//! creation time. Removal never deletes this directory.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::fs::{PRIVATE_DIR, PRIVATE_FILE, clean_path, sync_directory};
use crate::model::{Instance, State};
use crate::state::Store;

use super::DatabaseError;

/// The marker proving an external location belongs to one instance.
pub const DATA_OWNER_FILE: &str = ".service-manager-owner";

/// The instance data directory in the steady-state layout.
///
/// * External `DataDirectory`: `<DataDirectory>/data`.
/// * Redis: `instances/<id>` (the engine's working directory).
/// * Everything else: `instances/<id>/data`.
///
/// During creation `staging` is the staged data directory
/// (`initializations/<random>/data`); non-Redis engines nest their payload one
/// level below it, exactly as the Go initialization context does.
pub fn database_data_directory(
    root: &Path,
    instance: &Instance,
    staging: Option<&Path>,
) -> PathBuf {
    if !instance.data_directory.is_empty() {
        return Path::new(&instance.data_directory).join("data");
    }
    if instance.kind == "redis" {
        return root.join("instances").join(&instance.id);
    }
    match staging {
        Some(dir) => dir.join("data"),
        None => root.join("instances").join(&instance.id).join("data"),
    }
}

/// The instance's configuration directory: where `valkey.conf` and
/// `client.cnf` live.
pub fn config_directory(root: &Path, instance: &Instance, staging: Option<&Path>) -> PathBuf {
    match staging {
        Some(dir) => dir.to_path_buf(),
        None => root.join("instances").join(&instance.id),
    }
}

/// `dev:ino`, the identity the deletion and initialization records bind.
pub fn deletion_identity(metadata: &fs::Metadata) -> Result<String, DatabaseError> {
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

/// Whether `path` is `root` or lies below it (lexical, like `pathWithin`).
pub fn path_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

/// Every component of `path` must be a real directory. A missing final
/// component is allowed (the location is created later); a missing ancestor
/// is not. Symlinks and non-directories are rejected.
pub fn plain_path(path: &Path) -> Result<(), DatabaseError> {
    let mut current = path.to_path_buf();
    let mut first = true;
    loop {
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(DatabaseError::invalid(
                        "data location must contain only real directories",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && first => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(error.into()),
            Err(error) => return Err(error.into()),
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
        first = false;
    }
    Ok(())
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Validate an external data location before an instance is created.
pub fn validate_data_location(
    store: &Store,
    state: &State,
    instance: &Instance,
) -> Result<(), DatabaseError> {
    if instance.data_directory.is_empty() {
        return Ok(());
    }
    let raw = Path::new(&instance.data_directory);
    let home = canonical_or_original(&store.home()?);
    let app_root = canonical_or_original(store.root());
    if !raw.is_absolute()
        || clean_path(raw) != raw
        || raw == Path::new("/")
        || raw == home
        || path_within(raw, &app_root)
        || path_within(&app_root, raw)
    {
        return Err(DatabaseError::invalid(
            "choose a new absolute data location outside application storage",
        ));
    }
    plain_path(raw)?;
    for other in state.instances.values() {
        if other.id == instance.id || other.data_directory.is_empty() {
            continue;
        }
        let other_path = Path::new(&other.data_directory);
        if path_within(raw, other_path) || path_within(other_path, raw) {
            return Err(DatabaseError::invalid(
                "data location overlaps another instance",
            ));
        }
    }
    let mut parent = raw.parent().map(Path::to_path_buf).unwrap_or_default();
    while parent != Path::new("/") && !parent.as_os_str().is_empty() {
        if fs::symlink_metadata(parent.join(DATA_OWNER_FILE)).is_ok() {
            return Err(DatabaseError::invalid(
                "data location is inside another managed location",
            ));
        }
        match parent.parent() {
            Some(next) if next != parent => parent = next.to_path_buf(),
            _ => break,
        }
    }
    match fs::symlink_metadata(raw) {
        Ok(_) => verify_data_location(store, instance),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Verify an existing external location against the registered identity and
/// ownership marker.
pub fn verify_data_location(store: &Store, instance: &Instance) -> Result<(), DatabaseError> {
    if instance.data_directory.is_empty() {
        return Ok(());
    }
    if instance.data_owner.is_empty() {
        return Err(DatabaseError::invalid(
            "existing data location cannot be adopted",
        ));
    }
    let dir = Path::new(&instance.data_directory);
    plain_path(dir)?;
    let metadata = fs::symlink_metadata(dir)?;
    if deletion_identity(&metadata)? != instance.data_identity {
        return Err(DatabaseError::invalid("data location identity changed"));
    }
    let data = dir.join("data");
    match fs::symlink_metadata(&data) {
        Ok(found) if !found.is_dir() || found.file_type().is_symlink() => {
            return Err(DatabaseError::invalid("database data path changed"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let marker = dir.join(DATA_OWNER_FILE);
    let info = fs::symlink_metadata(&marker)?;
    if !info.is_file() {
        return Err(DatabaseError::invalid("data ownership marker changed"));
    }
    let bytes = fs::read(&marker)?;
    let expected = format!(
        "{}\n{}\n{}\n",
        store.root().display(),
        instance.id,
        instance.data_owner
    );
    if bytes != expected.as_bytes() {
        return Err(DatabaseError::invalid("data location ownership changed"));
    }
    Ok(())
}

/// Create the external location and write its ownership marker.
pub fn prepare_data_location(store: &Store, instance: &mut Instance) -> Result<(), DatabaseError> {
    if instance.data_directory.is_empty() {
        return Ok(());
    }
    let dir = Path::new(&instance.data_directory).to_path_buf();
    plain_path(&dir)?;
    fs::DirBuilder::new().mode(PRIVATE_DIR).create(&dir)?;
    let metadata = fs::symlink_metadata(&dir)?;
    instance.data_identity = deletion_identity(&metadata)?;
    let marker = dir.join(DATA_OWNER_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&marker)?;
    let result = file
        .write_all(
            format!(
                "{}\n{}\n{}\n",
                store.root().display(),
                instance.id,
                instance.data_owner
            )
            .as_bytes(),
        )
        .and_then(|()| file.sync_all());
    drop(file);
    result?;
    sync_directory(&dir)?;
    Ok(())
}
