//! `db` (ported from `cmd_databases.go`) over `devcore::databases`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;

use crate::args::{FlagSpecs, ParseOutcome, has_help_flag, parse_flags, read_secret_stdin};
use crate::commands::require_yes;
use crate::env::CliEnv;
use crate::error::{CliError, CliResult};
use crate::output::go_rfc3339_nano_local;

use devcore::databases::{
    Databases, InstanceAction, database_data_directory, upgrade::upgrade_targets,
};
use devcore::model::{ConnectionProfile, Instance};
use devcore::software::Software;

fn databases(env: &CliEnv) -> Databases {
    Databases::new(devcore::state::Store::new(env.root.clone()))
}

fn software(env: &CliEnv) -> Software {
    Software::new(devcore::state::Store::new(env.root.clone()))
}

fn read_state(env: &CliEnv) -> CliResult<devcore::model::State> {
    Ok(devcore::state::Store::new(env.root.clone()).read()?)
}

#[derive(Serialize)]
struct DbView<'a> {
    #[serde(flatten)]
    instance: &'a Instance,
    status: String,
    #[serde(rename = "resolvedDataDirectory")]
    resolved_data_directory: String,
    #[serde(rename = "upgradeTargets", skip_serializing_if = "Vec::is_empty")]
    upgrade_targets: Vec<String>,
}

