//! Secret and configuration files under `proxy/`.
//!
//! Ported from `tunnel_config.go`. Secrets are plain files with mode 0600 and
//! never enter `state.json`: the Cloudflare API token, the Worker HMAC secret,
//! and the per-Worker edge access tokens. Every write is a private atomic
//! rename; every read is bounded and validates its contents before use.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use devcore::fs::{
    PRIVATE_FILE, atomic_write, create_private_dir_all, read_regular_file, sync_directory,
};
use sha2::{Digest, Sha256};

/// The Cloudflare API token file name inside `proxy/`.
pub const CLOUDFLARE_TOKEN_FILE: &str = "cloudflare.key";
/// The Worker HMAC secret file name inside `proxy/`.
pub const TUNNEL_KEY_FILE: &str = "tunnel.key";
/// The per-Worker access token map file name inside `proxy/`.
pub const TUNNEL_ACCESS_FILE: &str = "tunnel-access.json";
/// The edge access token map's advisory lock file.
pub const TUNNEL_ACCESS_LOCK_FILE: &str = "tunnel-access.lock";
/// The tunnel routing snapshot written for the background process.
pub const TUNNEL_CONFIG_FILE: &str = "tunnel.json";
/// The tunnel live status snapshot.
pub const TUNNEL_STATUS_FILE: &str = "tunnel-status.json";

/// Maximum accepted size of any secret file (the Go reader caps at 4096).
const SECRET_LIMIT: i64 = 4096;

/// Errors raised while reading or writing secrets.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl SecretError {
    fn invalid(message: impl Into<String>) -> Self {
        SecretError::Invalid(message.into())
    }
}

/// Per-workspace access to the `proxy/` secret files.
#[derive(Debug, Clone)]
pub struct SecretStore {
    root: PathBuf,
}

