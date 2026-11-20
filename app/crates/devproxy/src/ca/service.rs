//! Authority lifecycle: status, prepare, trust, activate, discard, restore.
//!
//! Ported from `authority.go`. Every mutating action runs under `ca.lock` and
//! re-checks the revision the caller reviewed, so a concurrent change cannot
//! silently commit a different certificate.

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::material::{
    AuthorityInfo, AuthorityMaterial, LeafCertificate, TrustState, new_authority, new_leaf,
};
use super::store::{AuthorityStore, CURRENT_POINTER, PREPARED_POINTER, PREVIOUS_POINTER};
use super::{CaError, trust::TrustStore};

/// Whether the HTTPS proxy is verifiably stopped. Certificates are only
/// swapped while no listener can race the swap.
pub trait ProxyStopCheck: Send + Sync {
    fn verify_stopped(&self) -> Result<(), String>;
}

/// A gate that is always satisfied (unit tests of pure transitions).
#[derive(Debug, Clone, Copy, Default)]
pub struct AlwaysStopped;

impl ProxyStopCheck for AlwaysStopped {
    fn verify_stopped(&self) -> Result<(), String> {
        Ok(())
    }
}

/// A gate that reports the given state, for lifecycle tests.
#[derive(Debug, Clone, Copy)]
pub struct FixedProxyGate {
    pub running: bool,
}

impl ProxyStopCheck for FixedProxyGate {
    fn verify_stopped(&self) -> Result<(), String> {
        if self.running {
            Err("HTTPS proxy is still running".into())
        } else {
            Ok(())
        }
    }
}

/// The status document the authority page renders.
#[derive(Debug, Clone)]
pub struct AuthorityStatus {
    pub checked_at: DateTime<Utc>,
    pub active: AuthorityInfo,
    pub prepared: Option<AuthorityInfo>,
    pub previous: Option<AuthorityInfo>,
    pub archives: Vec<AuthorityInfo>,
    pub prepared_is_active: bool,
    pub revision: String,
}

/// The generational authority state machine.
pub struct AuthorityService {
    store: AuthorityStore,
    trust: Arc<dyn TrustStore>,
    stop_check: Arc<dyn ProxyStopCheck>,
    event: Arc<dyn Fn(&str) + Send + Sync>,
}

impl AuthorityService {
    pub fn new(
        store: AuthorityStore,
        trust: Arc<dyn TrustStore>,
        stop_check: Arc<dyn ProxyStopCheck>,
    ) -> Self {
        AuthorityService {
            store,
            trust,
            stop_check,
            event: Arc::new(|_| {}),
        }
    }

    /// Receive the same progress text the Go event sink gets.
    pub fn with_event(mut self, event: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.event = Arc::new(event);
        self
    }

    pub fn store(&self) -> &AuthorityStore {
        &self.store
    }

    /// Return usable CA material, creating the initial generation on demand.
    pub fn load_or_create(&self) -> Result<AuthorityMaterial, CaError> {
        self.store.load_or_create()
    }

    /// Sign a leaf for one local domain.
    pub fn leaf(&self, domain: &str) -> Result<LeafCertificate, CaError> {
        let authority = self.load_or_create()?;
        new_leaf(&authority.cert_pem, &authority.key_pem, domain, Utc::now())
    }