pub fn run_database_command(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
) -> CliResult<()> {
    if has_help_flag(args) && args.len() == 1 {
        crate::schema::print_topic_help(out, "db")?;
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "list";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    let cancelled: &dyn Fn() -> bool = &|| false;
    match sub {
        "list" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for db list"
                )));
            }
            let state = read_state(env)?;
            let db = databases(env);
            let engine = db.engine();
            let mut views = Vec::new();
            for (id, instance) in &state.instances {
                let (job, error) =
                    devcore::health::read_job_state(engine, &format!("db-{id}"), cancelled);
                let status = if error.is_err() {
                    "Unknown"
                } else if job.running {
                    "Running"
                } else if job.finished() && job.last_exit_code != Some(0) {
                    "Error"
                } else {
                    "Stopped"
                };
                views.push(DbView {
                    instance,
                    status: status.into(),
                    resolved_data_directory: database_data_directory(
                        devcore::state::Store::new(env.root.clone()).root(),
                        instance,
                        None,
                    )
                    .to_string_lossy()
                    .into_owned(),
                    upgrade_targets: upgrade_targets(&state, instance),
                });
            }
            #[derive(Serialize)]
            struct Payload<'a> {
                instances: &'a [DbView<'a>],
            }
            crate::commands::write_result(env, out, &Payload { instances: &views }, |w| {
                if views.is_empty() {
                    writeln!(w, "No database instances configured.")?;
                    return Ok(());
                }
                for view in &views {
                    let storage = if view.instance.data_directory.is_empty() {
                        "isolated".to_string()
                    } else {
                        format!("external ({})", view.instance.data_directory)
                    };
                    writeln!(
                        w,
                        "{}\t{}\t:{}\t{}\t{}\tpkg={}\tpersistent={}",
                        view.instance.id,
                        view.instance.kind,
                        view.instance.port,
                        view.status,
                        storage,
                        view.instance.package,
                        view.instance.persistent
                    )?;
                }
                Ok(())
            })
        }
        "create" => run_create(env, out, rest),
        "start" | "stop" | "restart" => {
            if rest.len() != 1 {
                return Err(CliError::usage(format!("usage: devbase db {sub} <id>")));
            }
            let id = rest[0].trim().to_string();
            let action = if sub == "start" {
                InstanceAction::Start
            } else if sub == "stop" {
                InstanceAction::Stop
            } else {
                InstanceAction::Restart
            };
            databases(env).instance_action(&id, action, cancelled)?;
            let mut payload = BTreeMap::new();
            payload.insert("id", id.clone());
            payload.insert("action", sub.to_string());
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Database instance {id}: {sub} completed.")
            })
        }
        "stop-all" => {
            let specs = FlagSpecs::new().bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if let Some(extra) = parsed.positionals.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for db stop-all"
                )));
            }
            require_yes(
                parsed.flag("yes"),
                "stopping all managed database instances requires --yes",
            )?;
            let store = devcore::state::Store::new(env.root.clone());
            let db = databases(env);
            let runner = devcore::shutdown::Shutdown::new(&store, db.engine());
            let plan = runner.preview(cancelled)?;
            runner.execute(&plan, &devcore::shutdown::ShutdownOptions {}, cancelled)?;
            let instances = plan
                .instances
                .iter()
                .map(|instance| serde_json::json!({"ID": instance.id, "Name": instance.name}))
                .collect::<Vec<_>>();
            let payload = serde_json::json!({
                "stopped": instances.len(),
                "instances": instances,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "Stopped {} managed database instances.",
                    payload["stopped"]
                )
            })
        }
        "remove" => {
            let specs = FlagSpecs::new().bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if parsed.positionals.len() != 1 {
                return Err(CliError::usage("usage: devbase db remove <id> --yes"));
            }
            require_yes(
                parsed.flag("yes"),
                "removing database instance requires --yes",
            )?;
            let id = parsed.positionals[0].trim().to_string();
            databases(env).instance_action(&id, InstanceAction::Remove, cancelled)?;
            let mut payload = BTreeMap::new();
            payload.insert("removed", id.clone());
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Removed database instance {id} (data files retained).")
            })
        }
        "delete-data" => run_delete_data(env, out, rest),
        "port" => run_port(env, out, rest),
        "upgrade" => run_upgrade(env, out, rest),
        "inspect" => {
            if rest.len() != 1 {
                return Err(CliError::usage("usage: devbase db inspect <id>"));
            }
            let id = rest[0].trim().to_string();
            let catalog = databases(env).inspect_instance_catalog(&id, cancelled)?;
            let payload = serde_json::json!({
                "id": id,
                "databases": catalog.databases,
                "users": catalog.users,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "Databases ({}): {}",
                    catalog.databases.len(),
                    catalog.databases.join(", ")
                )?;
                writeln!(
                    w,
                    "Users ({}): {}",
                    catalog.users.len(),
                    catalog.users.join(", ")
                )
            })
        }
        "create-db" => {
            if rest.len() != 2 {
                return Err(CliError::usage(
                    "usage: devbase db create-db <instance-id> <database-name>",
                ));
            }
            let id = rest[0].clone();
            let name = rest[1].clone();
            databases(env).create_database(&id, &name, cancelled)?;
            let payload = serde_json::json!({
                "instanceId": id,
                "database": name,
                "created": true,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Created database {name} on {id}.")
            })
        }
        "drop-db" => {
            let specs = FlagSpecs::new().bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if parsed.positionals.len() != 2 {
                return Err(CliError::usage(
                    "usage: devbase db drop-db <instance-id> <database-name> --yes",
                ));
            }
            require_yes(parsed.flag("yes"), "dropping a database requires --yes")?;
            let id = parsed.positionals[0].clone();
            let name = parsed.positionals[1].clone();
            databases(env).drop_database(&id, &name, cancelled)?;
            let payload = serde_json::json!({
                "instanceId": id,
                "database": name,
                "dropped": true,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Dropped database {name} on {id}.")
            })
        }
        "create-user" => {
            let specs = FlagSpecs::new().value("database").bool("password-stdin");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if parsed.positionals.len() != 2 {
                return Err(CliError::usage(
                    "usage: devbase db create-user <instance-id> <username> [--database <db>] --password-stdin",
                ));
            }
            if !parsed.flag("password-stdin") {
                return Err(CliError::usage(
                    "create-user requires --password-stdin (never pass secrets in argv)",
                ));
            }
            let secret = read_secret_stdin(&mut *env.input, "database user password")?;
            let id = parsed.positionals[0].clone();
            let username = parsed.positionals[1].clone();
            let database = parsed.string("database");
            databases(env).create_user(&id, &username, &secret, &database, cancelled)?;
            let payload = serde_json::json!({
                "instanceId": id,
                "username": username,
                "database": database,
                "created": true,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Created user {username} on {id}.")
            })
        }
        "drop-user" => {
            let specs = FlagSpecs::new().value("database").bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if parsed.positionals.len() != 2 {
                return Err(CliError::usage(
                    "usage: devbase db drop-user <instance-id> <username> [--database <db>] --yes",
                ));
            }
            require_yes(
                parsed.flag("yes"),
                "dropping a database user requires --yes",
            )?;
            let id = parsed.positionals[0].clone();
            let username = parsed.positionals[1].clone();
            let database = parsed.string("database");
            databases(env).drop_user(&id, &username, &database, cancelled)?;
            let payload = serde_json::json!({
                "instanceId": id,
                "username": username,
                "database": database,
                "dropped": true,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Dropped user {username} on {id}.")
            })
        }
        "connection" => run_connection(env, out, rest),
        "password" => {
            if rest.len() != 1 {
                return Err(CliError::usage("usage: devbase db password <id>"));
            }
            let id = rest[0].trim().to_string();
            let password = databases(env).database_password(&id)?;
            let mut payload = BTreeMap::new();
            payload.insert("instanceId", id.clone());
            payload.insert("password", password.clone());
            crate::commands::write_result(env, out, &payload, |w| writeln!(w, "{password}"))
        }
        "open-cli" => run_open_cli(env, out, rest),
        "profile" => run_profile(env, out, rest),
        "backup" => run_backup(env, out, rest),
        other => Err(CliError::usage(format!("unknown db subcommand {other:?}"))),
    }
}

