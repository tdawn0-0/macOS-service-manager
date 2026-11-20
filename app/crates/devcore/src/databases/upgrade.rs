//! Patch upgrades for database instances, ported from
//! `instance_upgrade.go:12-130,330-420`.
//!
//! `UpgradeTargets` is pure: same engine, same architecture, same series, a
//! higher patch or Homebrew revision. `upgrade_instance_package` moves a
//! stopped instance to such a target inside the state transaction, keeps data,
//! port and credentials untouched, and deletes only the generated job file so
//! the next start rewrites it.

use std::fs;
use std::path::Path;

use crate::model::{Instance, State};
use crate::state::{ResourceKey, Tx};

use super::database_work::database_work_records;
use super::lifecycle::verify_shutdown_target;
use super::locations::verify_data_location;
use super::{Cancelled, DatabaseError, Databases};

/// Split an exact `x.y.z` version into `"x.y"` and the patch number.
pub fn patch_series(version: &str) -> Option<(String, i64)> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    if !parts
        .iter()
        .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let patch = parts[2].parse().ok()?;
    Some((format!("{}.{}", parts[0], parts[1]), patch))
}

fn upgradable_kind(kind: &str) -> bool {
    matches!(kind, "mysql" | "mongodb" | "postgresql" | "redis")
}

fn parse_brew_revision(value: &str) -> Option<(String, i64)> {
    match value.split_once('_') {
        None => Some((value.to_string(), 0)),
        Some((base, revision)) => {
            if base.is_empty() || revision.is_empty() {
                return None;
            }
            if !revision.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            Some((base.to_string(), revision.parse().ok()?))
        }
    }
}

fn parse_upgrade_version(kind: &str, version: &str) -> Option<(String, i64, i64)> {
    match kind {
        "mysql" | "mongodb" => {
            let (series, patch) = patch_series(version)?;
            Some((series, patch, 0))
        }
        "postgresql" => {
            let (base, revision) = parse_brew_revision(version)?;
            let parts: Vec<&str> = base.split('.').collect();
            if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
                return None;
            }
            if !parts
                .iter()
                .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
            {
                return None;
            }
            let minor: i64 = parts[1].parse().ok()?;
            Some((parts[0].to_string(), minor, revision))
        }
        "redis" => {
            let (base, revision) = parse_brew_revision(version)?;
            let (series, patch) = patch_series(&base)?;
            Some((series, patch, revision))
        }
        _ => None,
    }
}

/// The installed packages this instance could move to, newest first.
pub fn upgrade_targets(state: &State, instance: &Instance) -> Vec<String> {
    if !upgradable_kind(&instance.kind) {
        return Vec::new();
    }
    let Some(current) = state.packages.get(&instance.package) else {
        return Vec::new();
    };
    let Some((series, patch, revision)) = parse_upgrade_version(&instance.kind, &current.version)
    else {
        return Vec::new();
    };
    let mut found: Vec<(String, i64, i64)> = Vec::new();
    for (id, package) in &state.packages {
        if package.name != instance.kind || package.arch != current.arch {
            continue;
        }
        let Some((candidate_series, candidate_patch, candidate_revision)) =
            parse_upgrade_version(&instance.kind, &package.version)
        else {
            continue;
        };
        if candidate_series != series {
            continue;
        }
        if candidate_patch < patch || (candidate_patch == patch && candidate_revision <= revision) {
            continue;
        }
        found.push((id.clone(), candidate_patch, candidate_revision));
    }
    found.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.0.cmp(&right.0))
    });
    found.into_iter().map(|(id, _, _)| id).collect()
}

/// `NewerListedPatch`: newest listed version in the installed series.
pub fn newer_listed_patch(installed: &str, listed: &[String]) -> String {
    let Some((series, patch)) = patch_series(installed) else {
        return String::new();
    };
    let mut best = String::new();
    let mut best_patch = patch;
    for version in listed {
        if let Some((candidate_series, candidate_patch)) = patch_series(version)
            && candidate_series == series
            && candidate_patch > best_patch
        {
            best = version.clone();
            best_patch = candidate_patch;
        }
    }
    best
}

