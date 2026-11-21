//! Signed in-app updater backed by [`nsh_updater`].
//!
//! Reads the packaged `nsh-updater.json` from the running `.app` bundle
//! (`Contents/Resources/nsh/assets/nsh-updater.json`, with legacy fallback to
//! `Contents/Resources/nsh/nsh-updater.json`), verifies release
//! manifests with Ed25519 signatures via [`nsh_updater::Config::check`],
//! verifies downloaded archives with SHA-256 via [`nsh_updater::Download`],
//! and installs updates atomically via [`nsh_updater::install::Installation`].
//!
//! When no release feed configuration is bundled (such as development builds
//! or releases built without an `"updater"` block), this module honestly
//! reports [`UpdateState::NotConfigured`] without making any network calls.
//! Signature verification is never bypassed.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use nsh_updater::{CONFIG_ASSET, Config, Download, install::Installation};

use crate::meta;

/// Maximum manifest or signature download size (1 MiB).
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
/// Maximum archive download size (512 MiB).
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
/// Network timeout for manifest / signature fetches.
const MANIFEST_TIMEOUT_SECS: &str = "15";
/// Network timeout for archive downloads.
const ARCHIVE_TIMEOUT_SECS: &str = "300";

/// A cryptographically verified newer release offered for this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: String,
    pub notes: Option<String>,
    pub url: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub can_install: bool,
}

/// Passive metadata about the bundled update feed (safe to read without network).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFeedInfo {
    pub configured: bool,
    pub channel: String,
    pub source: Option<String>,
}

impl Default for UpdateFeedInfo {
    fn default() -> Self {
        Self::not_configured()
    }
}

impl UpdateFeedInfo {
    pub fn not_configured() -> Self {
        UpdateFeedInfo {
            configured: false,
            channel: "Not configured".to_string(),
            source: None,
        }
    }
}

/// User-facing state of the in-app updater.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UpdateState {
    /// No `nsh-updater.json` feed is bundled with this app.
    #[default]
    NotConfigured,
    /// A signed update feed is bundled and ready to check.
    Idle,
    /// Currently fetching and verifying the signed manifest.
    Checking,
    /// The running version is already up to date.
    UpToDate,
    /// A verified newer version is available.
    Available(AvailableUpdate),
    /// Downloading, hashing, and installing the verified archive.
    Installing { version: String },
    /// Update installed on disk; relaunch Devbase to run the new version.
    Installed { version: String },
    /// The last check or install failed.
    Error(String),
}

/// Injectable updater seam so headless tests never make network or bundle calls.
pub trait UpdateProvider: Send + Sync {
    /// Passive feed metadata (must not hit the network).
    fn feed_info(&self) -> UpdateFeedInfo;

    /// Checks for a verified newer release.
    fn check(&self) -> UpdateState;

    /// Downloads (verifying SHA-256) and installs `update`, or returns an error.
    fn install(&self, update: &AvailableUpdate) -> Result<(), String>;
}

// --- Test provider registry --------------------------------------------------

static FAKE_MODE: AtomicBool = AtomicBool::new(false);
static FAKE_PROVIDER: LazyLock<Mutex<Option<Arc<dyn UpdateProvider>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Forces the updater to use fake providers instead of reading the app bundle
/// or contacting the network. Tests only.
pub fn use_fake_provider() {
    FAKE_MODE.store(true, Ordering::Relaxed);
}