fn run_create(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .value("kind")
        .value("package")
        .value("version")
        .value("port")
        .bool("persistent")
        .value("data-dir");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    let kind = parsed.string("kind").trim().to_lowercase();
    if parsed.positionals.len() != 1 || kind.is_empty() {
        return Err(CliError::usage(
            "usage: devbase db create <id> --kind <mysql|postgresql|redis|mongodb> [--package <pkg-id> | --version <ver>] [--port <port>] [--persistent] [--data-dir <path>]",
        ));
    }
    let id = parsed.positionals[0].trim().to_string();
    if !matches!(kind.as_str(), "mysql" | "postgresql" | "redis" | "mongodb") {
        return Err(CliError::usage(format!(
            "unsupported database kind {kind:?}"
        )));
    }
    let port = parsed.integer("port")?.unwrap_or(0);
    if !(0..=65535).contains(&port) {
        return Err(CliError::usage("--port must be 0 or 1–65535"));
    }
    let state = read_state(env)?;
    let mut package_id = parsed.string("package");
    if package_id.is_empty() {
        let version = parsed.string("version");
        if !version.is_empty() {
            let wanted = version.trim_start_matches('v');
            package_id = state
                .packages
                .iter()
                .find(|(_, package)| package.name == kind && package.version == wanted)
                .map(|(id, _)| id.clone())
                .unwrap_or_default();
            if package_id.is_empty() {
                let package = software(env).resolve_installed_or_catalog(&kind, &version)?;
                package_id = package.id();
            }
        } else {
            let matches: Vec<String> = state
                .packages
                .iter()
                .filter(|(_, package)| package.name == kind)
                .map(|(id, _)| id.clone())
                .collect();
            match matches.len() {
                0 => {
                    return Err(CliError::usage(format!(
                        "no installed {kind} package found; specify --package or --version after installing it"
                    )));
                }
                1 => package_id = matches[0].clone(),
                _ => {
                    return Err(CliError::usage(format!(
                        "multiple {kind} packages installed ({}); pass --package or --version",
                        matches.join(", ")
                    )));
                }
            }
        }
    }
    let db = databases(env);
    let port = if port == 0 {
        db.available_local_port(&|| false)?
    } else {
        port
    };
    let instance = Instance {
        id: id.clone(),
        kind,
        package: package_id,
        port,
        persistent: parsed.flag("persistent"),
        data_directory: parsed.string("data-dir").trim().to_string(),
        ..Instance::default()
    };
    db.create_instance(instance.clone(), &|| false)?;
    let saved = read_state(env)?
        .instances
        .get(&id)
        .cloned()
        .unwrap_or(instance);
    let data_directory = database_data_directory(db.store().root(), &saved, None);
    let payload = serde_json::json!({
        "instance": saved,
        "dataDirectory": data_directory.to_string_lossy(),
    });
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "Created {} instance {} on port {} ({}).",
            saved.kind, saved.id, saved.port, saved.package
        )
    })
}

