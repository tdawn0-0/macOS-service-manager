//! Database and user inventory plus drop operations, ported from
//! `database_inventory.go`, `database_drop.go` and the create-user half of
//! `database_other.go`.
//!
//! Reads are one-shot queries against a running instance through the engine's
//! command runner (PostgreSQL/MySQL) or the injected protocol seams
//! (MongoDB/Redis). Drops take the instance lease and reject every reserved
//! system database and built-in account before touching the server.

use std::collections::BTreeSet;

use super::data::{sql_identifier, sql_literal};
use super::{Cancelled, DatabaseError, Databases};

/// The databases and users currently present in a running instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceCatalog {
    pub databases: Vec<String>,
    pub users: Vec<String>,
}

/// Whether `name` is empty or a reserved system database.
pub fn is_protected_database(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "" | "postgres"
            | "template0"
            | "template1"
            | "mysql"
            | "sys"
            | "information_schema"
            | "performance_schema"
            | "admin"
            | "local"
            | "config"
    )
}

/// Whether `name` is empty or a built-in/admin account that must never drop.
pub fn is_protected_database_user(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return true;
    }
    let user = trimmed.split('@').next().unwrap_or_default();
    let lower = user.trim().to_lowercase();
    if lower.is_empty() || lower.starts_with("pg_") {
        return true;
    }
    matches!(
        lower.as_str(),
        "devmanager" | "root" | "default" | "mysql.sys" | "mysql.session" | "mysql.infoschema"
    )
}

pub(crate) fn split_catalog_lines(raw: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in raw.lines() {
        let value = line.trim();
        if value.is_empty() || !seen.insert(value.to_string()) {
            continue;
        }
        out.push(value.to_string());
    }
    out
}

pub(crate) fn redis_db_name(name: &str) -> Result<i64, DatabaseError> {
    match name.parse::<i64>() {
        Ok(value) if (0..=15).contains(&value) => Ok(value),
        _ => Err(DatabaseError::invalid(
            "Redis database number must be between 0 and 15",
        )),
    }
}

fn pg_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

impl Databases {
    /// `InspectInstanceCatalog`: query databases and users of a running
    /// instance. The caller supplies the same five-second intent Go uses.
    pub fn inspect_instance_catalog(
        &self,
        id: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<InstanceCatalog, DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        let state = self.store().read()?;
        let instance = state
            .instances
            .get(id)
            .cloned()
            .ok_or_else(|| DatabaseError::invalid("database instance not found"))?;
        match instance.kind.as_str() {
            "postgresql" => {
                let databases = self.database_command(
                    &state,
                    &instance,
                    "psql",
                    &[
                        "-X".to_string(),
                        "-d".to_string(),
                        "postgres".to_string(),
                        "-Atc".to_string(),
                        "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname".to_string(),
                    ],
                    None,
                    cancelled,
                )?;
                let users = self.database_command(
                    &state,
                    &instance,
                    "psql",
                    &[
                        "-X".to_string(),
                        "-d".to_string(),
                        "postgres".to_string(),
                        "-Atc".to_string(),
                        "SELECT rolname FROM pg_roles WHERE rolcanlogin = true AND rolname NOT LIKE 'pg_%' ORDER BY rolname".to_string(),
                    ],
                    None,
                    cancelled,
                )?;
                Ok(InstanceCatalog {
                    databases: split_catalog_lines(&databases),
                    users: split_catalog_lines(&users),
                })
            }
            "mysql" => {
                let databases = self.mysql_sql(
                    &state,
                    &instance,
                    "SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('information_schema', 'performance_schema', 'mysql', 'sys') ORDER BY schema_name;",
                    cancelled,
                )?;
                let users = self.mysql_sql(
                    &state,
                    &instance,
                    "SELECT DISTINCT user FROM mysql.user WHERE user NOT IN ('mysql.infoschema', 'mysql.session', 'mysql.sys') ORDER BY user;",
                    cancelled,
                )?;
                Ok(InstanceCatalog {
                    databases: split_catalog_lines(&databases),
                    users: split_catalog_lines(&users),
                })
            }
            "mongodb" => self
                .mongo()
                .catalog(instance.port, &self.database_password_from_state(id)?),
            "redis" => Ok(InstanceCatalog {
                databases: self
                    .redis()
                    .keyspace_databases(instance.port, &self.database_password_from_state(id)?)?,
                users: vec!["default".to_string()],
            }),
            _ => Err(DatabaseError::invalid("unsupported database engine")),
        }
    }

