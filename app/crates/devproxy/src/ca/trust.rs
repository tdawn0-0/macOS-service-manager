//! Keychain trust through `/usr/bin/security`.
//!
//! The Go implementation deliberately shells out: no supported Rust API can
//! add a root without mutating the keychain differently or prompting twice.
//! The CLI stays, injected through devcore's [`ProcessRunner`] so tests assert
//! the exact argv and never touch the real keychain.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use devcore::launchd::{CommandError, ProcessRunner};

use super::CaError;
use super::material::{TrustState, sha1_fingerprint};

/// Phrases macOS prints when a certificate is already absent.
const ITEM_MISSING: &str = "the specified item could not be found in the keychain";

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
}