fn run_delete_data(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .bool("preview")
        .value("confirm")
        .value("fingerprint");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase db delete-data <id> (--preview | --confirm <id> --fingerprint <fp>)",
        ));
    }
    let id = parsed.positionals[0].trim().to_string();
    let db = databases(env);
    if parsed.flag("preview") {
        let preview = db.preview_instance_deletion(&id, &|| false)?;
        return crate::commands::write_result(env, out, &preview, |w| {
            writeln!(w, "Instance: {} ({})", preview.instance_id, preview.kind)?;
            writeln!(w, "Can delete permanently: {}", preview.can_delete)?;
            writeln!(w, "Path: {}", preview.path.display())?;
            writeln!(w, "Entries: {} | Bytes: {}", preview.entries, preview.bytes)?;
            writeln!(w, "Fingerprint: {}", preview.fingerprint)?;
            writeln!(w, "Detail: {}", preview.detail)
        });
    }
    let confirm = parsed.string("confirm");
    let fingerprint = parsed.string("fingerprint");
    if confirm.trim().is_empty() || fingerprint.trim().is_empty() {
        return Err(CliError::usage(
            "permanent data deletion requires --confirm <id> and --fingerprint <fp> (run with --preview first)",
        ));
    }
    db.delete_instance_data(&id, &fingerprint, &confirm, &|| false)?;
    let mut payload = BTreeMap::new();
    payload.insert("deleted", id.clone());
    payload.insert("fingerprint", fingerprint.clone());
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(w, "Permanently deleted app-managed data for instance {id}.")
    })
}

fn run_port(env: &mut CliEnv, _out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new().bool("repair");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    let state = read_state(env)?;
    if parsed.flag("repair") {
        if parsed.positionals.len() != 1 {
            return Err(CliError::usage("usage: devbase db port <id> --repair"));
        }
        let id = parsed.positionals[0].trim();
        if !state.instances.contains_key(id) {
            return Err(CliError::operation(format!(
                "database instance {id:?} not found"
            )));
        }
        return Err(CliError::operation(
            "repairing an instance port is unavailable in this build (devcore has no port repair API)",
        ));
    }
    if parsed.positionals.len() != 2 {
        return Err(CliError::usage(
            "usage: devbase db port <id> <new-port> | devbase db port <id> --repair",
        ));
    }
    let id = parsed.positionals[0].trim();
    let instance = state
        .instances
        .get(id)
        .ok_or_else(|| CliError::operation(format!("database instance {id:?} not found")))?;
    let _port: i64 = parsed.positionals[1]
        .parse()
        .map_err(|_| CliError::usage("port must be 1–65535"))?;
    let _ = instance;
    Err(CliError::operation(
        "changing an instance port is unavailable in this build (devcore has no port change API)",
    ))
}

fn run_upgrade(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new().bool("targets");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.is_empty() || parsed.positionals.len() > 2 {
        return Err(CliError::usage(
            "usage: devbase db upgrade <id> [<target-package-or-version>] [--targets]",
        ));
    }
    let id = parsed.positionals[0].trim().to_string();
    let state = read_state(env)?;
    let instance = state
        .instances
        .get(&id)
        .cloned()
        .ok_or_else(|| CliError::operation(format!("database instance {id:?} not found")))?;
    let targets = upgrade_targets(&state, &instance);
    if parsed.flag("targets") || parsed.positionals.len() == 1 {
        let payload = serde_json::json!({
            "id": id,
            "currentPackage": instance.package,
            "targets": targets,
        });
        return crate::commands::write_result(env, out, &payload, |w| {
            if targets.is_empty() {
                writeln!(
                    w,
                    "No compatible upgrade targets installed for {id} ({}).",
                    instance.package
                )?;
                return Ok(());
            }
            for target in &targets {
                writeln!(w, "{target}")?;
            }
            Ok(())
        });
    }
    let want = parsed.positionals[1].trim();
    let target = targets
        .iter()
        .find(|target| {
            target.as_str() == want
                || state
                    .packages
                    .get(target.as_str())
                    .is_some_and(|package| package.version == want)
        })
        .cloned()
        .ok_or_else(|| {
            CliError::operation(format!(
                "target {want:?} is not an installed compatible upgrade for {id} (available: {targets:?})"
            ))
        })?;
    databases(env).upgrade_instance_package(&instance, &target, &|| false)?;
    let mut payload = BTreeMap::new();
    payload.insert("id", id.clone());
    payload.insert("fromPackage", instance.package.clone());
    payload.insert("toPackage", target.clone());
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(w, "Upgraded {id} from {} to {target}.", instance.package)
    })
}