impl SecretStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        SecretStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn proxy_directory(&self) -> PathBuf {
        self.root.join("proxy")
    }

    /// Create the private `proxy/` directory.
    pub fn init(&self) -> Result<(), SecretError> {
        create_private_dir_all(&self.proxy_directory())?;
        Ok(())
    }

    pub fn cloudflare_token_path(&self) -> PathBuf {
        self.proxy_directory().join(CLOUDFLARE_TOKEN_FILE)
    }

    pub fn tunnel_key_path(&self) -> PathBuf {
        self.proxy_directory().join(TUNNEL_KEY_FILE)
    }

    pub fn tunnel_access_path(&self) -> PathBuf {
        self.proxy_directory().join(TUNNEL_ACCESS_FILE)
    }

    pub fn tunnel_access_lock_path(&self) -> PathBuf {
        self.proxy_directory().join(TUNNEL_ACCESS_LOCK_FILE)
    }

    pub fn tunnel_config_path(&self) -> PathBuf {
        self.proxy_directory().join(TUNNEL_CONFIG_FILE)
    }

    pub fn tunnel_status_path(&self) -> PathBuf {
        self.proxy_directory().join(TUNNEL_STATUS_FILE)
    }

    /// Whether a plausible token file exists, without reading its contents.
    pub fn has_cloudflare_token(&self) -> bool {
        match std::fs::metadata(self.cloudflare_token_path()) {
            Ok(metadata) => metadata.is_file() && (10..=4096).contains(&metadata.len()),
            Err(_) => false,
        }
    }

    pub fn read_cloudflare_token(&self) -> Result<String, SecretError> {
        self.read_secret(
            CLOUDFLARE_TOKEN_FILE,
            validate_cloudflare_token,
            "Cloudflare API token",
        )
    }

    pub fn write_cloudflare_token(&self, token: &str) -> Result<(), SecretError> {
        let token = validate_cloudflare_token(token)?;
        self.write_secret(CLOUDFLARE_TOKEN_FILE, token.as_bytes())
    }

    pub fn read_tunnel_secret(&self) -> Result<String, SecretError> {
        self.read_secret(TUNNEL_KEY_FILE, validate_tunnel_secret, "tunnel secret")
    }

    pub fn write_tunnel_secret(&self, secret: &str) -> Result<(), SecretError> {
        let secret = validate_tunnel_secret(secret)?;
        self.write_secret(TUNNEL_KEY_FILE, secret.as_bytes())
    }

    /// Return the stored Worker HMAC secret, generating 32 random bytes as hex
    /// when none is usable yet.
    pub fn ensure_tunnel_secret(&self) -> Result<String, SecretError> {
        if let Ok(secret) = self.read_tunnel_secret() {
            return Ok(secret);
        }
        let mut raw = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut raw);
        let secret = hex::encode(raw);
        self.write_tunnel_secret(&secret)?;
        Ok(secret)
    }

    /// Read the per-Worker access token map. A missing map is empty.
    pub fn read_tunnel_access_map(&self) -> Result<BTreeMap<String, String>, SecretError> {
        let bytes =
            match read_regular_file(&self.proxy_directory(), TUNNEL_ACCESS_FILE, SECRET_LIMIT) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
                Err(error) => return Err(error.into()),
            };
        let raw: BTreeMap<String, String> = serde_json::from_slice(&bytes)
            .map_err(|error| SecretError::invalid(format!("invalid tunnel access map: {error}")))?;
        let mut cleaned = BTreeMap::new();
        for (worker, token) in raw {
            if !worker.starts_with("devbase-") || !is_dns_label(&worker) {
                return Err(SecretError::invalid(format!(
                    "invalid tunnel access worker {worker:?}"
                )));
            }
            cleaned.insert(worker, validate_tunnel_access_token(&token)?);
        }
        Ok(cleaned)
    }

    /// Serialize the access map atomically with mode 0600.
    pub fn write_tunnel_access_map(
        &self,
        tokens: &BTreeMap<String, String>,
    ) -> Result<(), SecretError> {
        let mut bytes = serde_json::to_vec_pretty(tokens)
            .map_err(|error| SecretError::invalid(error.to_string()))?;
        bytes.push(b'\n');
        atomic_write(&self.tunnel_access_path(), &bytes, PRIVATE_FILE)?;
        Ok(())
    }

    /// Run `f` while holding an exclusive, blocking lock on
    /// `proxy/tunnel-access.lock`.
    pub fn with_tunnel_access_lock<R>(
        &self,
        f: impl FnOnce() -> Result<R, SecretError>,
    ) -> Result<R, SecretError> {
        self.init()?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(PRIVATE_FILE)
            .open(self.tunnel_access_lock_path())?;
        // SAFETY: `flock` only needs a valid, open descriptor.
        let code = unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), libc::LOCK_EX) };
        if code != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let result = f();
        // SAFETY: same descriptor, still open.
        unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), libc::LOCK_UN) };
        result
    }

    /// Return the saved token for a Worker, or empty when none is stored.
    pub fn tunnel_access_token(&self, worker: &str) -> Result<String, SecretError> {
        Ok(self
            .read_tunnel_access_map()?
            .get(worker)
            .cloned()
            .unwrap_or_default())
    }

    /// Return the Worker token, generating and persisting a 32-hex-character
    /// token when the worker has none.
    pub fn ensure_tunnel_access_token(&self, worker: &str) -> Result<String, SecretError> {
        if !worker.starts_with("devbase-") || !is_dns_label(worker) {
            return Err(SecretError::invalid("invalid proxy domain"));
        }
        self.with_tunnel_access_lock(|| {
            let mut tokens = self.read_tunnel_access_map()?;
            if let Some(existing) = tokens.get(worker)
                && !existing.is_empty()
            {
                return Ok(existing.clone());
            }
            let token = random_token();
            tokens.insert(worker.to_string(), token.clone());
            self.write_tunnel_access_map(&tokens)?;
            Ok(token)
        })
    }

    /// Replace a Worker token with a fresh random value.
    pub fn rotate_tunnel_access_token(&self, worker: &str) -> Result<String, SecretError> {
        if !worker.starts_with("devbase-") || !is_dns_label(worker) {
            return Err(SecretError::invalid("invalid proxy domain"));
        }
        let token = random_token();
        self.with_tunnel_access_lock(|| {
            let mut tokens = self.read_tunnel_access_map()?;
            tokens.insert(worker.to_string(), token.clone());
            self.write_tunnel_access_map(&tokens)?;
            Ok(())
        })?;
        Ok(token)
    }

    fn read_secret(
        &self,
        name: &str,
        validate: fn(&str) -> Result<String, SecretError>,
        label: &str,
    ) -> Result<String, SecretError> {
        let bytes = read_regular_file(&self.proxy_directory(), name, SECRET_LIMIT)?;
        if bytes.len() as i64 > SECRET_LIMIT {
            return Err(SecretError::invalid(format!(
                "{label} file exceeds size limit"
            )));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| SecretError::invalid(format!("{label} is not valid UTF-8")))?;
        validate(&text)
    }

    fn write_secret(&self, name: &str, bytes: &[u8]) -> Result<(), SecretError> {
        self.init()?;
        atomic_write(&self.proxy_directory().join(name), bytes, PRIVATE_FILE)?;
        sync_directory(&self.proxy_directory())?;
        Ok(())
    }
}

/// The first 16 hex characters of SHA-256, stored in state to detect a changed
/// edge token without persisting the token itself.
pub fn tunnel_auth_digest(token: &str) -> String {
    if token.is_empty() {
        return String::new();
    }
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)[..16].to_string()
}

pub fn validate_cloudflare_token(token: &str) -> Result<String, SecretError> {
    let token = token.trim();
    if !(10..=512).contains(&token.len()) {
        return Err(SecretError::invalid(
            "Cloudflare API token must be 10–512 characters",
        ));
    }
    if token.contains([' ', '\t', '\r', '\n']) {
        return Err(SecretError::invalid(
            "Cloudflare API token cannot contain whitespace",
        ));
    }
    Ok(token.to_string())
}

