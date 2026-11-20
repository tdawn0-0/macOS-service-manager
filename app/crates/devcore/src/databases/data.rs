//! Database data operations: create database/user, logical backup and
//! restore, ported from `database.go:300-390`, `database_other.go`,
//! `mongodb_backup.go` and `redis.go`.
//!
//! Every operation holds the `database instance:<id>` resource lease and a
//! shared package lease, re-reads state, and runs with an immutable snapshot
//! (`database_operations.go`). Backups stage into `<path>.partial` (0600),
//! `fsync`, rename into `backups/`, and only then become the operation's
//! artifact. PostgreSQL credentials reach clients through a 0600 `pgpass` file
//! named in `PGPASSFILE`; MySQL uses the existing `client.cnf`. Passwords never
//! appear in `argv`, and SQL that contains a new password arrives on stdin.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::fs::{PRIVATE_FILE, sync_directory};
use crate::model::{Instance, State};
use crate::state::ResourceKey;

use super::locations::verify_data_location;
use super::{Cancelled, DatabaseError, Databases};

/// The largest restore input this slice will read into memory. MongoDB's own
/// file cap is 512 MiB (`mongodb_backup.go`); applying it to every engine keeps
/// one bad file from exhausting the GUI.
pub const MAX_RESTORE_BYTES: u64 = 512 << 20;