fn run_connection(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .value("profile")
        .value("database")
        .value("format");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase db connection <id> [--profile <profile-id>] [--database <db>] [--format uri|env|jdbc|cli]",
        ));
    }
    let id = parsed.positionals[0].trim().to_string();
    let state = read_state(env)?;
    let instance = state
        .instances
        .get(&id)
        .ok_or_else(|| CliError::operation(format!("database instance {id:?} not found")))?;
    let db = databases(env);
    let profile_id = parsed.string("profile");
    let database = parsed.string("database");
    let mut info = if profile_id.is_empty() {
        db.connection_info(&id)?
    } else {
        db.profile_connection_info(&id, &profile_id)?
    };
    info = info.with_database(&instance.kind, &database);
    let env_assign = info.env_assignment_for_kind(&instance.kind);
    let jdbc = info.jdbc_url(&instance.kind).ok();
    let cli = db
        .database_cli_command(&id, &profile_id, Some(&database))
        .ok();
    let format = parsed.string("format").trim().to_lowercase();
    if !matches!(format.as_str(), "" | "uri" | "env" | "jdbc" | "cli") {
        return Err(CliError::usage(
            "--format must be one of uri, env, jdbc, cli",
        ));
    }
    if format == "jdbc" && jdbc.is_none() {
        return Err(CliError::operation(
            "JDBC URL is unavailable for this engine",
        ));
    }
    if format == "cli" && cli.is_none() {
        return Err(CliError::operation(
            "CLI command is unavailable for this engine",
        ));
    }
    let mut payload = BTreeMap::new();
    payload.insert("instanceId", id.clone());
    payload.insert("kind", instance.kind.clone());
    payload.insert("profileId", profile_id.clone());
    payload.insert("uri", info.uri.clone());
    payload.insert("username", info.username.clone());
    payload.insert("credentialFile", info.credential_file.clone());
    payload.insert("env", env_assign.clone());
    if let Some(url) = &jdbc {
        payload.insert("jdbc", url.clone());
    }
    if let Some(command) = &cli {
        payload.insert("cli", command.clone());
    }
    crate::commands::write_result(env, out, &payload, |w| match format.as_str() {
        "uri" => writeln!(w, "{}", info.uri),
        "env" => writeln!(w, "{env_assign}"),
        "jdbc" => writeln!(w, "{}", jdbc.clone().unwrap_or_default()),
        "cli" => writeln!(w, "{}", cli.clone().unwrap_or_default()),
        _ => {
            writeln!(w, "URI: {}", info.uri)?;
            writeln!(w, "Env: {env_assign}")?;
            if let Some(url) = &jdbc {
                writeln!(w, "JDBC: {url}")?;
            }
            if !info.username.is_empty() {
                writeln!(w, "Username: {}", info.username)?;
            }
            if !info.credential_file.is_empty() {
                writeln!(w, "Credential file: {}", info.credential_file)?;
            }
            if let Some(command) = &cli {
                writeln!(w, "CLI: {command}")?;
            }
            Ok(())
        }
    })
}

