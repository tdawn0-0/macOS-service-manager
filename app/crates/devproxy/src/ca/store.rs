//! The generational authority store: `proxy/ca-generations/<id>/`, the three
//! pointer symlinks, the public aliases, and `ca.lock`.
//!
//! Ported from the storage half of `authority.go`. All pointer updates are
//! symlink-plus-rename so a reader never observes a half-written selection.

use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use devcore::fs::{
    PRIVATE_DIR, PRIVATE_FILE, atomic_write, clean_path, create_private_dir_all, sync_directory,
};

use super::CaError;
use super::material::{
    AuthorityMaterial, AuthorityRecord, new_authority, pair_digest, parse_authority,
};

pub const CURRENT_POINTER: &str = "ca-current";
pub const PREPARED_POINTER: &str = "ca-prepared";
pub const PREVIOUS_POINTER: &str = "ca-previous";
pub const ALIASES: [&str; 2] = ["ca.pem", "ca-key.pem"];

const LOCK_RETRY: Duration = Duration::from_millis(20);
const GENERATION_FILE_LIMIT: i64 = 1 << 20;

/// A handle to one workspace's authority storage.
#[derive(Debug, Clone)]
pub struct AuthorityStore {
    root: PathBuf,
}

impl AuthorityStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        AuthorityStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn authority_root(&self) -> PathBuf {
        self.root.join("proxy")
    }

    pub fn generations_dir(&self) -> PathBuf {
        self.authority_root().join("ca-generations")
    }

    pub fn generation_dir(&self, id: &str) -> PathBuf {
        self.generations_dir().join(id)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.authority_root().join("ca.lock")
    }

    /// `ca-generations` must be a real directory, never a link.
    pub fn validate_store(&self) -> Result<(), CaError> {
        let path = self.generations_dir();
        match fs::symlink_metadata(&path) {
            Ok(info) => {
                if !info.is_dir() || info.file_type().is_symlink() {
                    return Err(CaError::Invalid(
                        "managed authority store must be a directory, not a link or special file"
                            .into(),
                    ));
                }
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Read a pointer symlink and validate that it stays inside the store.
    ///
    /// A dangling pointer still returns its id; the generation read reports
    /// the missing files.
    pub fn read_pointer(&self, name: &str) -> Result<Option<String>, CaError> {
        let path = self.authority_root().join(name);
        let info = match fs::symlink_metadata(&path) {
            Ok(info) => info,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !info.file_type().is_symlink() {
            return Err(CaError::Invalid(format!(
                "{name} must be a managed authority link"
            )));
        }
        let target = fs::read_link(&path)?;
        let id = target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if !valid_id(&id) || target != Path::new("ca-generations").join(&id) {
            return Err(CaError::Invalid(format!(
                "{name} points outside the managed authority store"
            )));
        }
        self.validate_store()?;
        match fs::symlink_metadata(self.generation_dir(&id)) {
            Ok(info) => {
                if !info.is_dir() || info.file_type().is_symlink() {
                    return Err(CaError::Invalid(
                        "authority generation must be a directory".into(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(Some(id))
    }

    /// Read and validate one generation. A missing directory yields an
    /// `Invalid` material rather than an error, matching the Go inspector.
    pub fn read_generation(&self, id: &str) -> Result<AuthorityMaterial, CaError> {
        if !valid_id(id) {
            return Err(CaError::Invalid(format!("invalid identifier {id:?}")));
        }
        self.validate_store()?;
        let dir = self.generation_dir(id);
        let certificate_path = dir.join("ca.pem");
        let info = match fs::symlink_metadata(&dir) {
            Ok(info) => info,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut material =
                    parse_authority(b"", b"", id, certificate_path, Utc::now().timestamp());
                material.info.status = "Invalid".into();
                material.info.detail =
                    "Stored authority files are missing. Choose a saved authority or prepare a replacement."
                        .into();
                material.digest = devcore::fs::digest_bytes(format!("missing:{id}").as_bytes());
                return Ok(material);
            }
            Err(error) => return Err(error.into()),
        };
        if !info.is_dir() || info.file_type().is_symlink() {
            return Err(CaError::Invalid(
                "authority generation must be a directory".into(),
            ));
        }
        let cert = read_optional_regular(&dir, "ca.pem", GENERATION_FILE_LIMIT)?;
        let key = read_optional_regular(&dir, "ca-key.pem", GENERATION_FILE_LIMIT)?;
        let mut material =
            parse_authority(&cert, &key, id, certificate_path, Utc::now().timestamp());
        let metadata_ok = read_optional_regular(&dir, "metadata.json", GENERATION_FILE_LIMIT)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AuthorityRecord>(&bytes).ok())
            .filter(|record| {
                record.schema == 1 && record.id == id && record.pair_digest == material.digest
            });
        match metadata_ok {
            Some(record) => material.record = record,
            None => {
                material.info.status = "Invalid".into();
                material.info.usable = false;
                material.info.detail =
                    "Authority material or its generation record changed. Prepare a replacement."
                        .into();
            }
        }
        Ok(material)
    }

    /// Read the selected authority, including the legacy pre-generational
    /// layout where `proxy/ca.pem` and `proxy/ca-key.pem` are real files.
    pub fn read_active(&self) -> Result<AuthorityMaterial, CaError> {
        if let Some(id) = self.read_pointer(CURRENT_POINTER)? {
            return self.read_generation(&id);
        }
        let authority_root = self.authority_root();
        let read = |name: &str| -> Result<Vec<u8>, CaError> {
            let path = authority_root.join(name);
            if let Ok(target) = fs::read_link(&path)
                && target == Path::new(CURRENT_POINTER).join(name)
            {
                return Ok(Vec::new());
            }
            read_optional_regular(&authority_root, name, GENERATION_FILE_LIMIT)
        };
        let cert = read("ca.pem")?;
        let key = read("ca-key.pem")?;
        Ok(parse_authority(
            &cert,
            &key,
            "",
            authority_root.join("ca.pem"),
            Utc::now().timestamp(),
        ))
    }

    /// Both public aliases must be links to `ca-current/<name>`.
    pub fn aliases_valid(&self) -> Result<(), CaError> {
        for name in ALIASES {
            let target = fs::read_link(self.authority_root().join(name)).map_err(|_| {
                CaError::Invalid(
                    "public CA files do not match the managed authority links; repair or complete initial setup"
                        .into(),
                )
            })?;
            if target != Path::new(CURRENT_POINTER).join(name) {
                return Err(CaError::Invalid(
                    "public CA files do not match the managed authority links; repair or complete initial setup"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    /// Take the exclusive authority lock, retrying every 20 ms. `cancelled` is
    /// checked before each attempt.
    pub fn with_lock<R>(
        &self,
        cancelled: &dyn Fn() -> bool,
        f: impl FnOnce() -> Result<R, CaError>,
    ) -> Result<R, CaError> {
        create_private_dir_all(&self.authority_root())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(PRIVATE_FILE)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.lock_path())?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(CaError::Invalid("invalid authority lock".into()));
        }
        let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
        loop {
            if cancelled() {
                return Err(CaError::Cancelled);
            }
            // SAFETY: `fd` is open for the duration of this call.
            let code = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if code == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(error.into());
            }
            std::thread::sleep(LOCK_RETRY);
        }
        let result = f();
        // SAFETY: same descriptor, still open.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        result
    }

    /// Atomically point `name` at `ca-generations/<id>`.
    pub fn write_pointer(&self, name: &str, id: &str) -> Result<(), CaError> {
        if !valid_id(id) {
            return Err(CaError::Invalid("invalid authority ID".into()));
        }
        let path = self.authority_root().join(name);
        match fs::symlink_metadata(&path) {
            Ok(info) if !info.file_type().is_symlink() => {
                return Err(CaError::Invalid(
                    "refusing to replace an unmanaged authority pointer".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let tmp = self
            .authority_root()
            .join(format!(".ca-link-{}", random_id()));
        let target = Path::new("ca-generations").join(id);
        if let Err(error) = symlink(&target, &tmp) {
            let _ = fs::remove_file(&tmp);
            return Err(error.into());
        }
        if let Err(error) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(error.into());
        }
        sync_directory(&self.authority_root())?;
        Ok(())
    }

    /// Ensure `ca.pem` and `ca-key.pem` are links to the active generation.
    pub fn ensure_aliases(&self, replace: bool) -> Result<(), CaError> {
        for name in ALIASES {
            let path = self.authority_root().join(name);
            let target = Path::new(CURRENT_POINTER).join(name);
            let info = match fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    symlink(&target, &path)?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            if info.file_type().is_symlink()
                && fs::read_link(&path).ok().as_deref() == Some(target.as_path())
            {
                continue;
            }
            if !replace || (!info.is_file() && !info.file_type().is_symlink()) {
                return Err(CaError::Invalid(
                    "CA export path is not a replaceable managed file".into(),
                ));
            }
            let tmp = self
                .authority_root()
                .join(format!(".ca-alias-{}", random_id()));
            symlink(&target, &tmp)?;
            let renamed = fs::rename(&tmp, &path);
            let _ = fs::remove_file(&tmp);
            renamed?;
        }
        sync_directory(&self.authority_root())?;
        Ok(())
    }

    /// Write a new generation directory atomically (fresh dir, fsync, rename).
    pub fn create_generation(
        &self,
        cert: &[u8],
        key: &[u8],
        kind: &str,
        base_digest: &str,
    ) -> Result<String, CaError> {
        let id = random_id();
        let parent = self.generations_dir();
        create_private_dir_all(&parent)?;
        self.validate_store()?;
        let stage = parent.join(format!(".prepare-{}", random_id()));
        fs::create_dir(&stage)?;
        fs::set_permissions(&stage, fs::Permissions::from_mode(PRIVATE_DIR))?;
        let cleanup = StageCleanup(stage.clone());
        for (name, data) in [("ca.pem", cert), ("ca-key.pem", key)] {
            if !data.is_empty() {
                atomic_write(&stage.join(name), data, PRIVATE_FILE)?;
            }
        }
        let record = AuthorityRecord {
            schema: 1,
            id: id.clone(),
            kind: kind.to_string(),
            created_at: Utc::now(),
            pair_digest: pair_digest(cert, key),
            base_digest: base_digest.to_string(),
        };
        let mut metadata = serde_json::to_vec_pretty(&record)
            .map_err(|error| CaError::Invalid(error.to_string()))?;
        metadata.push(b'\n');
        atomic_write(&stage.join("metadata.json"), &metadata, PRIVATE_FILE)?;
        sync_directory(&stage)?;
        fs::rename(&stage, self.generation_dir(&id))?;
        std::mem::forget(cleanup);
        sync_directory(&parent)?;
        Ok(id)
    }

    /// Return the usable active authority, creating the initial generation on
    /// demand; inspection alone never creates storage.
    pub fn load_or_create(&self) -> Result<AuthorityMaterial, CaError> {
        if let Ok(active) = self.read_active()
            && !active.is_not_created()
        {
            if !active.info.usable {
                return Err(CaError::Invalid(active.info.detail));
            }
            if active.info.id.is_empty() || self.aliases_valid().is_ok() {
                return Ok(active);
            }
        }
        self.with_lock(&|| false, || {
            let current = self.read_active()?;
            if current.is_not_created() {
                self.validate_store()?;
                if !self.generation_ids()?.is_empty() {
                    return Err(CaError::Invalid(
                        "stored authorities exist without an active selection; use Local certificates to restore one or prepare a replacement"
                            .into(),
                    ));
                }
                if self.read_pointer(PREPARED_POINTER)?.is_some() {
                    return Err(CaError::Invalid(
                        "complete the prepared authority replacement before applying HTTPS".into(),
                    ));
                }
                let (cert, key) = new_authority(Utc::now())?;
                let id = self.create_generation(&cert, &key, "Initial", "")?;
                self.write_pointer(CURRENT_POINTER, &id)?;
                let material = self.read_generation(&id)?;
                if !material.info.usable {
                    return Err(CaError::Invalid(material.info.detail));
                }
                if material.info.id.is_empty() {
                    return Err(CaError::Invalid(
                        "created authority has no generation ID".into(),
                    ));
                }
                self.ensure_aliases(false)?;
                return Ok(material);
            }
            if !current.info.usable {
                return Err(CaError::Invalid(current.info.detail));
            }
            if !current.info.id.is_empty() {
                self.ensure_aliases(false)?;
            }
            Ok(current)
        })
    }

    /// All generation IDs currently on disk.
    pub fn generation_ids(&self) -> Result<Vec<String>, CaError> {
        let entries = match fs::read_dir(self.generations_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if entry.file_type()?.is_dir() && valid_id(&name) {
                ids.push(name.into_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// The concurrency token every mutating UI action re-checks.
    pub fn revision(
        &self,
        active: &AuthorityMaterial,
        prepared_id: &str,
        prepared: &AuthorityMaterial,
    ) -> String {
        let root = clean_path(&self.root);
        let payload = format!(
            "{}\0{}\0{}\0{}\0{}",
            root.display(),
            active.digest,
            prepared_id,
            prepared.digest,
            prepared.record.base_digest
        );
        devcore::fs::digest_bytes(payload.as_bytes())
    }
}

struct StageCleanup(PathBuf);

impl Drop for StageCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 16 lowercase hex characters, the generation identifier shape.
pub fn valid_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn random_id() -> String {
    let raw: [u8; 8] = rand::random();
    hex::encode(raw)
}

fn read_optional_regular(dir: &Path, name: &str, limit: i64) -> Result<Vec<u8>, CaError> {
    match devcore::fs::read_regular_file(dir, name, limit) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;
    use crate::ca::material::AUTHORITY_NAME;

    fn store() -> (TempDir, AuthorityStore) {
        let dir = TempDir::new().unwrap();
        let store = AuthorityStore::new(dir.path());
        (dir, store)
    }

    #[test]
    fn inspection_never_creates_storage() {
        let (dir, store) = store();
        let active = store.read_active().unwrap();
        assert_eq!(active.info.status, "Not created");
        assert!(!dir.path().join("proxy").exists());
        assert!(store.read_pointer(CURRENT_POINTER).unwrap().is_none());
        assert!(store.generation_ids().unwrap().is_empty());
    }

    #[test]
    fn initial_creation_writes_generation_pointers_and_aliases() {
        let (_dir, store) = store();
        let active = store.load_or_create().unwrap();
        assert_eq!(active.info.status, "Ready");
        assert_eq!(active.info.name, AUTHORITY_NAME);
        assert!(valid_id(&active.info.id));
        assert!(store.aliases_valid().is_ok());

        let generation = store.generation_dir(&active.info.id);
        for name in ["ca.pem", "ca-key.pem", "metadata.json"] {
            let mode = fs::metadata(generation.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{name} is private");
        }
        let dir_mode = fs::metadata(&generation).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);

        for name in [CURRENT_POINTER, "ca.pem", "ca-key.pem"] {
            let path = store.authority_root().join(name);
            assert!(
                fs::symlink_metadata(&path)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        assert_eq!(
            fs::read_link(store.authority_root().join("ca.pem")).unwrap(),
            Path::new(CURRENT_POINTER).join("ca.pem")
        );
        assert_eq!(
            fs::read_link(store.authority_root().join(CURRENT_POINTER)).unwrap(),
            Path::new("ca-generations").join(&active.info.id)
        );
    }

    #[test]
    fn generation_requires_real_directory() {
        let (dir, store) = store();
        create_private_dir_all(&dir.path().join("proxy")).unwrap();
        symlink("/tmp", store.generations_dir()).unwrap();
        assert!(store.validate_store().is_err());
        assert!(store.read_generation("0123456789abcdef").is_err());
    }

    #[test]
    fn pointer_must_target_a_generation() {
        let (_dir, store) = store();
        create_private_dir_all(&store.authority_root()).unwrap();
        symlink("/tmp", store.authority_root().join(CURRENT_POINTER)).unwrap();
        assert!(store.read_pointer(CURRENT_POINTER).is_err());
        assert!(store.write_pointer(PREPARED_POINTER, "not-an-id").is_err());
    }

    #[test]
    fn tampered_metadata_invalidates_the_generation() {
        let (_dir, store) = store();
        let active = store.load_or_create().unwrap();
        let metadata = store.generation_dir(&active.info.id).join("metadata.json");
        let mut record: AuthorityRecord =
            serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        record.pair_digest = "00".repeat(32);
        fs::write(&metadata, serde_json::to_vec(&record).unwrap()).unwrap();
        let read = store.read_generation(&active.info.id).unwrap();
        assert_eq!(read.info.status, "Invalid");
        assert!(read.info.detail.contains("generation record changed"));
        assert!(!read.info.usable);
    }

    #[test]
    fn revision_changes_with_root_and_material() {
        let (dir, store) = store();
        let active = store.load_or_create().unwrap();
        let empty = store.read_generation(&"0".repeat(16)).unwrap();
        let first = store.revision(&active, "", &empty);
        let second = store.revision(&active, "", &empty);
        assert_eq!(first, second);
        assert_ne!(first, store.revision(&active, &active.info.id, &empty));
        let other = AuthorityStore::new(dir.path().join("other"));
        assert_eq!(other.root(), dir.path().join("other"));
    }
}
