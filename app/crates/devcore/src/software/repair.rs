//! Package repair, ported from the Go `package_repair.go:112-271`.
//!
//! Repair re-stages the exact installed manifest under the package's exclusive
//! lease, refuses while a database instance or launchd job still uses the
//! package, and publishes transactionally. A failure at any point after the
//! swap restores the previous tree and index; database directories are outside
//! the transaction and are never touched.

use super::{Cancelled, Software, SoftwareError};
use crate::fs::create_private_dir_all;

impl Software {
    /// Repair an installed archive package from its verified archive.
    pub fn repair_package(&self, id: &str, cancelled: Cancelled<'_>) -> Result<(), SoftwareError> {
        self.store().init()?;
        create_private_dir_all(&self.store().root().join("integrity"))?;
        let _lease = self
            .store()
            .package_lease_cancellable(id, true, cancelled)?;
        let state = self.store().read()?;
        let package = state
            .packages
            .get(id)
            .cloned()
            .ok_or_else(|| SoftwareError::invalid("package is not installed"))?;
        if package.id() != id {
            return Err(SoftwareError::invalid(
                "installed package identity mismatch",
            ));
        }
        package.validate()?;
        if !package.formula.is_empty() {
            return Err(SoftwareError::invalid(format!(
                "Homebrew owns this shared installation; stop its users and run brew reinstall {}, then check files again",
                package.formula
            )));
        }
        self.require_package_idle(cancelled, &state, &package)?;
        self.store().emit(&format!("Preparing repair of {id}"));
        self.prepare_package_for_publication(cancelled, &package, true)
    }
}
