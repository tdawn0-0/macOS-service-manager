//! Platform CA trust stores: macOS keychain (`/usr/bin/security`) and Linux
//! system trust anchors + optional user NSS database (`~/.pki/nssdb`).
//!
//! Every operating-system boundary — `/usr/bin/security`, the privileged helper
//! client, `pkexec`, `openssl`, and `certutil` — is injected through traits so
//! tests use temp directories and fakes and never touch the real keychain or
//! `/etc`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use devcore::helper::admin::shell_quote;
use devcore::helper::paths::shell_quote_path;
use devcore::helper::{
    AdminError, AdminRunner, CaInstallRequest, CaRemoveRequest, CaTrustTarget, Client, HelperError,
    PkexecAdmin, detect_linux_ca_trust_target,
};
use devcore::launchd::{CommandError, ProcessRunner, SystemRunner};
use devcore::platform::resolve_program_in_path;

use super::CaError;
use super::material::{AUTHORITY_NAME, TrustState, decode_cert_der, sha1_fingerprint};

/// Phrases macOS prints when a certificate is already absent.
const ITEM_MISSING: &str = "the specified item could not be found in the keychain";

/// Default anchor base name installed into the Linux system trust store.
pub const LINUX_CA_ANCHOR_NAME: &str = "devbase-ca";

/// The subset of trust operations the authority service needs.
pub trait TrustStore: Send + Sync {
    fn verify(&self, certificate_path: &Path) -> TrustState;
    fn apply(&self, certificate_path: &Path) -> Result<(), CaError>;
    fn remove(&self, certificate_path: &Path, certificate_der: &[u8]) -> Result<(), CaError>;
}

/// Production trust store backed by the `security` executable.
#[derive(Clone)]
pub struct SecurityTrust {
    runner: Arc<dyn ProcessRunner>,
    security: PathBuf,
}

impl SecurityTrust {
    pub fn new(runner: Arc<dyn ProcessRunner>) -> Self {
        SecurityTrust {
            runner,
            security: PathBuf::from("/usr/bin/security"),
        }
    }

    fn run(&self, args: &[&str]) -> Result<String, CommandError> {
        self.runner
            .run(self.security.to_string_lossy().as_ref(), args)
    }

    fn default_keychain(&self) -> Result<String, CaError> {
        let output = self
            .run(&["default-keychain", "-d", "user"])
            .map_err(|error| CaError::Command(error.to_string()))?;
        let keychain = output.trim().trim_matches('"').to_string();
        if !Path::new(&keychain).is_absolute() {
            return Err(CaError::Invalid("cannot locate the user keychain".into()));
        }
        Ok(keychain)
    }
}

impl TrustStore for SecurityTrust {
    fn verify(&self, certificate_path: &Path) -> TrustState {
        let path = certificate_path.to_string_lossy();
        match self.run(&["verify-cert", "-c", path.as_ref(), "-p", "basic", "-l"]) {
            Ok(_) => TrustState::Trusted,
            Err(error) if error.timed_out => TrustState::Unknown,
            Err(_) => TrustState::NotTrusted,
        }
    }

    fn apply(&self, certificate_path: &Path) -> Result<(), CaError> {
        if self.verify(certificate_path) == TrustState::Trusted {
            return Ok(());
        }
        let keychain = self.default_keychain()?;
        let path = certificate_path.to_string_lossy();
        self.run(&[
            "add-trusted-cert",
            "-r",
            "trustRoot",
            "-k",
            &keychain,
            path.as_ref(),
        ])
        .map_err(|error| CaError::Command(error.to_string()))?;
        if self.verify(certificate_path) != TrustState::Trusted {
            return Err(CaError::Untrusted(
                "macOS did not verify the CA after the trust request; the active authority was kept"
                    .into(),
            ));
        }
        Ok(())
    }

    fn remove(&self, certificate_path: &Path, certificate_der: &[u8]) -> Result<(), CaError> {
        let keychain = self.default_keychain()?;
        let path = certificate_path.to_string_lossy();
        if let Err(error) = self.run(&["remove-trusted-cert", path.as_ref()])
            && !keychain_item_missing(&error)
        {
            return Err(CaError::Command(error.to_string()));
        }
        let fingerprint = sha1_fingerprint(certificate_der);
        match self.run(&["delete-certificate", "-Z", &fingerprint, &keychain]) {
            Ok(_) => Ok(()),
            Err(error) if keychain_item_missing(&error) => Ok(()),
            Err(error) => Err(CaError::Command(error.to_string())),
        }
    }
}

/// macOS's explicit not-found text is the only failure treated as success.
pub fn keychain_item_missing(error: &CommandError) -> bool {
    let text = format!("{}{}", error.stderr, error.stdout).to_lowercase();
    text.contains(ITEM_MISSING)
}

/// Privileged helper CA operations used by [`LinuxTrustStore`].
pub trait LinuxCaHelper: Send + Sync {
    fn ca_install(&self, request: &CaInstallRequest) -> Result<(), HelperError>;
    fn ca_remove(&self, request: &CaRemoveRequest) -> Result<(), HelperError>;
}