fn run_open_cli(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .value("profile")
        .value("database")
        .bool("print");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase db open-cli <id> [--profile <profile-id>] [--database <db>] [--print]",
        ));
    }
    let id = parsed.positionals[0].trim().to_string();
    let profile_id = parsed.string("profile");
    let database = parsed.string("database");
    let print_only = parsed.flag("print");
    let db = databases(env);
    let command = db.database_cli_command(&id, &profile_id, Some(&database))?;
    if !print_only {
        db.open_database_cli(&id, &profile_id, Some(&database), &|| false)?;
    }
    let payload = serde_json::json!({
        "instanceId": id,
        "profileId": profile_id,
        "database": database,
        "command": command,
        "opened": !print_only,
    });
    crate::commands::write_result(env, out, &payload, |w| writeln!(w, "{command}"))
}

fn run_profile(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let mut rest = args;
    let mut sub = "list";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    match sub {
        "list" => {
            if rest.len() > 1 {
                return Err(CliError::usage(
                    "usage: devbase db profile list [<instance-id>]",
                ));
            }
            let filter = rest
                .first()
                .map(|value| value.trim().to_string())
                .unwrap_or_default();
            let state = read_state(env)?;
            let profiles: Vec<ConnectionProfile> = state
                .connection_profiles
                .values()
                .filter(|profile| filter.is_empty() || profile.instance_id == filter)
                .cloned()
                .collect();
            #[derive(Serialize)]
            struct Payload<'a> {
                profiles: &'a [ConnectionProfile],
            }
            crate::commands::write_result(
                env,
                out,
                &Payload {
                    profiles: &profiles,
                },
                |w| {
                    if profiles.is_empty() {
                        writeln!(w, "No connection profiles saved.")?;
                        return Ok(());
                    }
                    for profile in &profiles {
                        writeln!(
                            w,
                            "{}\tinstance={}\tname=\"{}\"\tuser={}\tdb={}\tredisDb={}",
                            profile.id,
                            profile.instance_id,
                            profile.name,
                            profile.username,
                            profile.database,
                            profile.redis_db
                        )?;
                    }
                    Ok(())
                },
            )
        }
        "save" => {
            let specs = FlagSpecs::new()
                .value("id")
                .value("instance")
                .value("name")
                .value("database")
                .value("username")
                .value("auth-source")
                .value("redis-db");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if let Some(extra) = parsed.positionals.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for db profile save"
                )));
            }
            let instance_id = parsed.string("instance");
            let name = parsed.string("name");
            if instance_id.trim().is_empty() || name.trim().is_empty() {
                return Err(CliError::usage(
                    "usage: devbase db profile save --instance <id> --name <label> [--id <profile-id>] [--database <db>] [--username <user>] [--auth-source <db>] [--redis-db <0-15>]",
                ));
            }
            let profile = ConnectionProfile {
                id: parsed.string("id"),
                instance_id,
                name,
                database: parsed.string("database"),
                username: parsed.string("username"),
                mongo_auth_source: parsed.string("auth-source"),
                redis_db: parsed.integer("redis-db")?.unwrap_or(0),
            };
            let saved = databases(env).save_connection_profile(profile)?;
            #[derive(Serialize)]
            struct Payload {
                profile: ConnectionProfile,
            }
            crate::commands::write_result(
                env,
                out,
                &Payload {
                    profile: saved.clone(),
                },
                |w| {
                    writeln!(
                        w,
                        "Saved connection profile {} ({}) on {}.",
                        saved.id, saved.name, saved.instance_id
                    )
                },
            )
        }
        "delete" => {
            let specs = FlagSpecs::new().bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if parsed.positionals.len() != 2 {
                return Err(CliError::usage(
                    "usage: devbase db profile delete <instance-id> <profile-id> --yes",
                ));
            }
            require_yes(
                parsed.flag("yes"),
                "deleting a connection profile requires --yes",
            )?;
            let instance_id = parsed.positionals[0].clone();
            let profile_id = parsed.positionals[1].clone();
            databases(env).delete_connection_profile(&instance_id, &profile_id)?;
            let payload = serde_json::json!({
                "instanceId": instance_id,
                "profileId": profile_id,
                "deleted": true,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "Deleted connection profile {profile_id} from {instance_id}."
                )
            })
        }
        other => Err(CliError::usage(format!(
            "unknown db profile subcommand {other:?} (expected list, save, or delete)"
        ))),
    }
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct BackupView {
    Name: String,
    Path: String,
    Database: String,
    Size: i64,
    Modified: String,
}