/// Installs a fake [`UpdateProvider`] for headless tests. Tests only.
pub fn install_provider(provider: Arc<dyn UpdateProvider>) {
    use_fake_provider();
    *FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

/// Clears any installed fake [`UpdateProvider`] (keeping fake mode active if set).
pub fn clear_fake_provider() {
    *FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

struct DefaultFakeUpdater;

impl UpdateProvider for DefaultFakeUpdater {
    fn feed_info(&self) -> UpdateFeedInfo {
        UpdateFeedInfo::not_configured()
    }

    fn check(&self) -> UpdateState {
        UpdateState::NotConfigured
    }

    fn install(&self, _update: &AvailableUpdate) -> Result<(), String> {
        Err("updates are not configured".to_string())
    }
}

pub(crate) fn provider_of() -> Arc<dyn UpdateProvider> {
    if let Some(provider) = FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return provider;
    }
    if FAKE_MODE.load(Ordering::Relaxed) || crate::store::system::is_fake_mode() {
        return Arc::new(DefaultFakeUpdater);
    }
    Arc::new(BundleUpdater)
}

// --- Production bundle updater -----------------------------------------------

struct LoadedConfig {
    raw_bytes: Vec<u8>,
    config: Config,
}

enum ConfigLookup {
    NotFound,
    Invalid(String),
    Loaded(LoadedConfig),
}

struct BundleUpdater;

impl UpdateProvider for BundleUpdater {
    fn feed_info(&self) -> UpdateFeedInfo {
        match load_bundled_config() {
            ConfigLookup::NotFound => UpdateFeedInfo::not_configured(),
            ConfigLookup::Invalid(_) => UpdateFeedInfo {
                configured: true,
                channel: "Invalid configuration".to_string(),
                source: None,
            },
            ConfigLookup::Loaded(loaded) => feed_info_for(&loaded.config),
        }
    }

    fn check(&self) -> UpdateState {
        let loaded = match load_bundled_config() {
            ConfigLookup::NotFound => return UpdateState::NotConfigured,
            ConfigLookup::Invalid(error) => return UpdateState::Error(error),
            ConfigLookup::Loaded(loaded) => loaded,
        };

        let manifest_url = loaded.config.manifest_url();
        let Some(signature_url) = loaded.config.signature_url() else {
            return UpdateState::Error(
                "update configuration is missing a publicKey for Ed25519 signature verification"
                    .to_string(),
            );
        };

        let manifest = match fetch_bounded_bytes(&manifest_url, MAX_MANIFEST_BYTES) {
            Ok(bytes) => bytes,
            Err(error) => return UpdateState::Error(format!("Could not fetch manifest: {error}")),
        };
        let signature = match fetch_bounded_bytes(&signature_url, MAX_MANIFEST_BYTES) {
            Ok(bytes) => bytes,
            Err(error) => {
                return UpdateState::Error(format!("Could not fetch manifest signature: {error}"));
            }
        };

        let can_install = Installation::current(&loaded.config.id, &loaded.raw_bytes)
            .is_some_and(|installation| installation.can_install());

        evaluate_manifest(
            &loaded.config,
            &manifest,
            Some(&signature),
            &nsh_updater::platform(),
            can_install,
        )
    }

    fn install(&self, update: &AvailableUpdate) -> Result<(), String> {
        let loaded = match load_bundled_config() {
            ConfigLookup::NotFound => return Err("No update feed is configured.".to_string()),
            ConfigLookup::Invalid(error) => return Err(error),
            ConfigLookup::Loaded(loaded) => loaded,
        };
        let Some(sha256) = update.sha256.as_deref().filter(|value| !value.is_empty()) else {
            return Err(
                "This release does not provide a SHA-256 verified archive for in-place install."
                    .to_string(),
            );
        };
        let installation =
            Installation::current(&loaded.config.id, &loaded.raw_bytes).ok_or_else(|| {
                "In-place update requires running from a packaged Devbase.app bundle.".to_string()
            })?;
        if !installation.can_install() {
            return Err(
                "Devbase does not have permission to replace this application bundle.".to_string(),
            );
        }

        let updates_dir = devcore::state::default_root().join("updates");
        fs::create_dir_all(&updates_dir).map_err(|error| error.to_string())?;
        let archive_path = updates_dir.join(format!(
            "devbase-{}-{}.archive",
            update.version,
            std::process::id()
        ));

        let download_result =
            download_verified_archive(&update.url, &archive_path, sha256, update.size);
        if let Err(error) = download_result {
            let _ = fs::remove_file(&archive_path);
            return Err(error);
        }

        let install_result = installation
            .install(&archive_path)
            .map_err(|error| error.to_string());
        let _ = fs::remove_file(&archive_path);
        install_result
    }
}

fn current_install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let parent = exe.parent()?;
    if parent.file_name().and_then(|n| n.to_str()) == Some("MacOS")
        && let Some(contents) = parent.parent()
        && let Some(bundle) = contents.parent()
        && contents.file_name().and_then(|n| n.to_str()) == Some("Contents")
        && bundle.extension().and_then(|e| e.to_str()) == Some("app")
    {
        return Some(bundle.to_path_buf());
    }
    if crate::platform::is_linux() {
        return Some(parent.to_path_buf());
    }
    None
}