impl LinuxCaHelper for Client {
    fn ca_install(&self, request: &CaInstallRequest) -> Result<(), HelperError> {
        Client::ca_install(self, request)
    }

    fn ca_remove(&self, request: &CaRemoveRequest) -> Result<(), HelperError> {
        Client::ca_remove(self, request)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DefaultLinuxCaHelper;

impl LinuxCaHelper for DefaultLinuxCaHelper {
    fn ca_install(&self, request: &CaInstallRequest) -> Result<(), HelperError> {
        let client = Client::require()?;
        client.ca_install(request)
    }

    fn ca_remove(&self, request: &CaRemoveRequest) -> Result<(), HelperError> {
        let client = Client::require()?;
        client.ca_remove(request)
    }
}

#[derive(Debug, Clone)]
enum ToolPath {
    Auto,
    Path(PathBuf),
    Disabled,
}

impl ToolPath {
    fn resolve(&self, program: &str) -> Option<PathBuf> {
        match self {
            ToolPath::Auto => resolve_program_in_path(program, None),
            ToolPath::Path(path) => Some(path.clone()),
            ToolPath::Disabled => None,
        }
    }
}

/// Linux local CA trust store backed by the distribution's system trust anchor
/// directory (Arch/CachyOS, Debian/Ubuntu, Fedora/RHEL) and optional user NSS
/// database (`~/.pki/nssdb` via `certutil`).
///
/// Privileged system anchor writes/removals delegate to
/// [`devcore::helper::Client::ca_install`] / [`devcore::helper::Client::ca_remove`],
/// falling back to `pkexec` via [`AdminRunner`] when the privileged helper is
/// not installed.
#[derive(Clone)]
pub struct LinuxTrustStore {
    runner: Arc<dyn ProcessRunner>,
    helper: Arc<dyn LinuxCaHelper>,
    admin: Arc<dyn AdminRunner>,
    anchor_name: String,
    trust_target: Option<CaTrustTarget>,
    nss_db_dir: Option<PathBuf>,
    openssl: ToolPath,
    certutil: ToolPath,
}

impl LinuxTrustStore {
    /// Production Linux trust store using system `ProcessRunner`, the installed
    /// privileged helper (with `PkexecAdmin` fallback), distro auto-detection,
    /// and `$HOME/.pki/nssdb`.
    pub fn system() -> Self {
        Self::new(Arc::new(SystemRunner::new()))
    }

    /// Creates a Linux trust store with an explicit [`ProcessRunner`].
    pub fn new(runner: Arc<dyn ProcessRunner>) -> Self {
        LinuxTrustStore {
            runner,
            helper: Arc::new(DefaultLinuxCaHelper),
            admin: Arc::new(PkexecAdmin),
            anchor_name: LINUX_CA_ANCHOR_NAME.to_string(),
            trust_target: None,
            nss_db_dir: default_nss_db_dir(),
            openssl: ToolPath::Auto,
            certutil: ToolPath::Auto,
        }
    }

    /// Overrides the privileged helper backend (used in tests or with a custom
    /// helper client).
    pub fn with_helper(mut self, helper: Arc<dyn LinuxCaHelper>) -> Self {
        self.helper = helper;
        self
    }

    /// Overrides the `pkexec` fallback runner (used in tests).
    pub fn with_admin(mut self, admin: Arc<dyn AdminRunner>) -> Self {
        self.admin = admin;
        self
    }

    /// Overrides the detected Linux CA trust target (used in tests to isolate
    /// anchor files in a `TempDir` on both macOS and Linux).
    pub fn with_trust_target(mut self, target: CaTrustTarget) -> Self {
        self.trust_target = Some(target);
        self
    }

    /// Overrides the installed anchor base name (defaults to `devbase-ca`).
    pub fn with_anchor_name(mut self, name: impl Into<String>) -> Self {
        self.anchor_name = name.into();
        self
    }

    /// Overrides the user NSS database directory (pass `None` to disable NSS
    /// updates).
    pub fn with_nss_db_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.nss_db_dir = dir;
        self
    }

    /// Explicitly sets or disables the `openssl` executable path used during
    /// [`Self::is_trusted`].
    pub fn with_openssl(mut self, path: Option<PathBuf>) -> Self {
        self.openssl = match path {
            Some(path) => ToolPath::Path(path),
            None => ToolPath::Disabled,
        };
        self
    }

    /// Explicitly sets or disables the `certutil` executable path used for user
    /// NSS DB trust updates.
    pub fn with_certutil(mut self, path: Option<PathBuf>) -> Self {
        self.certutil = match path {
            Some(path) => ToolPath::Path(path),
            None => ToolPath::Disabled,
        };
        self
    }

    /// Resolves the anchor path for `target` and `name`.
    pub fn anchor_path_for(target: &CaTrustTarget, name: &str) -> PathBuf {
        let suffix = format!(".{}", target.extension);
        let filename = if name.ends_with(&suffix) {
            name.to_string()
        } else {
            format!("{name}{suffix}")
        };
        target.anchor_dir.join(filename)
    }

    fn resolve_target(&self) -> Result<CaTrustTarget, String> {
        match &self.trust_target {
            Some(target) => Ok(target.clone()),
            None => detect_linux_ca_trust_target(),
        }
    }

    /// Verifies `certificate_path` against the Linux system trust store:
    /// checks that the installed anchor file matches the certificate and, when
    /// `openssl` is present, runs `openssl verify`.
    pub fn is_trusted(&self, certificate_path: &Path) -> TrustState {
        let Ok(target) = self.resolve_target() else {
            return TrustState::NotTrusted;
        };
        let Ok(cert_pem) = std::fs::read(certificate_path) else {
            return TrustState::NotTrusted;
        };
        if cert_pem.is_empty() {
            return TrustState::NotTrusted;
        }
        let anchor_path = Self::anchor_path_for(&target, &self.anchor_name);
        let Ok(anchor_pem) = std::fs::read(&anchor_path) else {
            return TrustState::NotTrusted;
        };
        if !certificates_match(&cert_pem, &anchor_pem) {
            return TrustState::NotTrusted;
        }
        let Some(openssl) = self.openssl.resolve("openssl") else {
            return TrustState::Trusted;
        };
        let path = certificate_path.to_string_lossy();
        match self.runner.run(
            openssl.to_string_lossy().as_ref(),
            &["verify", path.as_ref()],
        ) {
            Ok(_) => TrustState::Trusted,
            Err(error) if error.timed_out => TrustState::Unknown,
            Err(_) => TrustState::NotTrusted,
        }
    }

    /// Installs `certificate_path` into the Linux system trust store (via the
    /// privileged helper or `pkexec` fallback) and updates the optional user
    /// NSS DB when `certutil` is available.
    pub fn install_trusted_ca(&self, certificate_path: &Path) -> Result<(), CaError> {
        if self.is_trusted(certificate_path) == TrustState::Trusted {
            return Ok(());
        }
        let cert_pem = std::fs::read(certificate_path)?;
        if decode_cert_der(&cert_pem).is_none() {
            return Err(CaError::Invalid(
                "the CA certificate is not PEM encoded".into(),
            ));
        }

        let request = CaInstallRequest {
            cert_pem,
            name: self.anchor_name.clone(),
        };
        match self.helper.ca_install(&request) {
            Ok(()) => {}
            Err(HelperError::NotInstalled | HelperError::Disabled) => {
                let target = self.resolve_target().map_err(CaError::Invalid)?;
                self.install_via_pkexec(&target, certificate_path)?;
            }
            Err(error) => return Err(CaError::Command(error.to_string())),
        }

        self.install_nss_trust(certificate_path);

        if self.is_trusted(certificate_path) != TrustState::Trusted {
            return Err(CaError::Untrusted(
                "Linux system trust store did not verify the CA after the trust request; the active authority was kept"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Removes the CA anchor from the Linux system trust store (if it belongs
    /// to this certificate) and removes the optional user NSS DB entry when
    /// `certutil` is available.
    pub fn remove_trusted_ca(
        &self,
        certificate_path: &Path,
        certificate_der: &[u8],
    ) -> Result<(), CaError> {
        let target_der = if !certificate_der.is_empty() {
            Some(certificate_der.to_vec())
        } else {
            std::fs::read(certificate_path)
                .ok()
                .and_then(|pem| decode_cert_der(&pem))
        };

        let should_remove_system_anchor = match (self.resolve_target(), target_der.as_deref()) {
            (Ok(target), Some(expected_der)) => {
                let anchor_path = Self::anchor_path_for(&target, &self.anchor_name);
                match std::fs::read(&anchor_path) {
                    Ok(anchor_pem) => match decode_cert_der(&anchor_pem) {
                        Some(anchor_der) => anchor_der == expected_der,
                        None => true,
                    },
                    Err(_) => true,
                }
            }
            _ => true,
        };

        if should_remove_system_anchor {
            let request = CaRemoveRequest {
                name: self.anchor_name.clone(),
            };
            match self.helper.ca_remove(&request) {
                Ok(()) => {}
                Err(HelperError::NotInstalled | HelperError::Disabled) => {
                    let target = self.resolve_target().map_err(CaError::Invalid)?;
                    self.remove_via_pkexec(&target)?;
                }
                Err(error) => return Err(CaError::Command(error.to_string())),
            }
        }

        self.remove_nss_trust();
        Ok(())
    }

    fn install_via_pkexec(
        &self,
        target: &CaTrustTarget,
        certificate_path: &Path,
    ) -> Result<(), CaError> {
        let anchor_path = Self::anchor_path_for(target, &self.anchor_name);
        let script = format!(
            "set -eu; install -d -m 0755 -o root -g root {}; install -m 0644 -o root -g root {} {}; {}",
            shell_quote_path(&target.anchor_dir),
            shell_quote_path(certificate_path),
            shell_quote_path(&anchor_path),
            ca_update_shell_command(target),
        );
        self.run_admin_script(&script)
    }

    fn remove_via_pkexec(&self, target: &CaTrustTarget) -> Result<(), CaError> {
        let anchor_path = Self::anchor_path_for(target, &self.anchor_name);
        let script = format!(
            "set -eu; rm -f {}; {}",
            shell_quote_path(&anchor_path),
            ca_update_shell_command(target),
        );
        self.run_admin_script(&script)
    }

    fn run_admin_script(&self, script: &str) -> Result<(), CaError> {
        match self.admin.run_admin("/bin/sh", &["-c", script]) {
            (_, None) => Ok(()),
            (_, Some(AdminError::Canceled)) => Err(CaError::Cancelled),
            (output, Some(AdminError::Message(message))) => {
                if message.trim().is_empty() && !output.trim().is_empty() {
                    Err(CaError::Command(output.trim().to_string()))
                } else {
                    Err(CaError::Command(message))
                }
            }
        }
    }

    fn install_nss_trust(&self, certificate_path: &Path) {
        let Some(nss_dir) = self.nss_db_dir.as_deref() else {
            return;
        };
        if !nss_dir.is_dir() {
            return;
        }
        let Some(certutil) = self.certutil.resolve("certutil") else {
            return;
        };
        let certutil_str = certutil.to_string_lossy();
        let db = format!("sql:{}", nss_dir.to_string_lossy());
        let cert_str = certificate_path.to_string_lossy();
        let _ = self.runner.run(
            certutil_str.as_ref(),
            &["-d", &db, "-D", "-n", AUTHORITY_NAME],
        );
        let _ = self.runner.run(
            certutil_str.as_ref(),
            &[
                "-d",
                &db,
                "-A",
                "-t",
                "C,,",
                "-n",
                AUTHORITY_NAME,
                "-i",
                cert_str.as_ref(),
            ],
        );
    }

    fn remove_nss_trust(&self) {
        let Some(nss_dir) = self.nss_db_dir.as_deref() else {
            return;
        };
        if !nss_dir.is_dir() {
            return;
        }
        let Some(certutil) = self.certutil.resolve("certutil") else {
            return;
        };
        let db = format!("sql:{}", nss_dir.to_string_lossy());
        let _ = self.runner.run(
            certutil.to_string_lossy().as_ref(),
            &["-d", &db, "-D", "-n", AUTHORITY_NAME],
        );
    }
}

impl TrustStore for LinuxTrustStore {
    fn verify(&self, certificate_path: &Path) -> TrustState {
        self.is_trusted(certificate_path)
    }

    fn apply(&self, certificate_path: &Path) -> Result<(), CaError> {
        self.install_trusted_ca(certificate_path)
    }

    fn remove(&self, certificate_path: &Path, certificate_der: &[u8]) -> Result<(), CaError> {
        self.remove_trusted_ca(certificate_path, certificate_der)
    }
}

/// Returns the default platform [`TrustStore`]: [`SecurityTrust`] on macOS and
/// [`LinuxTrustStore`] on Linux.
pub fn default_trust_store() -> Arc<dyn TrustStore> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(SecurityTrust::new(Arc::new(SystemRunner::new())))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(LinuxTrustStore::system())
    }
}

fn default_nss_db_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".pki").join("nssdb"))
}

fn ca_update_shell_command(target: &CaTrustTarget) -> String {
    let mut parts = vec![shell_quote(&target.update_program)];
    for arg in &target.update_args {
        parts.push(shell_quote(arg));
    }
    let primary = parts.join(" ");
    if target.update_program == "update-ca-trust" && target.update_args.is_empty() {
        format!(
            "if command -v update-ca-trust >/dev/null 2>&1; then {primary}; else trust extract-compat; fi"
        )
    } else {
        primary
    }
}

fn certificates_match(first_pem: &[u8], second_pem: &[u8]) -> bool {
    match (decode_cert_der(first_pem), decode_cert_der(second_pem)) {
        (Some(first_der), Some(second_der)) => first_der == second_der,
        _ => {
            let a = std::str::from_utf8(first_pem).map(str::trim).unwrap_or("");
            let b = std::str::from_utf8(second_pem).map(str::trim).unwrap_or("");
            !a.is_empty() && a == b
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        responses: Mutex<VecDeque<Result<String, CommandError>>>,
    }

    impl FakeRunner {
        fn with(responses: Vec<Result<String, CommandError>>) -> Arc<Self> {
            Arc::new(FakeRunner {
                calls: Mutex::new(Vec::new()),
                responses: Mutex::new(responses.into()),
            })
        }

        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ProcessRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
            self.calls.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|arg| arg.to_string()).collect(),
            ));
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Ok(String::new());
            }
            responses.pop_front().unwrap()
        }
    }

    fn ok() -> Result<String, CommandError> {
        Ok(String::new())
    }

    fn fail(text: &str) -> Result<String, CommandError> {
        Err(CommandError::new("/usr/bin/security", Vec::new(), text))
    }

    const CERT: &str = "/root/proxy/ca-generations/0123456789abcdef/ca.pem";
    const KEYCHAIN: &str = "/Users/me/Library/Keychains/login.keychain-db";

    #[test]
    fn apply_verifies_keychain_and_certificate_with_exact_argv() {
        let runner = FakeRunner::with(vec![
            fail("not trusted"),             // verify
            Ok(format!("\"{KEYCHAIN}\"\n")), // default-keychain
            ok(),                            // add-trusted-cert
            ok(),                            // verify
        ]);
        let trust = SecurityTrust::new(runner.clone());
        trust.apply(Path::new(CERT)).unwrap();

        assert_eq!(
            runner.calls(),
            vec![
                (
                    "/usr/bin/security".into(),
                    vec![
                        "verify-cert".into(),
                        "-c".into(),
                        CERT.into(),
                        "-p".into(),
                        "basic".into(),
                        "-l".into(),
                    ]
                ),
                (
                    "/usr/bin/security".into(),
                    vec!["default-keychain".into(), "-d".into(), "user".into()]
                ),
                (
                    "/usr/bin/security".into(),
                    vec![
                        "add-trusted-cert".into(),
                        "-r".into(),
                        "trustRoot".into(),
                        "-k".into(),
                        "/Users/me/Library/Keychains/login.keychain-db".into(),
                        CERT.into(),
                    ]
                ),
                (
                    "/usr/bin/security".into(),
                    vec![
                        "verify-cert".into(),
                        "-c".into(),
                        CERT.into(),
                        "-p".into(),
                        "basic".into(),
                        "-l".into(),
                    ]
                ),
            ]
        );
    }

    #[test]
    fn apply_is_a_noop_when_already_trusted() {
        let runner = FakeRunner::with(vec![ok()]);
        let trust = SecurityTrust::new(runner.clone());
        trust.apply(Path::new(CERT)).unwrap();
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn apply_keeps_active_authority_when_verification_still_fails() {
        let runner = FakeRunner::with(vec![
            fail("not trusted"),
            Ok(format!("\"{KEYCHAIN}\"")),
            ok(),
            fail("still not trusted"),
        ]);
        let trust = SecurityTrust::new(runner);
        let error = trust.apply(Path::new(CERT)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "macOS did not verify the CA after the trust request; the active authority was kept"
        );
    }

    #[test]
    fn remove_uses_uppercase_sha1_and_tolerates_missing_items() {
        let runner = FakeRunner::with(vec![
            Ok(format!("\"{KEYCHAIN}\"\n")),
            ok(),
            fail("security: The specified item could not be found in the keychain."),
        ]);
        let trust = SecurityTrust::new(runner.clone());
        trust.remove(Path::new(CERT), b"der").unwrap();
        let calls = runner.calls();
        assert_eq!(calls[1].1, vec!["remove-trusted-cert", CERT]);
        let expected = sha1_fingerprint(b"der");
        assert_eq!(
            calls[2].1,
            vec![
                "delete-certificate",
                "-Z",
                expected.as_str(),
                "/Users/me/Library/Keychains/login.keychain-db"
            ]
        );
    }

    #[test]
    fn non_absolute_keychain_is_rejected() {
        let runner = FakeRunner::with(vec![fail("not trusted"), Ok("login.keychain\n".into())]);
        let trust = SecurityTrust::new(runner);
        let error = trust.apply(Path::new(CERT)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot locate the user keychain")
        );
    }

    struct FakeLinuxHelper {
        target: CaTrustTarget,
        install_result: Mutex<Result<(), HelperError>>,
        remove_result: Mutex<Result<(), HelperError>>,
        installed: Mutex<Vec<CaInstallRequest>>,
        removed: Mutex<Vec<CaRemoveRequest>>,
        apply_to_disk: bool,
    }

    impl FakeLinuxHelper {
        fn writing_to(target: CaTrustTarget) -> Arc<Self> {
            Arc::new(FakeLinuxHelper {
                target,
                install_result: Mutex::new(Ok(())),
                remove_result: Mutex::new(Ok(())),
                installed: Mutex::new(Vec::new()),
                removed: Mutex::new(Vec::new()),
                apply_to_disk: true,
            })
        }

        fn not_installed(target: CaTrustTarget) -> Arc<Self> {
            Arc::new(FakeLinuxHelper {
                target,
                install_result: Mutex::new(Err(HelperError::NotInstalled)),
                remove_result: Mutex::new(Err(HelperError::NotInstalled)),
                installed: Mutex::new(Vec::new()),
                removed: Mutex::new(Vec::new()),
                apply_to_disk: false,
            })
        }
    }

    impl LinuxCaHelper for FakeLinuxHelper {
        fn ca_install(&self, request: &CaInstallRequest) -> Result<(), HelperError> {
            self.installed.lock().unwrap().push(request.clone());
            self.install_result.lock().unwrap().clone()?;
            if self.apply_to_disk {
                std::fs::create_dir_all(&self.target.anchor_dir).unwrap();
                let anchor = LinuxTrustStore::anchor_path_for(&self.target, &request.name);
                std::fs::write(anchor, &request.cert_pem).unwrap();
            }
            Ok(())
        }

        fn ca_remove(&self, request: &CaRemoveRequest) -> Result<(), HelperError> {
            self.removed.lock().unwrap().push(request.clone());
            self.remove_result.lock().unwrap().clone()?;
            if self.apply_to_disk {
                let anchor = LinuxTrustStore::anchor_path_for(&self.target, &request.name);
                let _ = std::fs::remove_file(anchor);
            }
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FakeAdmin {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        error: Mutex<Option<AdminError>>,
        anchor_copy: Option<(PathBuf, PathBuf)>,
        anchor_remove: Option<PathBuf>,
    }

    impl FakeAdmin {
        fn succeeding() -> Arc<Self> {
            Arc::new(FakeAdmin {
                calls: Mutex::new(Vec::new()),
                error: Mutex::new(None),
                anchor_copy: None,
                anchor_remove: None,
            })
        }

        fn copying(source: PathBuf, destination: PathBuf) -> Arc<Self> {
            Arc::new(FakeAdmin {
                calls: Mutex::new(Vec::new()),
                error: Mutex::new(None),
                anchor_copy: Some((source, destination)),
                anchor_remove: None,
            })
        }

        fn removing(anchor: PathBuf) -> Arc<Self> {
            Arc::new(FakeAdmin {
                calls: Mutex::new(Vec::new()),
                error: Mutex::new(None),
                anchor_copy: None,
                anchor_remove: Some(anchor),
            })
        }

        fn failing(error: AdminError) -> Arc<Self> {
            Arc::new(FakeAdmin {
                calls: Mutex::new(Vec::new()),
                error: Mutex::new(Some(error)),
                anchor_copy: None,
                anchor_remove: None,
            })
        }

        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl AdminRunner for FakeAdmin {
        fn run_admin(&self, program: &str, args: &[&str]) -> (String, Option<AdminError>) {
            self.calls.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|arg| arg.to_string()).collect(),
            ));
            if let Some(error) = self.error.lock().unwrap().clone() {
                return (String::new(), Some(error));
            }
            if let Some((source, destination)) = &self.anchor_copy {
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::copy(source, destination).unwrap();
            }
            if let Some(anchor) = &self.anchor_remove {
                let _ = std::fs::remove_file(anchor);
            }
            (String::new(), None)
        }
    }

    fn arch_target(dir: &Path) -> CaTrustTarget {
        CaTrustTarget {
            anchor_dir: dir.join("arch-anchors"),
            extension: "crt".into(),
            update_program: "update-ca-trust".into(),
            update_args: vec![],
        }
    }

    fn debian_target(dir: &Path) -> CaTrustTarget {
        CaTrustTarget {
            anchor_dir: dir.join("debian-anchors"),
            extension: "crt".into(),
            update_program: "update-ca-certificates".into(),
            update_args: vec![],
        }
    }

    fn fedora_target(dir: &Path) -> CaTrustTarget {
        CaTrustTarget {
            anchor_dir: dir.join("fedora-anchors"),
            extension: "pem".into(),
            update_program: "update-ca-trust".into(),
            update_args: vec!["extract".into()],
        }
    }

    fn write_test_ca(dir: &Path, filename: &str) -> (PathBuf, Vec<u8>, Vec<u8>) {
        let (cert_pem, _key_pem) = crate::ca::material::new_authority(chrono::Utc::now()).unwrap();
        let cert_der = decode_cert_der(&cert_pem).unwrap();
        let path = dir.join(filename);
        std::fs::write(&path, &cert_pem).unwrap();
        (path, cert_pem, cert_der)
    }

    #[test]
    fn linux_anchor_paths_and_update_commands_cover_arch_debian_and_fedora() {
        let dir = tempfile::TempDir::new().unwrap();
        let arch = arch_target(dir.path());
        let debian = debian_target(dir.path());
        let fedora = fedora_target(dir.path());

        assert_eq!(
            LinuxTrustStore::anchor_path_for(&arch, LINUX_CA_ANCHOR_NAME),
            arch.anchor_dir.join("devbase-ca.crt")
        );
        assert_eq!(
            LinuxTrustStore::anchor_path_for(&debian, LINUX_CA_ANCHOR_NAME),
            debian.anchor_dir.join("devbase-ca.crt")
        );
        assert_eq!(
            LinuxTrustStore::anchor_path_for(&fedora, LINUX_CA_ANCHOR_NAME),
            fedora.anchor_dir.join("devbase-ca.pem")
        );
        assert!(ca_update_shell_command(&arch).contains("update-ca-trust"));
        assert!(ca_update_shell_command(&arch).contains("trust extract-compat"));
        assert_eq!(ca_update_shell_command(&debian), "'update-ca-certificates'");
        assert_eq!(
            ca_update_shell_command(&fedora),
            "'update-ca-trust' 'extract'"
        );
    }

    #[test]
    fn linux_trust_install_and_verify_via_helper_with_openssl_and_nss() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = arch_target(dir.path());
        let nss_dir = dir.path().join("nssdb");
        std::fs::create_dir_all(&nss_dir).unwrap();
        let (cert_path, cert_pem, _cert_der) = write_test_ca(dir.path(), "ca.pem");

        let runner = FakeRunner::with(vec![
            ok(), // certutil -D
            ok(), // certutil -A
            ok(), // openssl verify
        ]);
        let helper = FakeLinuxHelper::writing_to(target.clone());
        let admin = FakeAdmin::succeeding();
        let trust = LinuxTrustStore::new(runner.clone())
            .with_helper(helper.clone())
            .with_admin(admin.clone())
            .with_trust_target(target.clone())
            .with_nss_db_dir(Some(nss_dir.clone()))
            .with_openssl(Some(PathBuf::from("/usr/bin/openssl")))
            .with_certutil(Some(PathBuf::from("/usr/bin/certutil")));

        assert_eq!(trust.verify(&cert_path), TrustState::NotTrusted);

        trust.apply(&cert_path).unwrap();
        assert!(
            admin.calls().is_empty(),
            "helper path should not use pkexec"
        );

        let installed = helper.installed.lock().unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].name, LINUX_CA_ANCHOR_NAME);
        assert_eq!(installed[0].cert_pem, cert_pem);
        drop(installed);

        let anchor = LinuxTrustStore::anchor_path_for(&target, LINUX_CA_ANCHOR_NAME);
        assert_eq!(std::fs::read(&anchor).unwrap(), cert_pem);

        let calls = runner.calls();
        assert_eq!(calls.len(), 3);
        let db = format!("sql:{}", nss_dir.to_string_lossy());
        assert_eq!(
            calls[0],
            (
                "/usr/bin/certutil".into(),
                vec![
                    "-d".into(),
                    db.clone(),
                    "-D".into(),
                    "-n".into(),
                    AUTHORITY_NAME.into(),
                ]
            )
        );
        assert_eq!(
            calls[1],
            (
                "/usr/bin/certutil".into(),
                vec![
                    "-d".into(),
                    db,
                    "-A".into(),
                    "-t".into(),
                    "C,,".into(),
                    "-n".into(),
                    AUTHORITY_NAME.into(),
                    "-i".into(),
                    cert_path.to_string_lossy().into_owned(),
                ]
            )
        );
        assert_eq!(
            calls[2],
            (
                "/usr/bin/openssl".into(),
                vec!["verify".into(), cert_path.to_string_lossy().into_owned()]
            )
        );

        // Calling apply again when already trusted is a no-op.
        trust.apply(&cert_path).unwrap();
        assert_eq!(helper.installed.lock().unwrap().len(), 1);
    }

    #[test]
    fn linux_trust_falls_back_to_pkexec_when_helper_not_installed() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = fedora_target(dir.path());
        let (cert_path, _cert_pem, cert_der) = write_test_ca(dir.path(), "ca.pem");
        let anchor = LinuxTrustStore::anchor_path_for(&target, LINUX_CA_ANCHOR_NAME);

        let helper = FakeLinuxHelper::not_installed(target.clone());
        let install_admin = FakeAdmin::copying(cert_path.clone(), anchor.clone());
        let runner = FakeRunner::with(vec![]);
        let trust = LinuxTrustStore::new(runner)
            .with_helper(helper.clone())
            .with_admin(install_admin.clone())
            .with_trust_target(target.clone())
            .with_nss_db_dir(None)
            .with_openssl(None)
            .with_certutil(None);

        trust.apply(&cert_path).unwrap();
        assert_eq!(trust.verify(&cert_path), TrustState::Trusted);
        let install_calls = install_admin.calls();
        assert_eq!(install_calls.len(), 1);
        assert_eq!(install_calls[0].0, "/bin/sh");
        assert_eq!(install_calls[0].1[0], "-c");
        assert!(install_calls[0].1[1].contains("install -m 0644 -o root -g root"));
        assert!(install_calls[0].1[1].contains("'update-ca-trust' 'extract'"));

        // Remove via pkexec fallback too.
        let remove_admin = FakeAdmin::removing(anchor.clone());
        let trust = trust.with_admin(remove_admin.clone());
        trust.remove(&cert_path, &cert_der).unwrap();
        assert!(!anchor.exists());
        let remove_calls = remove_admin.calls();
        assert_eq!(remove_calls.len(), 1);
        assert!(remove_calls[0].1[1].contains("rm -f"));
        assert!(remove_calls[0].1[1].contains("'update-ca-trust' 'extract'"));
    }

    #[test]
    fn linux_trust_maps_pkexec_cancel_to_cancelled_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = debian_target(dir.path());
        let (cert_path, _cert_pem, _cert_der) = write_test_ca(dir.path(), "ca.pem");

        let trust = LinuxTrustStore::new(FakeRunner::with(vec![]))
            .with_helper(FakeLinuxHelper::not_installed(target.clone()))
            .with_admin(FakeAdmin::failing(AdminError::Canceled))
            .with_trust_target(target)
            .with_nss_db_dir(None)
            .with_openssl(None)
            .with_certutil(None);

        assert!(matches!(trust.apply(&cert_path), Err(CaError::Cancelled)));
    }

    #[test]
    fn linux_trust_verify_handles_openssl_mismatch_and_timeout() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = debian_target(dir.path());
        std::fs::create_dir_all(&target.anchor_dir).unwrap();
        let (cert_a, cert_a_pem, _der_a) = write_test_ca(dir.path(), "ca-a.pem");
        let (cert_b, _cert_b_pem, _der_b) = write_test_ca(dir.path(), "ca-b.pem");
        let anchor = LinuxTrustStore::anchor_path_for(&target, LINUX_CA_ANCHOR_NAME);
        std::fs::write(&anchor, &cert_a_pem).unwrap();

        // Different certificate than the installed anchor -> NotTrusted without invoking openssl.
        let runner = FakeRunner::with(vec![]);
        let trust = LinuxTrustStore::new(runner.clone())
            .with_trust_target(target.clone())
            .with_openssl(Some(PathBuf::from("/usr/bin/openssl")))
            .with_certutil(None)
            .with_nss_db_dir(None);
        assert_eq!(trust.verify(&cert_b), TrustState::NotTrusted);
        assert!(runner.calls().is_empty());

        // Matching anchor + openssl verify fails -> NotTrusted.
        let fail_runner = FakeRunner::with(vec![Err(CommandError::new(
            "/usr/bin/openssl",
            vec![],
            "error 18 at 0 depth lookup",
        ))]);
        let trust = LinuxTrustStore::new(fail_runner)
            .with_trust_target(target.clone())
            .with_openssl(Some(PathBuf::from("/usr/bin/openssl")))
            .with_certutil(None)
            .with_nss_db_dir(None);
        assert_eq!(trust.verify(&cert_a), TrustState::NotTrusted);

        // Matching anchor + openssl timeout -> Unknown.
        let mut timeout_err = CommandError::new("/usr/bin/openssl", vec![], "timed out");
        timeout_err.timed_out = true;
        let timeout_runner = FakeRunner::with(vec![Err(timeout_err)]);
        let trust = LinuxTrustStore::new(timeout_runner)
            .with_trust_target(target)
            .with_openssl(Some(PathBuf::from("/usr/bin/openssl")))
            .with_certutil(None)
            .with_nss_db_dir(None);
        assert_eq!(trust.verify(&cert_a), TrustState::Unknown);
    }

    #[test]
    fn linux_trust_ignores_missing_or_failing_certutil() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = arch_target(dir.path());
        let nss_dir = dir.path().join("nssdb");
        std::fs::create_dir_all(&nss_dir).unwrap();
        let (cert_path, _cert_pem, cert_der) = write_test_ca(dir.path(), "ca.pem");

        let runner = FakeRunner::with(vec![
            Err(CommandError::new("certutil", vec![], "nss error")),
            Err(CommandError::new("certutil", vec![], "nss error")),
            Err(CommandError::new("certutil", vec![], "nss error")),
        ]);
        let helper = FakeLinuxHelper::writing_to(target.clone());
        let trust = LinuxTrustStore::new(runner)
            .with_helper(helper.clone())
            .with_trust_target(target)
            .with_nss_db_dir(Some(nss_dir))
            .with_openssl(None)
            .with_certutil(Some(PathBuf::from("/usr/bin/certutil")));

        trust.apply(&cert_path).unwrap();
        trust.remove(&cert_path, &cert_der).unwrap();
        assert_eq!(helper.installed.lock().unwrap().len(), 1);
        assert_eq!(helper.removed.lock().unwrap().len(), 1);
    }

    #[test]
    fn linux_trust_remove_preserves_newer_active_anchor() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = arch_target(dir.path());
        std::fs::create_dir_all(&target.anchor_dir).unwrap();
        let (old_path, _old_pem, old_der) = write_test_ca(dir.path(), "old-ca.pem");
        let (_new_path, new_pem, _new_der) = write_test_ca(dir.path(), "new-ca.pem");
        let anchor = LinuxTrustStore::anchor_path_for(&target, LINUX_CA_ANCHOR_NAME);
        std::fs::write(&anchor, &new_pem).unwrap();

        let helper = FakeLinuxHelper::writing_to(target.clone());
        let trust = LinuxTrustStore::new(FakeRunner::with(vec![]))
            .with_helper(helper.clone())
            .with_trust_target(target)
            .with_nss_db_dir(None)
            .with_openssl(None)
            .with_certutil(None);

        trust.remove(&old_path, &old_der).unwrap();
        assert!(
            helper.removed.lock().unwrap().is_empty(),
            "removing archived trust must not delete a different active anchor"
        );
        assert_eq!(std::fs::read(&anchor).unwrap(), new_pem);
    }

    #[test]
    fn linux_trust_apply_errors_when_post_install_verification_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = arch_target(dir.path());
        let (cert_path, _cert_pem, _cert_der) = write_test_ca(dir.path(), "ca.pem");

        let helper = FakeLinuxHelper::writing_to(target.clone());
        let runner = FakeRunner::with(vec![Err(CommandError::new(
            "/usr/bin/openssl",
            vec![],
            "still untrusted",
        ))]);
        let trust = LinuxTrustStore::new(runner)
            .with_helper(helper)
            .with_trust_target(target)
            .with_nss_db_dir(None)
            .with_openssl(Some(PathBuf::from("/usr/bin/openssl")))
            .with_certutil(None);

        let error = trust.apply(&cert_path).unwrap_err();
        assert!(matches!(error, CaError::Untrusted(_)));
    }
}