/// `mysqlStoppedCleanly`: the last server event is a completed shutdown.
fn mysql_stopped_cleanly(root: &Path, id: &str) -> bool {
    let path = root.join("logs").join(format!("db-{id}.err.log"));
    let Ok(file) = fs::File::open(&path) else {
        return false;
    };
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file;
    let length = file.metadata().map(|info| info.len()).unwrap_or(0);
    let start = length.saturating_sub(256 << 10);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return false;
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return false;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().rev().take(2000).collect();
    for line in lines {
        if line.contains("Shutdown complete") {
            return true;
        }
        if line.contains("ready for connections") {
            return false;
        }
    }
    false
}

impl Databases {
    /// Move a stopped instance to a newer installed patch of its series.
    pub fn upgrade_instance_package(
        &self,
        expected: &Instance,
        target: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(&expected.id) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {:?}",
                expected.id
            )));
        }
        if !upgradable_kind(&expected.kind) {
            return Err(DatabaseError::invalid(
                "patch upgrades are available for MySQL, PostgreSQL, Redis, and MongoDB instances only",
            ));
        }
        let _lease = self.store().resource_leases_cancellable(
            &[ResourceKey::new("database instance", &expected.id)],
            cancelled,
        )?;
        let records = database_work_records(self.store().root())
            .map_err(|error| DatabaseError::invalid(error.to_string()))?;
        if records
            .iter()
            .any(|record| record.instance.id == expected.id)
        {
            return Err(DatabaseError::invalid(
                "finish or resolve this instance's unfinished database task before upgrading",
            ));
        }
        self.store()
            .with_state_transaction_cancellable(cancelled, |state| {
                let Some(instance) = state.instances.get(&expected.id).cloned() else {
                    return Err(crate::state::StateError::invalid("instance not found"));
                };
                verify_data_location(self.store(), &instance)
                    .map_err(crate::state::StateError::from)?;
                if &instance != expected {
                    return Err(crate::state::StateError::invalid(
                        "database settings changed; select the instance again before upgrading",
                    ));
                }
                if !upgrade_targets(state, &instance)
                    .iter()
                    .any(|candidate| candidate == target)
                {
                    return Err(crate::state::StateError::invalid(
                        "choose a newer installed patch release of the same series",
                    ));
                }
                let server = match instance.kind.as_str() {
                    "mysql" => Some("mysqld"),
                    "mongodb" => Some("mongod"),
                    "postgresql" => Some("postgres"),
                    "redis" => Some("redis-server"),
                    _ => None,
                };
                let Some(server) = server else {
                    return Err(crate::state::StateError::invalid(
                        "unsupported database engine",
                    ));
                };
                self.software()
                    .binary(state, target, server)
                    .map_err(|error| {
                        crate::state::StateError::from(DatabaseError::from(error))
                    })?;
                verify_shutdown_target(
                    self.engine(),
                    &self.engine().target(&format!("db-{}", instance.id)),
                    cancelled,
                )
                .map_err(|error| {
                    crate::state::StateError::invalid(format!(
                        "stop the database before upgrading: {error}"
                    ))
                })?;
                if instance.kind == "mysql"
                    && !mysql_stopped_cleanly(self.store().root(), &instance.id)
                {
                    return Err(crate::state::StateError::invalid(
                        "could not confirm that MySQL shut down cleanly; start the instance, stop it, then upgrade",
                    ));
                }
                let mut updated = instance.clone();
                updated.package = target.to_string();
                state.instances.insert(instance.id.clone(), updated);
                self.store()
                    .emit(&format!("Moved {} to {}", instance.id, target));
                let job = self.engine().job_path(&format!("db-{}", instance.id));
                let store = self.store().clone();
                let id = instance.id.clone();
                Ok(Tx::new(()).with_commit(move || {
                    if let Err(error) = fs::remove_file(&job)
                        && error.kind() != std::io::ErrorKind::NotFound
                    {
                        store.emit(&format!(
                            "Could not remove the old job file for {id}; the next start replaces it"
                        ));
                    }
                }))
            })
            .map_err(Into::into)
    }
}