fn candidate_config_paths(install_root: &Path) -> Vec<PathBuf> {
    let macos_nsh = install_root.join("Contents/Resources/nsh");
    let linux_nsh = install_root.join("nsh");
    vec![
        macos_nsh.join("assets").join(CONFIG_ASSET),
        macos_nsh.join(CONFIG_ASSET),
        linux_nsh.join("assets").join(CONFIG_ASSET),
        linux_nsh.join(CONFIG_ASSET),
        install_root.join(CONFIG_ASSET),
    ]
}

fn load_config_from_bundle(bundle_root: &Path) -> ConfigLookup {
    let Some(path) = candidate_config_paths(bundle_root)
        .into_iter()
        .find(|candidate| candidate.is_file())
    else {
        return ConfigLookup::NotFound;
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ConfigLookup::Invalid(format!("Could not read {}: {error}", path.display()));
        }
    };
    parse_and_require_signing(&bytes)
}

fn load_bundled_config() -> ConfigLookup {
    let Some(bundle) = current_install_root() else {
        return ConfigLookup::NotFound;
    };
    load_config_from_bundle(&bundle)
}

/// Parses `nsh-updater.json` and enforces that it matches Devbase's bundle id
/// and configures an Ed25519 `publicKey` so signature verification is never skipped.
fn parse_and_require_signing(bytes: &[u8]) -> ConfigLookup {
    let config = match Config::parse(bytes) {
        Ok(config) => config,
        Err(error) => return ConfigLookup::Invalid(error.to_string()),
    };
    if config.id != meta::IDENTIFIER {
        return ConfigLookup::Invalid(format!(
            "update configuration id {:?} does not match {:?}",
            config.id,
            meta::IDENTIFIER
        ));
    }
    if config.public_key.is_none() {
        return ConfigLookup::Invalid(
            "update configuration must set publicKey for Ed25519 manifest verification".to_string(),
        );
    }
    ConfigLookup::Loaded(LoadedConfig {
        raw_bytes: bytes.to_vec(),
        config,
    })
}

fn feed_info_for(config: &Config) -> UpdateFeedInfo {
    let source = config
        .github
        .as_ref()
        .map(|repo| format!("github:{repo}"))
        .or_else(|| config.url.clone());
    UpdateFeedInfo {
        configured: true,
        channel: "Stable (signed)".to_string(),
        source,
    }
}

/// Pure manifest verification + version comparison seam used by production and tests.
pub(crate) fn evaluate_manifest(
    config: &Config,
    manifest: &[u8],
    signature: Option<&[u8]>,
    platform: &str,
    can_install_bundle: bool,
) -> UpdateState {
    if config.public_key.is_none() {
        return UpdateState::Error(
            "refusing unsigned update feed: publicKey is required".to_string(),
        );
    }
    match config.check(manifest, signature, platform) {
        Ok(None) => UpdateState::UpToDate,
        Ok(Some(available)) => UpdateState::Available(AvailableUpdate {
            version: available.version,
            notes: available.notes,
            url: available.url,
            can_install: can_install_bundle && available.sha256.is_some(),
            sha256: available.sha256,
            size: available.size,
        }),
        Err(error) => UpdateState::Error(error.to_string()),
    }
}

fn check_allowed_url(url: &str) -> Result<(), String> {
    let loopback = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|prefix| {
            url.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
        });
    if url.starts_with("https://") || loopback {
        Ok(())
    } else {
        Err(format!("refusing non-HTTPS update URL {url:?}"))
    }
}

fn fetch_bounded_bytes(url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    check_allowed_url(url)?;
    let mut cmd = Command::new(devcore::platform::resolve_curl());
    cmd.args([
        "-fsSL",
        "--max-time",
        MANIFEST_TIMEOUT_SECS,
        "--max-filesize",
        &max_bytes.to_string(),
        "--",
        url,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|error| error.to_string())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture curl stdout".to_string())?;
    let mut bytes = Vec::new();
    let mut limited = (&mut stdout).take((max_bytes + 1) as u64);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("HTTP request failed ({})", output.status)
        } else {
            stderr
        });
    }
    if bytes.len() > max_bytes {
        return Err(format!("response exceeded {max_bytes} bytes"));
    }
    Ok(bytes)
}