    /// Drop a non-system database on a running instance.
    pub fn drop_database(
        &self,
        id: &str,
        name: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(DatabaseError::invalid("database name is required"));
        }
        if is_protected_database(name) {
            return Err(DatabaseError::invalid(format!(
                "cannot drop protected system database {name:?}"
            )));
        }
        if !crate::model::valid_id(name) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {name:?}"
            )));
        }
        self.with_database_operation(id, cancelled, |state, instance| {
            match instance.kind.as_str() {
                "postgresql" => {
                    self.database_command(
                        state,
                        instance,
                        "psql",
                        &[
                            "-X".to_string(),
                            "-d".to_string(),
                            "postgres".to_string(),
                            "--set".to_string(),
                            "ON_ERROR_STOP=1".to_string(),
                            "-c".to_string(),
                            format!("DROP DATABASE {}", pg_identifier(name)),
                        ],
                        None,
                        cancelled,
                    )?;
                    Ok(())
                }
                "mysql" => {
                    self.mysql_sql(
                        state,
                        instance,
                        &format!("DROP DATABASE {}", sql_identifier(name)),
                        cancelled,
                    )?;
                    Ok(())
                }
                "mongodb" => self.mongo().drop_database(
                    instance.port,
                    &self.database_password_from_state(&instance.id)?,
                    name,
                ),
                "redis" => Err(DatabaseError::invalid(
                    "Redis uses numbered databases; database drop is not available",
                )),
                _ => Err(DatabaseError::invalid("unsupported database engine")),
            }
        })
    }

    /// Drop a non-admin user/role on a running instance.
    pub fn drop_user(
        &self,
        id: &str,
        username: &str,
        database: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        let username = username.trim();
        let database = database.trim();
        if username.is_empty() {
            return Err(DatabaseError::invalid("username is required"));
        }
        if is_protected_database_user(username) {
            return Err(DatabaseError::invalid(format!(
                "cannot drop protected built-in user {username:?}"
            )));
        }
        if !database.is_empty() && !crate::model::valid_id(database) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {database:?}"
            )));
        }
        if let Some((user, db)) = username.split_once('@') {
            if !crate::model::valid_id(user) {
                return Err(DatabaseError::invalid(format!(
                    "invalid identifier {user:?}"
                )));
            }
            if db.is_empty() {
                return Err(DatabaseError::invalid(format!(
                    "invalid identifier {username:?}"
                )));
            }
            if !crate::model::valid_id(db) {
                return Err(DatabaseError::invalid(format!("invalid identifier {db:?}")));
            }
        } else if !crate::model::valid_id(username) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {username:?}"
            )));
        }
        self.with_database_operation(id, cancelled, |state, instance| {
            match instance.kind.as_str() {
                "postgresql" => {
                    if username.contains('@') {
                        return Err(DatabaseError::invalid(format!(
                            "invalid identifier {username:?}"
                        )));
                    }
                    let role = pg_identifier(username);
                    let first = self.database_command(
                        state,
                        instance,
                        "psql",
                        &[
                            "-X".to_string(),
                            "-d".to_string(),
                            "postgres".to_string(),
                            "--set".to_string(),
                            "ON_ERROR_STOP=1".to_string(),
                            "-c".to_string(),
                            format!("DROP ROLE {role}"),
                        ],
                        None,
                        cancelled,
                    );
                    match first {
                        Ok(_) => Ok(()),
                        Err(error)
                            if !database.is_empty()
                                && error.to_string().contains("cannot be dropped") =>
                        {
                            let cleanup = self.database_command(
                                state,
                                instance,
                                "psql",
                                &[
                                    "-X".to_string(),
                                    "-d".to_string(),
                                    database.to_string(),
                                    "--set".to_string(),
                                    "ON_ERROR_STOP=1".to_string(),
                                    "-c".to_string(),
                                    format!(
                                        "REASSIGN OWNED BY {role} TO devmanager; DROP OWNED BY {role}"
                                    ),
                                ],
                                None,
                                cancelled,
                            );
                            if cleanup.is_ok() {
                                self.database_command(
                                    state,
                                    instance,
                                    "psql",
                                    &[
                                        "-X".to_string(),
                                        "-d".to_string(),
                                        "postgres".to_string(),
                                        "--set".to_string(),
                                        "ON_ERROR_STOP=1".to_string(),
                                        "-c".to_string(),
                                        format!("DROP ROLE {role}"),
                                    ],
                                    None,
                                    cancelled,
                                )?;
                                return Ok(());
                            }
                            Err(error)
                        }
                        Err(error) => Err(error),
                    }
                }
                "mysql" => {
                    if username.contains('@') {
                        return Err(DatabaseError::invalid(format!(
                            "invalid identifier {username:?}"
                        )));
                    }
                    let hosts = self.mysql_sql(
                        state,
                        instance,
                        &format!(
                            "SELECT host FROM mysql.user WHERE user = {};",
                            sql_literal(username)
                        ),
                        cancelled,
                    )?;
                    let hosts = split_catalog_lines(&hosts);
                    if hosts.is_empty() {
                        self.mysql_sql(
                            state,
                            instance,
                            &format!("DROP USER {}@'localhost'", sql_literal(username)),
                            cancelled,
                        )?;
                        return Ok(());
                    }
                    let targets: Vec<String> = hosts
                        .iter()
                        .map(|host| format!("{}@{}", sql_literal(username), sql_literal(host)))
                        .collect();
                    self.mysql_sql(
                        state,
                        instance,
                        &format!("DROP USER {}", targets.join(", ")),
                        cancelled,
                    )?;
                    Ok(())
                }
                "mongodb" => {
                    let (user, mut db) = match username.split_once('@') {
                        Some((user, at_db)) => (user.to_string(), at_db.to_string()),
                        None => (username.to_string(), database.to_string()),
                    };
                    if db.is_empty() {
                        db = "admin".to_string();
                    }
                    if !crate::model::valid_id(&user) || !crate::model::valid_id(&db) {
                        return Err(DatabaseError::invalid("invalid identifier"));
                    }
                    self.mongo().drop_user(
                        instance.port,
                        &self.database_password_from_state(&instance.id)?,
                        &user,
                        &db,
                    )
                }
                "redis" => Err(DatabaseError::invalid(
                    "Redis uses a single default user; user drop is not available",
                )),
                _ => Err(DatabaseError::invalid("unsupported database engine")),
            }
        })
    }
}