fn run_backup(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let mut rest = args;
    let mut sub = "list";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    match sub {
        "list" => {
            if rest.len() != 1 {
                return Err(CliError::usage(
                    "usage: devbase db backup list <instance-id>",
                ));
            }
            let id = rest[0].clone();
            let backups = databases(env).backups(&id, &|| false)?;
            let views: Vec<BackupView> = backups
                .into_iter()
                .map(|backup| BackupView {
                    Name: backup.name,
                    Path: backup.path.to_string_lossy().into_owned(),
                    Database: backup.database,
                    Size: backup.size,
                    Modified: system_time_rfc3339(backup.modified),
                })
                .collect();
            let payload = serde_json::json!({
                "instanceId": id,
                "backups": views,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                if views.is_empty() {
                    writeln!(w, "No backups for {id}.")?;
                    return Ok(());
                }
                for view in &views {
                    writeln!(
                        w,
                        "{}\tdb={}\tsize={}\tmodified={}",
                        view.Name, view.Database, view.Size, view.Modified
                    )?;
                }
                Ok(())
            })
        }
        "create" => {
            let specs = FlagSpecs::new().value("database");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if parsed.positionals.len() != 1 {
                return Err(CliError::usage(
                    "usage: devbase db backup create <instance-id> [--database <db>]",
                ));
            }
            let id = parsed.positionals[0].clone();
            let database = parsed.string("database");
            let path = databases(env).backup(&id, &database, &|| false)?;
            let mut payload = BTreeMap::new();
            payload.insert("instanceId", id.clone());
            payload.insert("database", database.clone());
            payload.insert("path", path.clone());
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Created backup: {path}")
            })
        }
        "restore" => {
            let specs = FlagSpecs::new().value("database").bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if parsed.positionals.len() != 2 {
                return Err(CliError::usage(
                    "usage: devbase db backup restore <instance-id> <backup-name-or-path> [--database <db>] --yes",
                ));
            }
            require_yes(parsed.flag("yes"), "restoring a backup requires --yes")?;
            let id = parsed.positionals[0].trim().to_string();
            let target = parsed.positionals[1].trim().to_string();
            let database = parsed.string("database");
            let restore_path = if Path::new(&target).exists() {
                target.clone()
            } else {
                let backups = databases(env).backups(&id, &|| false)?;
                backups
                    .into_iter()
                    .find(|backup| backup.name == target || backup.path.to_string_lossy() == target)
                    .map(|backup| backup.path.to_string_lossy().into_owned())
                    .ok_or_else(|| {
                        CliError::operation(format!(
                            "backup {target:?} not found for instance {id}"
                        ))
                    })?
            };
            databases(env).restore(&id, &database, &restore_path, &|| false)?;
            let payload = serde_json::json!({
                "instanceId": id,
                "database": database,
                "path": restore_path,
                "restored": true,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Restored {id} from {restore_path}.")
            })
        }
        "delete" => {
            let specs = FlagSpecs::new().bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if parsed.positionals.len() != 2 {
                return Err(CliError::usage(
                    "usage: devbase db backup delete <instance-id> <backup-name> --yes",
                ));
            }
            require_yes(parsed.flag("yes"), "deleting a backup requires --yes")?;
            let id = parsed.positionals[0].clone();
            let name = parsed.positionals[1].clone();
            databases(env).delete_backup(&id, &name, &|| false)?;
            let payload = serde_json::json!({
                "instanceId": id,
                "name": name,
                "deleted": true,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Deleted backup {name} from {id}.")
            })
        }
        other => Err(CliError::usage(format!(
            "unknown db backup subcommand {other:?} (expected list, create, restore, or delete)"
        ))),
    }
}

fn system_time_rfc3339(time: std::time::SystemTime) -> String {
    let local: chrono::DateTime<chrono::Local> = time.into();
    go_rfc3339_nano_local(&local)
}