impl Databases {
    /// The shared lease/snapshot wrapper Go calls `withDatabaseOperation`.
    pub(crate) fn with_database_operation<R>(
        &self,
        id: &str,
        cancelled: Cancelled<'_>,
        action: impl FnOnce(&State, &Instance) -> Result<R, DatabaseError>,
    ) -> Result<R, DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        self.init()?;
        let _instance = self
            .store()
            .resource_leases_cancellable(&[ResourceKey::new("database instance", id)], cancelled)?;
        let state = self.store().read()?;
        let instance = state
            .instances
            .get(id)
            .cloned()
            .ok_or_else(|| DatabaseError::invalid("database instance not found"))?;
        let _package =
            self.store()
                .package_lease_cancellable(&instance.package, false, cancelled)?;
        let state = self.store().read()?;
        match state.instances.get(id) {
            Some(current) if current == &instance => {}
            _ => {
                return Err(DatabaseError::invalid(
                    "database settings changed; retry with the current instance",
                ));
            }
        }
        verify_data_location(self.store(), &instance)?;
        self.store().emit(&format!("Using database {id}"));
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        action(&state, &instance)
    }

    /// `createDatabase`.
    pub fn create_database(
        &self,
        id: &str,
        name: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(name) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {name:?}"
            )));
        }
        self.with_database_operation(id, cancelled, |state, instance| {
            if instance.kind != "postgresql" {
                return self.other_create_database(state, instance, name, cancelled);
            }
            self.database_command(
                state,
                instance,
                "createdb",
                &["--".to_string(), name.to_string()],
                None,
                cancelled,
            )?;
            Ok(())
        })
    }

    /// `createUser`. The new password is validated, then passed on stdin; it
    /// never enters the operation record or any command line.
    pub fn create_user(
        &self,
        id: &str,
        name: &str,
        password: &str,
        database: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(name) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {name:?}"
            )));
        }
        if !crate::model::valid_id(database) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {database:?}"
            )));
        }
        valid_database_password(password)?;
        self.with_database_operation(id, cancelled, |state, instance| {
            match instance.kind.as_str() {
                "postgresql" => {
                    let identifier = |value: &str| {
                        format!("\"{}\"", value.replace('"', "\"\""))
                    };
                    let literal = |value: &str| format!("'{}'", value.replace('\'', "''"));
                    let role = identifier(name);
                    let sql = format!(
                        "BEGIN; CREATE ROLE {role} LOGIN PASSWORD {}; GRANT ALL PRIVILEGES ON DATABASE {} TO {role}; GRANT USAGE, CREATE ON SCHEMA public TO {role}; GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO {role}; GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO {role}; ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL PRIVILEGES ON TABLES TO {role}; ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL PRIVILEGES ON SEQUENCES TO {role}; COMMIT;",
                        literal(password),
                        identifier(database)
                    );
                    let mysql_args = vec![
                        "-X".to_string(),
                        "-w".to_string(),
                        "-h".to_string(),
                        "127.0.0.1".to_string(),
                        "-p".to_string(),
                        instance.port.to_string(),
                        "-U".to_string(),
                        "devmanager".to_string(),
                        "-d".to_string(),
                        database.to_string(),
                        "--set".to_string(),
                        "ON_ERROR_STOP=1".to_string(),
                    ];
                    let binary = self
                        .software()
                        .binary(state, &instance.package, "psql")?
                        .display()
                        .to_string();
                    self.run_pg(&binary, instance, &mysql_args, Some(sql.as_bytes()), cancelled)
                        .map_err(|error| {
                            DatabaseError::invalid(format!("create user failed: {error}"))
                        })?;
                    Ok(())
                }
                "mysql" => {
                    let sql = format!(
                        "CREATE USER {}@'localhost' IDENTIFIED BY {}; GRANT ALL ON {}.* TO {}@'localhost'",
                        sql_literal(name),
                        sql_literal(password),
                        sql_identifier(database),
                        sql_literal(name)
                    );
                    self.mysql_sql(state, instance, &sql, cancelled)?;
                    Ok(())
                }
                "mongodb" => self.mongo().create_user(
                    instance.port,
                    &self.database_password_from_state(id)?,
                    name,
                    password,
                    database,
                ),
                _ => Err(DatabaseError::invalid(
                    "user management is not available for this engine",
                )),
            }
        })
    }

    /// `backup`: stage, verify, publish and return the final path.
    pub fn backup(
        &self,
        id: &str,
        name: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<String, DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        if !crate::model::valid_id(name) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {name:?}"
            )));
        }
        let mut published: Option<PathBuf> = None;
        self.with_database_operation(id, cancelled, |state, instance| {
            let path = self
                .store()
                .root()
                .join("backups")
                .join(format!("{id}-{name}-{}.dump", crate::software::random_id()));
            let partial = PathBuf::from(format!("{}.partial", path.display()));
            let created = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(PRIVATE_FILE)
                .open(&partial)?;
            drop(created);
            let outcome = self.run_backup(state, instance, name, &partial, cancelled);
            let outcome = match outcome {
                Ok(()) => {
                    if cancelled() {
                        Err(DatabaseError::Cancelled)
                    } else {
                        Ok(())
                    }
                }
                Err(error) => Err(error),
            };
            if let Err(error) = outcome {
                let _ = fs::remove_file(&partial);
                return Err(error);
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&partial)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&partial, &path)?;
            sync_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
            published = Some(path.clone());
            Ok(())
        })?;
        published
            .map(|path| path.display().to_string())
            .ok_or_else(|| DatabaseError::invalid("backup was not published"))
    }

    fn run_backup(
        &self,
        state: &State,
        instance: &Instance,
        name: &str,
        partial: &Path,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        match instance.kind.as_str() {
            "postgresql" => {
                self.database_command(
                    state,
                    instance,
                    "pg_dump",
                    &[
                        "-Fc".to_string(),
                        "-f".to_string(),
                        partial.display().to_string(),
                        "--".to_string(),
                        name.to_string(),
                    ],
                    None,
                    cancelled,
                )?;
                Ok(())
            }
            "mysql" => {
                let binary = self
                    .software()
                    .binary(state, &instance.package, "mysqldump")?;
                let config = self.configuration_directory(instance).join("client.cnf");
                let args = [
                    format!("--defaults-file={}", config.display()),
                    "--single-transaction".to_string(),
                    "--set-gtid-purged=OFF".to_string(),
                    name.to_string(),
                ];
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let output = self.runner().run_with_input(
                    &binary.display().to_string(),
                    &refs,
                    &[],
                    None,
                    Duration::from_secs(5 * 60),
                )?;
                if output.len() as u64 > MAX_RESTORE_BYTES {
                    return Err(DatabaseError::invalid(
                        "database backup exceeds the 512 MiB limit",
                    ));
                }
                let mut file = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .mode(PRIVATE_FILE)
                    .open(partial)?;
                file.write_all(output.as_bytes())?;
                file.sync_all()?;
                Ok(())
            }
            "redis" => {
                let db = super::inventory::redis_db_name(name)?;
                self.redis().backup(
                    instance.port,
                    &self.database_password_from_state(&instance.id)?,
                    db,
                    partial,
                )
            }
            "mongodb" => self.mongo().backup(
                instance.port,
                &self.database_password_from_state(&instance.id)?,
                name,
                partial,
            ),
            _ => Err(DatabaseError::invalid(
                "backup for this engine is not implemented",
            )),
        }
    }

    /// `restore`: recreate the target database and replay `path`.
    pub fn restore(
        &self,
        id: &str,
        name: &str,
        path: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        if !crate::model::valid_id(name) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {name:?}"
            )));
        }
        if path.ends_with(".partial") {
            return Err(DatabaseError::invalid(
                "incomplete backup output cannot be restored",
            ));
        }
        let source = PathBuf::from(path);
        self.with_database_operation(id, cancelled, |state, instance| {
            match instance.kind.as_str() {
                "postgresql" => {
                    self.database_command(
                        state,
                        instance,
                        "createdb",
                        &["--".to_string(), name.to_string()],
                        None,
                        cancelled,
                    )?;
                    self.database_command(
                        state,
                        instance,
                        "pg_restore",
                        &[
                            "--exit-on-error".to_string(),
                            "--single-transaction".to_string(),
                            "-d".to_string(),
                            name.to_string(),
                            source.display().to_string(),
                        ],
                        None,
                        cancelled,
                    )?;
                    Ok(())
                }
                "mysql" => {
                    let info = fs::metadata(&source)?;
                    if !info.is_file() {
                        return Err(DatabaseError::invalid(
                            "MySQL restore input must be a regular file",
                        ));
                    }
                    if info.len() > MAX_RESTORE_BYTES {
                        return Err(DatabaseError::invalid(
                            "MySQL restore input exceeds the 512 MiB limit",
                        ));
                    }
                    let dump = fs::read(&source)?;
                    self.other_create_database(state, instance, name, cancelled)?;
                    let binary = self.software().binary(state, &instance.package, "mysql")?;
                    let config = self.configuration_directory(instance).join("client.cnf");
                    let mut input = format!("USE {};\n", sql_identifier(name)).into_bytes();
                    input.extend_from_slice(&dump);
                    let args = [
                        format!("--defaults-file={}", config.display()),
                        "--batch".to_string(),
                        "--skip-column-names".to_string(),
                    ];
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    self.runner()
                        .run_with_input(
                            &binary.display().to_string(),
                            &refs,
                            &[],
                            Some(&input),
                            Duration::from_secs(5 * 60),
                        )
                        .map_err(|error| {
                            DatabaseError::invalid(format!("MySQL restore failed: {error}"))
                        })?;
                    Ok(())
                }
                "redis" => {
                    let db = super::inventory::redis_db_name(name)?;
                    self.redis().restore(
                        instance.port,
                        &self.database_password_from_state(&instance.id)?,
                        db,
                        &source,
                    )
                }
                "mongodb" => self.mongo().restore(
                    instance.port,
                    &self.database_password_from_state(&instance.id)?,
                    name,
                    &source,
                ),
                _ => Err(DatabaseError::invalid(
                    "restore for this engine is not implemented",
                )),
            }
        })
    }

    pub(crate) fn other_create_database(
        &self,
        state: &State,
        instance: &Instance,
        name: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        match instance.kind.as_str() {
            "mysql" => {
                self.mysql_sql(
                    state,
                    instance,
                    &format!("CREATE DATABASE {}", sql_identifier(name)),
                    cancelled,
                )?;
                Ok(())
            }
            "mongodb" => self.mongo().create_database(
                instance.port,
                &self.database_password_from_state(&instance.id)?,
                name,
            ),
            _ => Err(DatabaseError::invalid(
                "this engine uses numbered databases; no database creation is needed",
            )),
        }
    }

    pub(crate) fn database_password_from_state(&self, id: &str) -> Result<String, DatabaseError> {
        let path = self
            .store()
            .root()
            .join("instances")
            .join(id)
            .join("password");
        let bytes = crate::fs::read_regular_file(
            path.parent().unwrap_or_else(|| Path::new(".")),
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default(),
            4096,
        )?;
        String::from_utf8(bytes)
            .map_err(|_| DatabaseError::invalid("invalid database credential file"))
    }

    /// Run one PostgreSQL client command. The admin password is written to a
    /// private single-use `pgpass` file whose path (not the secret) is in the
    /// environment. `stdin` carries SQL such as a new role's password.
    pub(crate) fn database_command(
        &self,
        state: &State,
        instance: &Instance,
        tool: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        cancelled: Cancelled<'_>,
    ) -> Result<String, DatabaseError> {
        if instance.kind != "postgresql" {
            return Err(DatabaseError::invalid("PostgreSQL instance not found"));
        }
        let binary = self
            .software()
            .binary(state, &instance.package, tool)?
            .display()
            .to_string();
        let mut full = vec![
            "-w".to_string(),
            "-h".to_string(),
            "127.0.0.1".to_string(),
            "-p".to_string(),
            instance.port.to_string(),
            "-U".to_string(),
            "devmanager".to_string(),
        ];
        full.extend(args.iter().cloned());
        self.run_pg(&binary, instance, &full, stdin, cancelled)
            .map_err(|error| DatabaseError::invalid(format!("{tool} failed: {error}")))
    }

    fn run_pg(
        &self,
        binary: &str,
        instance: &Instance,
        args: &[String],
        stdin: Option<&[u8]>,
        cancelled: Cancelled<'_>,
    ) -> Result<String, DatabaseError> {
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let password = self.database_password_from_state(&instance.id)?;
        let pgpass = self
            .store()
            .root()
            .join("instances")
            .join(&instance.id)
            .join(format!(".pgpass-{}", crate::software::random_id()));
        let contents = format!(
            "127.0.0.1:{}:*:devmanager:{}\n",
            instance.port,
            password.replace('\\', "\\\\").replace(':', "\\:")
        );
        let write = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(PRIVATE_FILE)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&pgpass)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()
        })();
        if let Err(error) = write {
            let _ = fs::remove_file(&pgpass);
            return Err(error.into());
        }
        let pgpass_value = pgpass.display().to_string();
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = self.runner().run_with_input(
            binary,
            &refs,
            &[("PGPASSFILE", &pgpass_value)],
            stdin,
            Duration::from_secs(5 * 60),
        );
        let _ = fs::remove_file(&pgpass);
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        result.map_err(|error| DatabaseError::invalid(error.to_string()))
    }

    pub(crate) fn mysql_sql(
        &self,
        state: &State,
        instance: &Instance,
        sql: &str,
        cancelled: Cancelled<'_>,
    ) -> Result<String, DatabaseError> {
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let binary = self.software().binary(state, &instance.package, "mysql")?;
        let config = self.configuration_directory(instance).join("client.cnf");
        let args = [
            format!("--defaults-file={}", config.display()),
            "--batch".to_string(),
            "--skip-column-names".to_string(),
        ];
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.runner()
            .run_with_input(
                &binary.display().to_string(),
                &refs,
                &[],
                Some(sql.as_bytes()),
                Duration::from_secs(5 * 60),
            )
            .map_err(|error| DatabaseError::invalid(format!("MySQL command failed: {error}")))
    }
}

pub(crate) fn valid_database_password(password: &str) -> Result<(), DatabaseError> {
    if password.is_empty() {
        return Err(DatabaseError::invalid("password is required"));
    }
    if password.len() > 256 {
        return Err(DatabaseError::invalid("password exceeds maximum length"));
    }
    if password
        .chars()
        .any(|character| character < '\u{20}' || character == '\u{7f}')
    {
        return Err(DatabaseError::invalid(
            "password cannot contain newline or control characters",
        ));
    }
    Ok(())
}

pub(crate) fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

pub(crate) fn sql_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}