fn download_verified_archive(
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    expected_size: Option<u64>,
) -> Result<(), String> {
    check_allowed_url(url)?;
    let max_bytes = expected_size
        .unwrap_or(MAX_ARCHIVE_BYTES)
        .min(MAX_ARCHIVE_BYTES);

    let mut cmd = Command::new(devcore::platform::resolve_curl());
    cmd.args([
        "-fsSL",
        "--max-time",
        ARCHIVE_TIMEOUT_SECS,
        "--max-filesize",
        &max_bytes.to_string(),
        "--",
        url,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|error| error.to_string())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture download stream".to_string())?;

    let mut file = fs::File::create(dest).map_err(|error| error.to_string())?;
    let mut verifier = Download::new(expected_sha256, expected_size);
    let mut buf = [0u8; 32 * 1024];
    let mut stream_error: Option<String> = None;

    loop {
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let chunk = &buf[..n];
                if let Err(error) = verifier.update(chunk) {
                    stream_error = Some(error.to_string());
                    let _ = child.kill();
                    break;
                }
                if let Err(error) = file.write_all(chunk) {
                    stream_error = Some(error.to_string());
                    let _ = child.kill();
                    break;
                }
            }
            Err(error) => {
                stream_error = Some(error.to_string());
                let _ = child.kill();
                break;
            }
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if let Some(error) = stream_error {
        return Err(error);
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("archive download failed ({})", output.status)
        } else {
            stderr
        });
    }
    file.flush().map_err(|error| error.to_string())?;
    verifier.finish().map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Standard RFC 8032 Ed25519 test vector #1 (base64 32-byte keys).
    const TEST_SECRET_KEY: &str = "nWGxne/9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A=";
    const TEST_PUBLIC_KEY: &str = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=";

    fn signed_config_bytes(public_key: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "id": meta::IDENTIFIER,
            "version": meta::VERSION,
            "url": "https://releases.devbase.example/latest.json",
            "publicKey": public_key,
        }))
        .unwrap()
    }

    fn manifest_bytes(version: &str, notes: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "id": meta::IDENTIFIER,
            "version": version,
            "notes": notes,
            "platforms": {
                "macos-aarch64": {
                    "url": format!("Devbase-{version}-macos-arm64.tar.gz"),
                    "sha256": nsh_updater::sha256(b"fake-archive-payload"),
                    "size": 20
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn missing_bundle_config_reports_not_configured() {
        let dir =
            std::env::temp_dir().join(format!("devbase-updater-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            load_config_from_bundle(&dir),
            ConfigLookup::NotFound
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsigned_github_config_is_refused() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "id": meta::IDENTIFIER,
            "version": meta::VERSION,
            "github": "example/devbase",
        }))
        .unwrap();
        match parse_and_require_signing(&bytes) {
            ConfigLookup::Invalid(message) => {
                assert!(message.contains("publicKey"), "{message}");
            }
            other => panic!(
                "expected Invalid, got {}",
                match other {
                    ConfigLookup::NotFound => "NotFound",
                    ConfigLookup::Loaded(_) => "Loaded",
                    ConfigLookup::Invalid(_) => "Invalid",
                }
            ),
        }
    }

    #[test]
    fn foreign_bundle_id_is_refused() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "id": "com.other.app",
            "version": meta::VERSION,
            "url": "https://releases.devbase.example/latest.json",
            "publicKey": TEST_PUBLIC_KEY,
        }))
        .unwrap();
        assert!(matches!(
            parse_and_require_signing(&bytes),
            ConfigLookup::Invalid(_)
        ));
    }

    #[test]
    fn signed_manifest_verification_handles_available_up_to_date_and_tampering() {
        let config_bytes = signed_config_bytes(TEST_PUBLIC_KEY);
        let ConfigLookup::Loaded(loaded) = parse_and_require_signing(&config_bytes) else {
            panic!("expected valid signed config");
        };

        // 1. Newer signed manifest -> UpdateState::Available
        let newer = manifest_bytes("9.9.9", "- Faster proxy startup\n- Built-in MCP server");
        let sig = nsh_updater::sign(TEST_SECRET_KEY, &newer).unwrap();
        let state = evaluate_manifest(
            &loaded.config,
            &newer,
            Some(sig.as_bytes()),
            "macos-aarch64",
            true,
        );
        match state {
            UpdateState::Available(avail) => {
                assert_eq!(avail.version, "9.9.9");
                assert_eq!(
                    avail.notes.as_deref(),
                    Some("- Faster proxy startup\n- Built-in MCP server")
                );
                assert_eq!(
                    avail.url,
                    "https://releases.devbase.example/Devbase-9.9.9-macos-arm64.tar.gz"
                );
                assert!(avail.can_install);
            }
            other => panic!("expected Available, got {other:?}"),
        }

        // 2. Current version signed manifest -> UpdateState::UpToDate
        let current = manifest_bytes(meta::VERSION, "Current release");
        let current_sig = nsh_updater::sign(TEST_SECRET_KEY, &current).unwrap();
        let state = evaluate_manifest(
            &loaded.config,
            &current,
            Some(current_sig.as_bytes()),
            "macos-aarch64",
            true,
        );
        assert_eq!(state, UpdateState::UpToDate);

        // 3. Missing signature -> UpdateState::Error
        let missing_sig = evaluate_manifest(&loaded.config, &newer, None, "macos-aarch64", true);
        assert!(matches!(missing_sig, UpdateState::Error(_)));

        // 4. Tampered manifest with original signature -> UpdateState::Error
        let tampered = manifest_bytes("9.9.9", "tampered notes");
        let tampered_state = evaluate_manifest(
            &loaded.config,
            &tampered,
            Some(sig.as_bytes()),
            "macos-aarch64",
            true,
        );
        match tampered_state {
            UpdateState::Error(message) => {
                assert!(message.contains("signature"), "{message}");
            }
            other => panic!("expected signature error, got {other:?}"),
        }

        // 5. Bundle config discovery reads Contents/Resources/nsh/assets/nsh-updater.json
        // (matching `nsh package --target macos` and `nsh_updater::install::Installation::of`).
        let dir =
            std::env::temp_dir().join(format!("devbase-updater-bundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let assets_dir = dir.join("Devbase.app/Contents/Resources/nsh/assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join(CONFIG_ASSET), &config_bytes).unwrap();
        let found = load_config_from_bundle(&dir.join("Devbase.app"));
        assert!(matches!(found, ConfigLookup::Loaded(_)));

        // 6. Linux install directory config discovery reads nsh/assets/nsh-updater.json.
        let linux_dir = dir.join("devbase-linux");
        let linux_assets = linux_dir.join("nsh/assets");
        fs::create_dir_all(&linux_assets).unwrap();
        fs::write(linux_assets.join(CONFIG_ASSET), &config_bytes).unwrap();
        let found_linux = load_config_from_bundle(&linux_dir);
        assert!(matches!(found_linux, ConfigLookup::Loaded(_)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nsh_json_updater_config_is_signed_and_valid() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../nsh.json")).expect("valid app/nsh.json");
        let updater = manifest
            .get("updater")
            .expect("app/nsh.json must configure updater");
        let bundled = serde_json::to_vec(&serde_json::json!({
            "id": manifest["id"],
            "version": manifest["version"],
            "github": updater["github"],
            "publicKey": updater["publicKey"],
            "build": manifest["build"],
        }))
        .unwrap();
        let ConfigLookup::Loaded(loaded) = parse_and_require_signing(&bundled) else {
            panic!("app/nsh.json updater config must parse and require Ed25519 signing");
        };
        assert_eq!(
            loaded.config.github.as_deref(),
            Some("tdawn0-0/macOS-service-manager")
        );
        assert_eq!(
            loaded.config.manifest_url(),
            "https://github.com/tdawn0-0/macOS-service-manager/releases/latest/download/latest.json"
        );
        assert_eq!(
            loaded.config.signature_url().as_deref(),
            Some(
                "https://github.com/tdawn0-0/macOS-service-manager/releases/latest/download/latest.json.sig"
            )
        );
    }
}