    /// Inspect every generation and compute the revision.
    pub fn status(&self) -> Result<AuthorityStatus, CaError> {
        let mut active = self.store.read_active()?;
        if !active.info.id.is_empty()
            && let Err(error) = self.store.aliases_valid()
        {
            active.info.status = "Invalid".into();
            active.info.usable = false;
            active.info.detail = error.to_string();
        }
        if active.valid_ca {
            active.info.trust = self.trust.verify(&active.info.certificate_path);
        }

        let prepared_id = self
            .store
            .read_pointer(PREPARED_POINTER)?
            .unwrap_or_default();
        let prepared = if prepared_id.is_empty() {
            None
        } else {
            let mut material = self.store.read_generation(&prepared_id)?;
            material.info.trust = self.trust.verify(&material.info.certificate_path);
            Some(material)
        };
        let prepared_material = prepared.clone().unwrap_or_else(empty_material);
        let prepared_is_active = prepared_id == active.info.id;

        let revision = self
            .store
            .revision(&active, &prepared_id, &prepared_material);
        let previous_id = self
            .store
            .read_pointer(PREVIOUS_POINTER)?
            .unwrap_or_default();
        self.store.validate_store()?;
        let mut archived = Vec::new();
        let mut previous = None;
        for id in self.store.generation_ids()? {
            if id == active.info.id || id == prepared_id {
                continue;
            }
            let Ok(item) = self.store.read_generation(&id) else {
                continue;
            };
            if id == previous_id {
                previous = Some(item.info.clone());
            }
            archived.push(item);
        }
        archived.sort_by(|a, b| {
            let a_previous = a.info.id == previous_id;
            let b_previous = b.info.id == previous_id;
            if a_previous != b_previous {
                return if a_previous {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
            b.record
                .created_at
                .cmp(&a.record.created_at)
                .then_with(|| b.info.not_after.cmp(&a.info.not_after))
                .then_with(|| a.info.id.cmp(&b.info.id))
        });

        Ok(AuthorityStatus {
            checked_at: Utc::now(),
            active: active.info.clone(),
            prepared: prepared.map(|material| material.info),
            previous,
            archives: archived.into_iter().map(|item| item.info).collect(),
            prepared_is_active,
            revision,
        })
    }

    /// Create a replacement generation and point `ca-prepared` at it.
    pub fn prepare(&self) -> Result<(), CaError> {
        self.store.with_lock(&|| false, || {
            if self.store.read_pointer(PREPARED_POINTER)?.is_some() {
                return Err(CaError::Invalid(
                    "review or discard the already prepared authority first".into(),
                ));
            }
            let active = self.store.read_active()?;
            let (cert, key) = new_authority(Utc::now())?;
            let id = self
                .store
                .create_generation(&cert, &key, "Replacement", &active.digest)?;
            self.store.write_pointer(PREPARED_POINTER, &id)
        })
    }

    /// Trust the prepared generation without activating it.
    pub fn trust_prepared(&self, revision: &str) -> Result<(), CaError> {
        self.store.with_lock(&|| false, || {
            let (active, prepared, _) = self.checked_revision(revision)?;
            let prepared = prepared.ok_or_else(|| {
                CaError::Invalid("prepare a valid replacement authority first".into())
            })?;
            if !prepared.info.usable {
                return Err(CaError::Invalid(
                    "prepare a valid replacement authority first".into(),
                ));
            }
            if prepared.info.id != active.info.id && prepared.record.base_digest != active.digest {
                return Err(CaError::Invalid(
                    "active authority changed since preparation; prepare a new replacement".into(),
                ));
            }
            self.configure_trust(&prepared, false)
        })
    }

    /// Commit the prepared generation as active.
    pub fn activate(&self, revision: &str) -> Result<(), CaError> {
        self.store.with_lock(&|| false, || {
            let (active, prepared, _) = self.checked_revision(revision)?;
            let prepared = prepared.ok_or_else(|| {
                CaError::Invalid("prepare a valid replacement authority first".into())
            })?;
            if !prepared.info.usable {
                return Err(CaError::Invalid(
                    "prepare a valid replacement authority first".into(),
                ));
            }
            if prepared.info.id == active.info.id {
                self.remove_pointer(PREPARED_POINTER)?;
                return Ok(());
            }
            if prepared.record.base_digest != active.digest {
                return Err(CaError::Invalid(
                    "active authority changed since preparation; prepare a new replacement".into(),
                ));
            }
            if self.trust.verify(&prepared.info.certificate_path) != TrustState::Trusted {
                return Err(CaError::Untrusted(
                    "trust the prepared CA and verify macOS trust before activation".into(),
                ));
            }
            self.require_proxy_stopped()?;

            let mut previous = active.info.id.clone();
            if previous.is_empty() {
                previous = self.store.create_generation(
                    &active.cert_pem,
                    &active.key_pem,
                    "Archived",
                    "",
                )?;
                self.store.write_pointer(CURRENT_POINTER, &previous)?;
            }
            self.store.ensure_aliases(true)?;
            self.store.write_pointer(PREVIOUS_POINTER, &previous)?;
            self.store
                .write_pointer(CURRENT_POINTER, &prepared.info.id)?;
            (self.event)("Authority activation committed");
            if let Err(error) = self.remove_pointer(PREPARED_POINTER) {
                return Err(CaError::Invalid(format!(
                    "authority activated; refresh and finish activation: {error}"
                )));
            }
            Ok(())
        })
    }

    /// Forget the prepared pointer; the certificate stays archived so granted
    /// trust can still be removed.
    pub fn discard_prepared(&self, revision: &str) -> Result<(), CaError> {
        self.store.with_lock(&|| false, || {
            let (_, prepared, _) = self.checked_revision(revision)?;
            if prepared.is_none() {
                return Err(CaError::Invalid("no prepared authority".into()));
            }
            self.remove_pointer(PREPARED_POINTER)
        })
    }

    /// Select a saved generation as active.
    pub fn restore(&self, id: &str, fingerprint: &str, revision: &str) -> Result<(), CaError> {
        self.store.with_lock(&|| false, || {
            let (active, prepared, _) = self.checked_revision(revision)?;
            if prepared.is_some() {
                return Err(CaError::Invalid(
                    "finish or discard the prepared replacement before restoring another authority"
                        .into(),
                ));
            }
            let previous = self.store.read_generation(id)?;
            if !previous.info.usable || previous.info.fingerprint != fingerprint {
                return Err(CaError::Invalid(
                    "selected authority is invalid, expired or changed".into(),
                ));
            }
            if self.trust.verify(&previous.info.certificate_path) != TrustState::Trusted {
                return Err(CaError::Untrusted(
                    "selected authority is not currently trusted by macOS".into(),
                ));
            }
            self.require_proxy_stopped()?;
            let mut active_id = active.info.id.clone();
            if active_id.is_empty() {
                active_id = self.store.create_generation(
                    &active.cert_pem,
                    &active.key_pem,
                    "Archived",
                    "",
                )?;
                self.store.write_pointer(CURRENT_POINTER, &active_id)?;
            }
            self.store.ensure_aliases(true)?;
            self.store.write_pointer(PREVIOUS_POINTER, &active_id)?;
            self.store.write_pointer(CURRENT_POINTER, id)
        })
    }

    /// Install or remove trust for an archived generation.
    pub fn configure_saved_trust(
        &self,
        id: &str,
        fingerprint: &str,
        remove: bool,
    ) -> Result<(), CaError> {
        self.store.with_lock(&|| false, || {
            let active = self
                .store
                .read_pointer(CURRENT_POINTER)?
                .unwrap_or_default();
            let prepared = self
                .store
                .read_pointer(PREPARED_POINTER)?
                .unwrap_or_default();
            if id == active || id == prepared {
                return Err(CaError::Invalid(
                    "use the active or prepared authority controls for this certificate".into(),
                ));
            }
            let archived = self.store.read_generation(id)?;
            if !archived.valid_ca || archived.info.fingerprint != fingerprint {
                return Err(CaError::Invalid(
                    "archived certificate changed; refresh before removing trust".into(),
                ));
            }
            self.configure_trust(&archived, remove)
        })
    }

    /// Remove trust from an archived generation.
    pub fn remove_archived_trust(&self, id: &str, fingerprint: &str) -> Result<(), CaError> {
        self.configure_saved_trust(id, fingerprint, true)
    }

    /// Trust or untrust the active authority (the GUI's one-click action).
    pub fn configure_active_trust(&self, remove: bool) -> Result<(), CaError> {
        if !remove {
            self.store.load_or_create()?;
        }
        self.store.with_lock(&|| false, || {
            let active = self.store.read_active()?;
            if remove && active.cert_der.is_empty() && active.is_not_created() {
                return Ok(());
            }
            self.configure_trust(&active, remove)
        })
    }

    fn checked_revision(
        &self,
        revision: &str,
    ) -> Result<(AuthorityMaterial, Option<AuthorityMaterial>, String), CaError> {
        let active = self.store.read_active()?;
        let prepared_id = self
            .store
            .read_pointer(PREPARED_POINTER)?
            .unwrap_or_default();
        let prepared = if prepared_id.is_empty() {
            None
        } else {
            Some(self.store.read_generation(&prepared_id)?)
        };
        let prepared_material = prepared.clone().unwrap_or_else(empty_material);
        if revision.is_empty()
            || revision
                != self
                    .store
                    .revision(&active, &prepared_id, &prepared_material)
        {
            return Err(CaError::Invalid(
                "authority material changed; refresh and review its fingerprints again".into(),
            ));
        }
        Ok((active, prepared, prepared_id))
    }

    fn require_proxy_stopped(&self) -> Result<(), CaError> {
        self.stop_check.verify_stopped().map_err(|error| {
            CaError::Invalid(format!(
                "stop the HTTPS proxy before switching authorities: {error}"
            ))
        })
    }

    fn configure_trust(&self, material: &AuthorityMaterial, remove: bool) -> Result<(), CaError> {
        if material.cert_der.is_empty() {
            return Err(CaError::Invalid(
                "a readable CA certificate is required".into(),
            ));
        }
        if !material.valid_ca {
            return Err(CaError::Invalid(
                "only this app's stored self-signed authority can be managed here".into(),
            ));
        }
        let path = material.info.certificate_path.as_path();
        if !remove {
            if !material.info.usable {
                return Err(CaError::Invalid(
                    "repair the authority before trusting it".into(),
                ));
            }
            if self.trust.verify(path) == TrustState::Trusted {
                return Ok(());
            }
        }
        if remove {
            self.trust.remove(path, &material.cert_der)
        } else {
            self.trust.apply(path)
        }
    }

    fn remove_pointer(&self, name: &str) -> Result<(), CaError> {
        match std::fs::remove_file(self.store.authority_root().join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        devcore::fs::sync_directory(&self.store.authority_root())?;
        Ok(())
    }
}

impl AuthorityService {
    /// Path used by the trust CLI; exposed for callers that render it.
    pub fn certificate_path(&self, id: &str) -> std::path::PathBuf {
        self.store.generation_dir(id).join("ca.pem")
    }

    /// Whether a path is the active certificate (used by diagnostics).
    pub fn is_active_certificate(&self, path: &Path) -> bool {
        self.store
            .read_pointer(CURRENT_POINTER)
            .ok()
            .flatten()
            .map(|id| self.certificate_path(&id) == path)
            .unwrap_or(false)
    }
}

fn empty_material() -> AuthorityMaterial {
    AuthorityMaterial {
        info: AuthorityInfo::default(),
        cert_pem: Vec::new(),
        key_pem: Vec::new(),
        digest: String::new(),
        record: Default::default(),
        cert_der: Vec::new(),
        valid_ca: false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::*;
    use crate::ca::store::AuthorityStore;

    struct FakeTrust {
        trusted: Mutex<BTreeSet<PathBuf>>,
        applied: Mutex<Vec<PathBuf>>,
    }

    impl FakeTrust {
        fn new() -> Arc<Self> {
            Arc::new(FakeTrust {
                trusted: Mutex::new(BTreeSet::new()),
                applied: Mutex::new(Vec::new()),
            })
        }
    }

    impl TrustStore for FakeTrust {
        fn verify(&self, certificate_path: &Path) -> TrustState {
            if self.trusted.lock().unwrap().contains(certificate_path) {
                TrustState::Trusted
            } else {
                TrustState::NotTrusted
            }
        }

        fn apply(&self, certificate_path: &Path) -> Result<(), CaError> {
            self.trusted
                .lock()
                .unwrap()
                .insert(certificate_path.to_path_buf());
            self.applied
                .lock()
                .unwrap()
                .push(certificate_path.to_path_buf());
            Ok(())
        }

        fn remove(&self, certificate_path: &Path, _der: &[u8]) -> Result<(), CaError> {
            self.trusted.lock().unwrap().remove(certificate_path);
            Ok(())
        }
    }

    fn service(
        gate: bool,
    ) -> (
        TempDir,
        Arc<AuthorityService>,
        Arc<FakeTrust>,
        Arc<Mutex<bool>>,
    ) {
        struct Gate(Arc<Mutex<bool>>);
        impl ProxyStopCheck for Gate {
            fn verify_stopped(&self) -> Result<(), String> {
                if *self.0.lock().unwrap() {
                    Err("HTTPS proxy is still running".into())
                } else {
                    Ok(())
                }
            }
        }
        let dir = TempDir::new().unwrap();
        let trust = FakeTrust::new();
        let running = Arc::new(Mutex::new(gate));
        let service = Arc::new(AuthorityService::new(
            AuthorityStore::new(dir.path()),
            trust.clone(),
            Arc::new(Gate(running.clone())),
        ));
        (dir, service, trust, running)
    }

    #[test]
    fn prepare_trust_activate_and_restore() {
        let (_dir, service, trust, running) = service(false);
        let initial = service.load_or_create().unwrap();
        let status = service.status().unwrap();
        assert_eq!(status.active.status, "Ready");
        assert!(status.prepared.is_none());
        assert!(!status.revision.is_empty());
        assert_ne!(
            trust.verify(&initial.info.certificate_path),
            TrustState::Trusted
        );

        service.prepare().unwrap();
        let prepared = service.status().unwrap();
        let prepared_info = prepared.prepared.clone().expect("prepared");
        assert!(!prepared.prepared_is_active);
        let prepared_material = service.store().read_generation(&prepared_info.id).unwrap();
        assert_eq!(prepared_material.record.kind, "Replacement");
        assert_eq!(prepared_material.record.base_digest, initial.digest);

        let revision = prepared.revision.clone();
        let error = service.activate(&revision).unwrap_err();
        assert!(error.to_string().contains("trust the prepared CA"));

        service.trust_prepared(&revision).unwrap();
        assert_eq!(
            trust.verify(&prepared_info.certificate_path),
            TrustState::Trusted
        );

        *running.lock().unwrap() = true;
        let error = service.activate(&revision).unwrap_err();
        assert!(error.to_string().contains("stop the HTTPS proxy"));
        *running.lock().unwrap() = false;

        service.activate(&revision).unwrap();
        let activated = service.status().unwrap();
        assert_eq!(activated.active.id, prepared_info.id);
        assert!(activated.prepared.is_none());
        assert_eq!(
            activated.previous.as_ref().map(|info| info.id.clone()),
            Some(initial.info.id.clone())
        );
        assert!(
            activated
                .archives
                .iter()
                .any(|info| info.id == initial.info.id)
        );
        assert!(
            service
                .store()
                .read_pointer(PREPARED_POINTER)
                .unwrap()
                .is_none()
        );

        // Restore the previous generation.
        let revision = activated.revision.clone();
        let old = activated
            .archives
            .iter()
            .find(|info| info.id == initial.info.id)
            .unwrap();
        trust
            .trusted
            .lock()
            .unwrap()
            .insert(old.certificate_path.clone());
        service
            .restore(&old.id, &old.fingerprint, &revision)
            .unwrap();
        let restored = service.status().unwrap();
        assert_eq!(restored.active.id, initial.info.id);
        assert_eq!(
            restored.previous.as_ref().map(|info| info.id.clone()),
            Some(prepared_info.id)
        );
    }

    #[test]
    fn revision_mismatch_is_rejected() {
        let (_dir, service, _trust, _running) = service(false);
        service.load_or_create().unwrap();
        service.prepare().unwrap();
        let error = service.trust_prepared("stale").unwrap_err();
        assert!(error.to_string().contains("authority material changed"));
        let revision = service.status().unwrap().revision;
        assert!(service.trust_prepared(&revision).is_ok());
    }

    #[test]
    fn discard_removes_only_the_prepared_pointer() {
        let (_dir, service, _trust, _running) = service(false);
        service.load_or_create().unwrap();
        service.prepare().unwrap();
        let status = service.status().unwrap();
        let prepared = status.prepared.clone().unwrap();
        service.discard_prepared(&status.revision).unwrap();
        let after = service.status().unwrap();
        assert!(after.prepared.is_none());
        assert!(service.store().read_generation(&prepared.id).is_ok());
        assert!(
            after.archives.iter().any(|info| info.id == prepared.id),
            "the discarded generation stays archived"
        );
        assert!(service.discard_prepared(&after.revision).is_err());
    }

    #[test]
    fn third_prepare_is_refused() {
        let (_dir, service, _trust, _running) = service(false);
        service.load_or_create().unwrap();
        service.prepare().unwrap();
        assert!(
            service
                .prepare()
                .unwrap_err()
                .to_string()
                .contains("already prepared")
        );
    }

    #[test]
    fn finishing_an_already_active_prepared_authority_clears_the_pointer() {
        let (_dir, service, trust, _running) = service(false);
        let active = service.load_or_create().unwrap();
        trust
            .trusted
            .lock()
            .unwrap()
            .insert(active.info.certificate_path.clone());
        service
            .store()
            .write_pointer(PREPARED_POINTER, &active.info.id)
            .unwrap();
        let status = service.status().unwrap();
        assert!(status.prepared_is_active);
        service.activate(&status.revision).unwrap();
        assert!(
            service
                .store()
                .read_pointer(PREPARED_POINTER)
                .unwrap()
                .is_none()
        );
        assert_eq!(service.status().unwrap().active.id, active.info.id);
    }

    #[test]
    fn untrusted_archives_cannot_be_restored() {
        let (_dir, service, trust, _running) = service(false);
        let initial = service.load_or_create().unwrap();
        service.prepare().unwrap();
        let status = service.status().unwrap();
        let revision = status.revision.clone();
        service.trust_prepared(&revision).unwrap();
        service.activate(&revision).unwrap();
        let status = service.status().unwrap();
        let previous = status.previous.clone().unwrap();
        assert_ne!(
            trust.verify(&previous.certificate_path),
            TrustState::Trusted
        );
        let error = service
            .restore(&previous.id, &previous.fingerprint, &status.revision)
            .unwrap_err();
        assert!(error.to_string().contains("not currently trusted"));
        assert_ne!(service.status().unwrap().active.id, previous.id);
        assert_eq!(initial.info.name, previous.name);
    }
}
