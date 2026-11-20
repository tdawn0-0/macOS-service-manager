//! Credential-free connection profiles and credential files, ported from
//! `connection_profiles.go` and `connections.go`.
//!
//! Profiles never carry a password; the URI is built from the instance port
//! and the profile's address metadata only. The password lives in
//! `instances/<id>/password` (0600); MySQL also keeps
//! `instances/<id>/client.cnf` (0600). The Terminal launcher reads the file
//! inside the shell script, so the secret never appears in `argv`.

use std::fs;
use std::path::PathBuf;

use url::Url;

use crate::fs::read_regular_file;
use crate::model::{ConnectionProfile, validate_connection_profile};

use super::{Cancelled, DatabaseError, Databases};

/// A password-free connection descriptor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionInfo {
    pub uri: String,
    pub username: String,
    pub credential_file: String,
}

impl ConnectionInfo {
    /// Replace the database path when `db` is valid for the engine.
    pub fn with_database(&self, kind: &str, db: &str) -> Self {
        let db = db.trim();
        if db.is_empty() {
            return self.clone();
        }
        if kind == "redis" {
            if db
                .parse::<i64>()
                .map_or(true, |value| !(0..=15).contains(&value))
            {
                return self.clone();
            }
        } else if !crate::model::valid_id(db) {
            return self.clone();
        }
        let Ok(mut url) = Url::parse(&self.uri) else {
            return self.clone();
        };
        url.set_path(&format!("/{db}"));
        ConnectionInfo {
            uri: url.to_string(),
            ..self.clone()
        }
    }

    /// `DATABASE_URL=<uri>`.
    pub fn env_assignment(&self) -> String {
        format!("DATABASE_URL={}", self.uri)
    }

    /// Engine-appropriate `.env` variable.
    pub fn env_assignment_for_kind(&self, kind: &str) -> String {
        match kind {
            "redis" => format!("REDIS_URL={}", self.uri),
            "mongodb" => format!("MONGODB_URI={}", self.uri),
            _ => self.env_assignment(),
        }
    }

    /// Password-free JDBC URL for PostgreSQL/MySQL.
    pub fn jdbc_url(&self, kind: &str) -> Result<String, DatabaseError> {
        if kind != "postgresql" && kind != "mysql" {
            return Err(DatabaseError::invalid(
                "JDBC URLs are supported only for PostgreSQL and MySQL",
            ));
        }
        let Ok(url) = Url::parse(&self.uri) else {
            return Err(DatabaseError::invalid("invalid database connection URI"));
        };
        let Some(host) = url.host_str() else {
            return Err(DatabaseError::invalid("invalid database connection URI"));
        };
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        let db = url.path().trim_start_matches('/');
        let mut out = format!("jdbc:{kind}://{host}/{db}");
        if !url.username().is_empty() {
            out.push_str("?user=");
            out.push_str(&encode_query_value(url.username()));
        }
        Ok(out)
    }
}

fn encode_query_value(value: &str) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("user", value);
    serializer.finish().trim_start_matches("user=").to_string()
}