pub fn validate_tunnel_secret(secret: &str) -> Result<String, SecretError> {
    let secret = secret.trim();
    if !(16..=512).contains(&secret.len()) {
        return Err(SecretError::invalid(
            "tunnel secret must be 16–512 characters",
        ));
    }
    if secret.contains(['\r', '\n']) {
        return Err(SecretError::invalid("tunnel secret must be a single line"));
    }
    Ok(secret.to_string())
}

pub fn validate_tunnel_access_token(token: &str) -> Result<String, SecretError> {
    let token = token.trim();
    if !(16..=256).contains(&token.len()) {
        return Err(SecretError::invalid(
            "tunnel access token must be 16–256 characters",
        ));
    }
    if token.contains(['\r', '\n', '\0']) {
        return Err(SecretError::invalid(
            "tunnel access token must be a single line",
        ));
    }
    Ok(token.to_string())
}

fn random_token() -> String {
    let mut raw = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut raw);
    hex::encode(raw)
}

fn is_dns_label(value: &str) -> bool {
    if value.is_empty() || value.len() > 63 {
        return false;
    }
    let bytes = value.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return false;
    }
    let last = bytes[bytes.len() - 1];
    if !last.is_ascii_lowercase() && !last.is_ascii_digit() {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;

    fn store() -> (TempDir, SecretStore) {
        let dir = TempDir::new().unwrap();
        let store = SecretStore::new(dir.path());
        (dir, store)
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn secret_writes_are_private() {
        let (_dir, store) = store();
        store
            .write_cloudflare_token("abcdefghijklmnopqrstuvwxyz")
            .unwrap();
        store.write_tunnel_secret(&"a".repeat(64)).unwrap();
        assert_eq!(mode(&store.cloudflare_token_path()), 0o600);
        assert_eq!(mode(&store.tunnel_key_path()), 0o600);
        assert_eq!(
            store.read_cloudflare_token().unwrap(),
            "abcdefghijklmnopqrstuvwxyz"
        );
        assert!(store.has_cloudflare_token());
    }

    #[test]
    fn tunnel_secret_is_generated_as_32_bytes_of_hex() {
        let (_dir, store) = store();
        let secret = store.ensure_tunnel_secret().unwrap();
        assert_eq!(secret.len(), 64);
        assert!(hex::decode(&secret).is_ok());
        assert_eq!(store.ensure_tunnel_secret().unwrap(), secret);
        assert_eq!(mode(&store.tunnel_key_path()), 0o600);
    }

    #[test]
    fn token_validators_reject_short_and_multiline_values() {
        assert!(validate_cloudflare_token("short").is_err());
        assert!(validate_cloudflare_token("has space in it").is_err());
        assert_eq!(
            validate_cloudflare_token("  abcdefghijklmnop  ").unwrap(),
            "abcdefghijklmnop"
        );
        assert!(validate_tunnel_secret("too-short").is_err());
        assert!(validate_tunnel_secret(&("a".repeat(8) + "\n" + &"a".repeat(8))).is_err());
        assert!(validate_tunnel_access_token("tiny").is_err());
        assert!(validate_tunnel_access_token(&("a".repeat(8) + "\r" + &"a".repeat(8))).is_err());
    }

    #[test]
    fn access_tokens_are_created_rotated_and_private() {
        let (_dir, store) = store();
        let first = store.ensure_tunnel_access_token("devbase-app").unwrap();
        assert_eq!(first.len(), 32);
        assert_eq!(
            store.ensure_tunnel_access_token("devbase-app").unwrap(),
            first
        );
        assert_eq!(mode(&store.tunnel_access_path()), 0o600);
        assert_eq!(mode(&store.tunnel_access_lock_path()), 0o600);

        let rotated = store.rotate_tunnel_access_token("devbase-app").unwrap();
        assert_ne!(rotated, first);
        assert_eq!(store.tunnel_access_token("devbase-app").unwrap(), rotated);
        assert_eq!(
            store.read_tunnel_access_map().unwrap().len(),
            1,
            "the map holds only worker tokens"
        );
        assert!(store.ensure_tunnel_access_token("other-worker").is_err());
    }

    #[test]
    fn access_map_rejects_invalid_worker_names() {
        let (_dir, store) = store();
        store.init().unwrap();
        std::fs::write(
            store.tunnel_access_path(),
            r#"{"notdevbase":"abcdefghijklmnop"}"#,
        )
        .unwrap();
        assert!(store.read_tunnel_access_map().is_err());
    }

    #[test]
    fn auth_digest_is_stable_and_short() {
        assert_eq!(tunnel_auth_digest(""), "");
        let digest = tunnel_auth_digest("abcdefghijklmnop");
        assert_eq!(digest.len(), 16);
        assert_eq!(digest, tunnel_auth_digest("abcdefghijklmnop"));
        assert_ne!(digest, tunnel_auth_digest("abcdefghijklmnoq"));
    }
}
