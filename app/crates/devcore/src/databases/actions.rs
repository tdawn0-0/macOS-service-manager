//! The database half of the operation journal's executor, matching the Go
//! wrapper methods in `operations.go:423-464`.
//!
//! The journal owns the record, phase and owner lease; this adapter calls the
//! exact database implementation and forwards the same progress text. Software
//! installs and repairs use the [`crate::software::Software`] facade that
//! shares this store, so one adapter serves the whole journal.

use std::sync::Arc;

use crate::model::Package;
use crate::operations::{OperationActions, OperationScope};
use crate::state::StateError;

use super::{Databases, InstanceAction};

/// The production operation executor for a development home.
#[derive(Clone)]
pub struct DatabaseActions {
    databases: Databases,
}

impl std::fmt::Debug for DatabaseActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseActions").finish()
    }
}

impl DatabaseActions {
    pub fn new(databases: Databases) -> Self {
        DatabaseActions { databases }
    }

    pub fn databases(&self) -> &Databases {
        &self.databases
    }

    fn actions(&self) -> Arc<dyn OperationActions> {
        Arc::new(self.clone())
    }
}

impl OperationActions for DatabaseActions {
    fn install(&self, _scope: &OperationScope<'_>, package: &Package) -> Result<(), StateError> {
        self.databases
            .software()
            .install(package, &|| false)
            .map_err(|error| StateError::from(super::DatabaseError::from(error)))
    }

    fn repair_package(&self, _scope: &OperationScope<'_>, id: &str) -> Result<(), StateError> {
        self.databases
            .software()
            .repair_package(id, &|| false)
            .map_err(|error| StateError::from(super::DatabaseError::from(error)))
    }

    fn create_instance(
        &self,
        _scope: &OperationScope<'_>,
        instance: &crate::model::Instance,
    ) -> Result<(), StateError> {
        self.databases
            .create_instance(instance.clone(), &|| false)
            .map_err(Into::into)
    }

    fn backup(
        &self,
        scope: &OperationScope<'_>,
        id: &str,
        name: &str,
    ) -> Result<String, StateError> {
        let path = self
            .databases
            .backup(id, name, &|| false)
            .map_err(StateError::from)?;
        scope.artifact(&path);
        Ok(path)
    }

    fn restore(
        &self,
        _scope: &OperationScope<'_>,
        id: &str,
        name: &str,
        path: &str,
    ) -> Result<(), StateError> {
        self.databases
            .restore(id, name, path, &|| false)
            .map_err(Into::into)
    }

    fn instance_action(
        &self,
        _scope: &OperationScope<'_>,
        id: &str,
        action: &str,
    ) -> Result<(), StateError> {
        let action = InstanceAction::parse(action)
            .ok_or_else(|| StateError::invalid("unknown instance action"))?;
        self.databases
            .instance_action(id, action, &|| false)
            .map_err(Into::into)
    }

    fn create_database(
        &self,
        _scope: &OperationScope<'_>,
        id: &str,
        name: &str,
    ) -> Result<(), StateError> {
        self.databases
            .create_database(id, name, &|| false)
            .map_err(Into::into)
    }

    fn create_user(
        &self,
        _scope: &OperationScope<'_>,
        id: &str,
        name: &str,
        password: &str,
        database: &str,
    ) -> Result<(), StateError> {
        self.databases
            .create_user(id, name, password, database, &|| false)
            .map_err(Into::into)
    }

    fn resolve_package(&self, name: &str, version: &str) -> Result<Package, StateError> {
        self.databases
            .software()
            .resolve_installed_or_catalog(name, version)
            .map_err(|error| StateError::from(super::DatabaseError::from(error)))
    }

    fn instance_action_direct(&self, id: &str, action: &str) -> Result<(), StateError> {
        let action = InstanceAction::parse(action)
            .ok_or_else(|| StateError::invalid("unknown instance action"))?;
        self.databases
            .instance_action(id, action, &|| false)
            .map_err(Into::into)
    }
}

impl DatabaseActions {
    /// The journal configured with this executor.
    pub fn operation_journal(&self) -> crate::operations::OperationJournal {
        crate::operations::OperationJournal::new(self.databases.store().clone())
            .with_actions(self.actions())
    }
}