fn build_uri(
    port: i64,
    scheme: &str,
    username: &str,
    path: &str,
    query: &[(&str, String)],
) -> Result<String, DatabaseError> {
    let mut url = Url::parse(&format!("{scheme}://placeholder/"))
        .map_err(|_| DatabaseError::invalid("invalid database connection URI"))?;
    url.set_host(Some("127.0.0.1"))
        .map_err(|_| DatabaseError::invalid("invalid database connection URI"))?;
    url.set_port(Some(u16::try_from(port).map_err(|_| {
        DatabaseError::invalid("invalid database connection port")
    })?))
    .map_err(|_| DatabaseError::invalid("invalid database connection port"))?;
    url.set_username(username)
        .map_err(|_| DatabaseError::invalid("invalid database connection URI"))?;
    url.set_path(path);
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

impl Databases {
    /// Save (or replace) a credential-free profile owned by an instance.
    pub fn save_connection_profile(
        &self,
        mut profile: ConnectionProfile,
    ) -> Result<ConnectionProfile, DatabaseError> {
        if profile.id.is_empty() {
            profile.id = crate::software::random_id();
        }
        let saved = profile.clone();
        self.store().with_state(|state| {
            let instance = state
                .instances
                .get(&profile.instance_id)
                .ok_or_else(|| crate::state::StateError::invalid("database instance not found"))?;
            validate_connection_profile(&profile, &instance.kind)?;
            if let Some(previous) = state.connection_profiles.get(&profile.id)
                && previous.instance_id != profile.instance_id
            {
                return Err(crate::state::StateError::invalid(
                    "connection profile belongs to another instance",
                ));
            }
            state
                .connection_profiles
                .insert(profile.id.clone(), profile);
            Ok(())
        })?;
        Ok(saved)
    }

    /// Delete a profile that belongs to `instance_id`.
    pub fn delete_connection_profile(
        &self,
        instance_id: &str,
        profile_id: &str,
    ) -> Result<(), DatabaseError> {
        self.store().with_state(|state| {
            match state.connection_profiles.get(profile_id) {
                Some(profile) if profile.instance_id == instance_id => {}
                _ => {
                    return Err(crate::state::StateError::invalid(
                        "connection profile not found",
                    ));
                }
            }
            state.connection_profiles.remove(profile_id);
            Ok(())
        })?;
        Ok(())
    }

    /// The password-free URI and username for a saved profile.
    pub fn profile_connection_info(
        &self,
        instance_id: &str,
        profile_id: &str,
    ) -> Result<ConnectionInfo, DatabaseError> {
        let state = self.store().read()?;
        let instance = state
            .instances
            .get(instance_id)
            .filter(|instance| (1024..=65535).contains(&instance.port))
            .ok_or_else(|| DatabaseError::invalid("database instance unavailable"))?;
        let profile = state
            .connection_profiles
            .get(profile_id)
            .filter(|profile| profile.instance_id == instance_id)
            .ok_or_else(|| DatabaseError::invalid("connection profile not found"))?;
        validate_connection_profile(profile, &instance.kind)?;
        let (uri, username) = match instance.kind.as_str() {
            "mysql" | "postgresql" => (
                build_uri(
                    instance.port,
                    &instance.kind,
                    &profile.username,
                    &format!("/{}", profile.database),
                    &[],
                )?,
                profile.username.clone(),
            ),
            "mongodb" => (
                build_uri(
                    instance.port,
                    "mongodb",
                    &profile.username,
                    &format!("/{}", profile.database),
                    &[
                        ("authSource", profile.mongo_auth_source.clone()),
                        ("directConnection", "true".to_string()),
                    ],
                )?,
                profile.username.clone(),
            ),
            "redis" => (
                build_uri(
                    instance.port,
                    "redis",
                    "default",
                    &format!("/{}", profile.redis_db),
                    &[],
                )?,
                "default".to_string(),
            ),
            _ => return Err(DatabaseError::invalid("unsupported database engine")),
        };
        Ok(ConnectionInfo {
            uri,
            username,
            credential_file: String::new(),
        })
    }

    /// The default (owner) connection information for an instance.
    pub fn connection_info(&self, id: &str) -> Result<ConnectionInfo, DatabaseError> {
        let state = self.store().read()?;
        let instance = state
            .instances
            .get(id)
            .ok_or_else(|| DatabaseError::invalid("database instance not found"))?;
        if !crate::model::valid_id(&instance.id) {
            return Err(DatabaseError::invalid("invalid identifier"));
        }
        if !(1024..=65535).contains(&instance.port) {
            return Err(DatabaseError::invalid("invalid database connection port"));
        }
        let (scheme, username, path, query) = match instance.kind.as_str() {
            "postgresql" => ("postgresql", "devmanager", "/postgres", Vec::new()),
            "mysql" => ("mysql", "root", "", Vec::new()),
            "mongodb" => (
                "mongodb",
                "devmanager",
                "/admin",
                vec![
                    ("authSource", "admin".to_string()),
                    ("directConnection", "true".to_string()),
                ],
            ),
            "redis" => ("redis", "default", "/0", Vec::new()),
            _ => return Err(DatabaseError::invalid("unsupported database engine")),
        };
        let uri = build_uri(instance.port, scheme, username, path, &query)?;
        Ok(ConnectionInfo {
            uri,
            username: username.to_string(),
            credential_file: self
                .store()
                .root()
                .join("instances")
                .join(&instance.id)
                .join("password")
                .display()
                .to_string(),
        })
    }

    /// `Connection`: the password-free display block the UI shows and copies.
    pub fn connection_text(&self, id: &str) -> Result<String, DatabaseError> {
        let info = self.connection_info(id)?;
        Ok(format!(
            "{}\nUsername: {}\nPassword file: {}",
            info.uri, info.username, info.credential_file
        ))
    }

    /// Read the credential file with the Go 4096-byte limit.
    pub fn database_password(&self, id: &str) -> Result<String, DatabaseError> {
        let info = self.connection_info(id)?;
        let path = PathBuf::from(&info.credential_file);
        let directory = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let bytes = match read_regular_file(directory, name, 4096) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(DatabaseError::Io(error));
            }
            Err(_) => {
                return Err(DatabaseError::invalid("invalid database credential file"));
            }
        };
        if bytes.is_empty() {
            return Err(DatabaseError::invalid("invalid database credential file"));
        }
        String::from_utf8(bytes)
            .map_err(|_| DatabaseError::invalid("invalid database credential file"))
    }

    /// Build the POSIX launcher script. The password is read from its file at
    /// run time; it is never interpolated into the script text or argv.
    pub fn database_cli_command(
        &self,
        id: &str,
        profile_id: &str,
        database_override: Option<&str>,
    ) -> Result<String, DatabaseError> {
        if !crate::model::valid_id(id) {
            return Err(DatabaseError::invalid(format!("invalid identifier {id:?}")));
        }
        if !profile_id.is_empty() && !crate::model::valid_id(profile_id) {
            return Err(DatabaseError::invalid(format!(
                "invalid identifier {profile_id:?}"
            )));
        }
        let state = self.store().read()?;
        let instance = state
            .instances
            .get(id)
            .ok_or_else(|| DatabaseError::invalid("database instance not found"))?;
        if !(1024..=65535).contains(&instance.port) {
            return Err(DatabaseError::invalid("invalid database connection port"));
        }
        let profile = if profile_id.is_empty() {
            None
        } else {
            let profile = state
                .connection_profiles
                .get(profile_id)
                .filter(|profile| profile.instance_id == id)
                .ok_or_else(|| DatabaseError::invalid("connection profile not found"))?;
            validate_connection_profile(profile, &instance.kind)?;
            Some(profile)
        };
        let tool = match instance.kind.as_str() {
            "postgresql" => "psql",
            "mysql" => "mysql",
            "redis" => "redis-cli",
            _ => {
                return Err(DatabaseError::invalid(
                    "Terminal CLI is supported for PostgreSQL, MySQL, and Redis instances only",
                ));
            }
        };
        let binary = self.software().binary(&state, &instance.package, tool)?;
        let binary = binary.display().to_string();
        let port = instance.port.to_string();
        let instance_dir = self.store().root().join("instances").join(id);
        let database_override = database_override.map(str::trim).unwrap_or_default();
        let mut script = String::from(
            "#!/bin/sh\nset -eu\nunset PGHOST PGPORT PGUSER PGDATABASE PGOPTIONS PGPASSFILE PGPASSWORD MYSQL_PWD MYSQL_HOST MYSQL_TCP_PORT REDISCLI_AUTH\n",
        );
        match instance.kind.as_str() {
            "postgresql" => match profile {
                None => {
                    let password_file = instance_dir.join("password");
                    require_regular_non_empty(&password_file, "database credential file")?;
                    let mut db = "postgres";
                    if !database_override.is_empty() && crate::model::valid_id(database_override) {
                        db = database_override;
                    }
                    script.push_str(&format!(
                        "PGPASSWORD=\"$(cat {})\"\nexport PGPASSWORD\nexec {} -h 127.0.0.1 -p {} -U devmanager -d {}\n",
                        shell_quote(&password_file.display().to_string()),
                        shell_quote(&binary),
                        shell_quote(&port),
                        shell_quote(db)
                    ));
                }
                Some(profile) => {
                    script.push_str(&format!(
                        "exec {} -h 127.0.0.1 -p {} -U {} -d {}\n",
                        shell_quote(&binary),
                        shell_quote(&port),
                        shell_quote(&profile.username),
                        shell_quote(&profile.database)
                    ));
                }
            },
            "mysql" => match profile {
                None => {
                    let client_cnf = instance_dir.join("client.cnf");
                    require_regular_non_empty(&client_cnf, "MySQL client configuration")?;
                    script.push_str(&format!(
                        "exec {} {}",
                        shell_quote(&binary),
                        shell_quote(&format!("--defaults-file={}", client_cnf.display()))
                    ));
                    if !database_override.is_empty() && crate::model::valid_id(database_override) {
                        script.push(' ');
                        script.push_str(&shell_quote(database_override));
                    }
                    script.push('\n');
                }
                Some(profile) => {
                    script.push_str(&format!(
                        "exec {} --no-defaults --host=127.0.0.1 {} --protocol=tcp {} -p {}\n",
                        shell_quote(&binary),
                        shell_quote(&format!("--port={port}")),
                        shell_quote(&format!("--user={}", profile.username)),
                        shell_quote(&profile.database)
                    ));
                }
            },
            "redis" => {
                let password_file = instance_dir.join("password");
                require_regular_non_empty(&password_file, "database credential file")?;
                let db = match profile {
                    Some(profile) => profile.redis_db.to_string(),
                    None if !database_override.is_empty()
                        && database_override
                            .parse::<i64>()
                            .is_ok_and(|value| (0..=15).contains(&value)) =>
                    {
                        database_override.to_string()
                    }
                    None => "0".to_string(),
                };
                script.push_str(&format!(
                    "REDISCLI_AUTH=\"$(cat {})\"\nexport REDISCLI_AUTH\nexec {} -h 127.0.0.1 -p {} -n {}\n",
                    shell_quote(&password_file.display().to_string()),
                    shell_quote(&binary),
                    shell_quote(&port),
                    shell_quote(&db)
                ));
            }
            _ => {}
        }
        Ok(script)
    }

    /// Write the launcher 0700 and open it in Terminal. The file path, not any
    /// secret, is the only argument.
    pub fn open_database_cli(
        &self,
        id: &str,
        profile_id: &str,
        database_override: Option<&str>,
        cancelled: Cancelled<'_>,
    ) -> Result<(), DatabaseError> {
        if cancelled() {
            return Err(DatabaseError::Cancelled);
        }
        let script = self.database_cli_command(id, profile_id, database_override)?;
        let path = self
            .store()
            .root()
            .join("instances")
            .join(id)
            .join("open-cli.command");
        crate::fs::atomic_write(&path, script.as_bytes(), 0o700)?;
        self.engine().run_command(
            "/usr/bin/open",
            &["-a", "Terminal", &path.display().to_string()],
            cancelled,
        )?;
        Ok(())
    }
}

fn require_regular_non_empty(path: &std::path::Path, label: &str) -> Result<(), DatabaseError> {
    match fs::metadata(path) {
        Ok(info) if info.is_file() && info.len() > 0 => Ok(()),
        Ok(_) => Err(DatabaseError::invalid(format!("{label} unavailable"))),
        Err(error) => Err(DatabaseError::invalid(format!(
            "{label} unavailable: {error}"
        ))),
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
