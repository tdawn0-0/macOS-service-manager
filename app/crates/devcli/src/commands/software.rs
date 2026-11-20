//! `software` (ported from `cmd_software.go`) over `devcore::software`.
//!
//! JDK discovery, uv/Python management, and the node/java shell integration
//! belong to the shell/diagnostics slice. Those subcommands report an explicit
//! `unavailable` operation error rather than fabricating an empty success.

use std::collections::BTreeMap;
use std::io::Write;

use serde::Serialize;

use crate::args::{FlagSpecs, ParseOutcome, has_help_flag, parse_flags};
use crate::commands::require_yes;
use crate::env::CliEnv;
use crate::error::{CliError, CliResult};

use devcore::model::{Package, State};
use devcore::shell::{Shell, UVPython};
use devcore::software::Software;
use devcore::software::catalog;
use devcore::software::details::PackageDetails;
use devcore::software::integrity::PackageIntegrityResult;

const CATALOG_ORDER: [&str; 7] = [
    "node",
    "java",
    "python",
    "mysql",
    "postgresql",
    "redis",
    "mongodb",
];

fn software(env: &CliEnv) -> Software {
    Software::new(devcore::state::Store::new(env.root.clone()))
}

fn shell(env: &CliEnv) -> Shell {
    let store = devcore::state::Store::new(env.root.clone());
    Shell::new(store.clone(), Software::new(store))
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct UVPythonView {
    Key: String,
    Version: String,
    Path: String,
    Implementation: String,
    Arch: String,
    OS: String,
}

impl From<&UVPython> for UVPythonView {
    fn from(python: &UVPython) -> Self {
        UVPythonView {
            Key: python.key.clone(),
            Version: python.version.clone(),
            Path: python.path.clone(),
            Implementation: python.implementation.clone(),
            Arch: python.arch.clone(),
            OS: python.os_name.clone(),
        }
    }
}

fn state(env: &CliEnv) -> CliResult<State> {
    Ok(devcore::state::Store::new(env.root.clone()).read()?)
}

fn unavailable(feature: &str) -> CliError {
    CliError::operation(format!(
        "{feature} is unavailable in this build (the shell/diagnostics slice has not landed)"
    ))
}

fn known_software(name: &str) -> bool {
    CATALOG_ORDER.contains(&name)
}

#[derive(Serialize)]
struct PackageView {
    id: String,
    package: Package,
    #[serde(skip_serializing_if = "is_false")]
    default: bool,
    #[serde(skip_serializing_if = "is_false")]
    unused: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn package_default(state: &State, package: &Package) -> bool {
    match package.name.as_str() {
        "node" => state.node_default == package.id(),
        other => state.runtime_defaults.get(other) == Some(&package.id()),
    }
}

fn package_unused(state: &State, package: &Package) -> bool {
    !package_default(state, package)
        && !state
            .instances
            .values()
            .any(|instance| instance.package == package.id())
}

pub fn run_software_command(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
) -> CliResult<()> {
    if has_help_flag(args) && args.len() == 1 {
        crate::schema::print_topic_help(out, "software")?;
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "list";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    match sub {
        "list" | "status" => run_list(env, out, rest),
        "versions" => run_versions(env, out, rest),
        "review" => run_review(env, out, rest),
        "install" => install(env, out, rest),
        "uninstall" => run_uninstall(env, out, rest),
        "verify" => run_verify(env, out, rest),
        "repair" => run_repair(env, out, rest),
        "default" => run_default(env, out, rest),
        "shell" => run_shell(env, out, rest),
        "which" => run_which(env, out, rest),
        "uv-path" => run_uv_path(env, out, rest),
        other => Err(CliError::usage(format!(
            "unknown software subcommand {other:?}"
        ))),
    }
}

/// The `install` top-level shortcut.
pub fn run_software_install_command(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
) -> CliResult<()> {
    install(env, out, args)
}

fn run_list(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if let Some(extra) = args.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for software list"
        )));
    }
    let state = state(env)?;
    let packages: Vec<PackageView> = state
        .packages
        .iter()
        .map(|(id, package)| PackageView {
            id: id.clone(),
            package: package.clone(),
            default: package_default(&state, package),
            unused: package_unused(&state, package),
        })
        .collect();
    let shell = shell(env);
    let uv_path = shell.uv_path();
    let (uv_path_text, uv_description) = match &uv_path {
        Ok(path) => (
            path.to_string_lossy().into_owned(),
            shell.uv_description().unwrap_or_default(),
        ),
        Err(_) => (String::new(), String::new()),
    };
    let uv_installed: Vec<UVPythonView> = if uv_path.is_ok() {
        shell
            .uv_pythons(true)
            .unwrap_or_default()
            .iter()
            .map(UVPythonView::from)
            .collect()
    } else {
        Vec::new()
    };
    let uv_pin = if uv_path.is_ok() {
        shell.uv_global_pin().unwrap_or_default()
    } else {
        String::new()
    };
    let mut uv = BTreeMap::new();
    uv.insert("path", serde_json::Value::String(uv_path_text));
    uv.insert("description", serde_json::Value::String(uv_description));
    uv.insert("globalPin", serde_json::Value::String(uv_pin));
    uv.insert("installed", serde_json::to_value(&uv_installed).unwrap());
    let mut payload = BTreeMap::new();
    payload.insert("catalog", serde_json::to_value(CATALOG_ORDER).unwrap());
    payload.insert("packages", serde_json::to_value(&packages).unwrap());
    payload.insert(
        "nodeDefault",
        serde_json::Value::String(state.node_default.clone()),
    );
    payload.insert(
        "runtimeDefaults",
        serde_json::to_value(&state.runtime_defaults).unwrap(),
    );
    payload.insert("nodeShellEnabled", serde_json::Value::Bool(false));
    payload.insert("javaShellEnabled", serde_json::Value::Bool(false));
    payload.insert("javaInstallDir", serde_json::Value::String(String::new()));
    payload.insert("javaInstallations", serde_json::Value::Array(Vec::new()));
    payload.insert("uv", serde_json::to_value(&uv).unwrap());
    payload.insert(
        "javaError",
        serde_json::Value::String(unavailable("JDK discovery").to_string()),
    );
    if let Err(error) = &uv_path {
        payload.insert("uvError", serde_json::Value::String(error.to_string()));
    }
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(w, "Catalog: {}", CATALOG_ORDER.join(", "))?;
        if packages.is_empty() {
            writeln!(w, "No managed packages installed.")?;
        }
        for view in &packages {
            let flags = [
                view.default.then_some("default"),
                view.unused.then_some("unused"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            let suffix = if flags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", flags.join(","))
            };
            writeln!(w, "{}{suffix}", view.id)?;
        }
        writeln!(w, "Node default: {}", state.node_default)?;
        Ok(())
    })
}

fn run_versions(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new().bool("refresh");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    let _ = parsed.flag("refresh");
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase software versions <node|java|python|mysql|postgresql|redis|mongodb> [--refresh]",
        ));
    }
    let name = parsed.positionals[0].trim().to_lowercase();
    if !known_software(&name) {
        return Err(CliError::usage(format!(
            "unknown software {name:?} (node, java, python, mysql, postgresql, redis, mongodb)"
        )));
    }
    match name.as_str() {
        "node" => {
            let shell = shell(env);
            match shell.node_releases(&|| false, parsed.flag("refresh")) {
                Ok((releases, stale)) => {
                    let payload = serde_json::json!({
                        "software": "node",
                        "stale": stale,
                        "releases": releases,
                    });
                    crate::commands::write_result(env, out, &payload, |w| {
                        for release in &releases {
                            if release.lts.is_empty() {
                                writeln!(w, "{}", release.version)?;
                            } else {
                                writeln!(w, "{}\tLTS {}", release.version, release.lts)?;
                            }
                        }
                        Ok(())
                    })
                }
                Err(error) => {
                    let fallback: Vec<String> = catalog::available_versions("node");
                    let payload = serde_json::json!({
                        "software": "node",
                        "versions": fallback,
                        "stale": true,
                        "error": error.to_string(),
                    });
                    crate::commands::write_result(env, out, &payload, |w| {
                        writeln!(w, "Warning: {error}")?;
                        writeln!(w, "Fallback Node.js versions: {}", fallback.join(", "))
                    })
                }
            }
        }
        "java" => Err(unavailable("JDK discovery")),
        "python" => {
            let shell = shell(env);
            let rows: Vec<UVPythonView> = shell
                .uv_pythons(false)
                .map_err(|error| CliError::operation(error.to_string()))?
                .iter()
                .filter(|python| python.os_name == "macos" && python.arch == "aarch64")
                .map(UVPythonView::from)
                .collect();
            let payload = serde_json::json!({
                "software": "python",
                "versions": rows,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                for row in &rows {
                    writeln!(w, "{}\t{}", row.Key, row.Version)?;
                }
                Ok(())
            })
        }
        _ => {
            let versions = catalog::available_versions(&name);
            #[derive(Serialize)]
            struct Payload {
                software: String,
                versions: Vec<String>,
            }
            let payload = Payload {
                software: name.clone(),
                versions: versions.clone(),
            };
            crate::commands::write_result(env, out, &payload, |w| {
                for version in &versions {
                    writeln!(w, "{version}")?;
                }
                Ok(())
            })
        }
    }
}

/// The capitalized `PackageDetails` view.
#[derive(Serialize)]
#[allow(non_snake_case)]
struct DetailsView {
    Package: Package,
    ArchiveBytes: i64,
    Cached: bool,
    Installed: bool,
    Compatible: bool,
    Compatibility: String,
    Location: String,
    References: Vec<String>,
    Integrity: String,
    IntegrityNote: String,
}

impl From<&PackageDetails> for DetailsView {
    fn from(details: &PackageDetails) -> Self {
        DetailsView {
            Package: details.package.clone(),
            ArchiveBytes: details.archive_bytes,
            Cached: details.cached,
            Installed: details.installed,
            Compatible: details.compatible,
            Compatibility: details.compatibility.clone(),
            Location: details.location.to_string_lossy().into_owned(),
            References: details.references.clone(),
            Integrity: details.integrity.clone(),
            IntegrityNote: details.integrity_note.clone(),
        }
    }
}

fn run_review(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if args.len() != 2 {
        return Err(CliError::usage(
            "usage: devbase software review <software> <version>",
        ));
    }
    let name = args[0].trim().to_lowercase();
    let version = args[1].trim();
    if !known_software(&name) {
        return Err(CliError::usage(format!("unknown software {name:?}")));
    }
    let software = software(env);
    let package = software.resolve_installed_or_catalog(&name, version)?;
    let details = software.package_details(&|| false, &package)?;
    let view = DetailsView::from(&details);
    crate::commands::write_result(env, out, &view, |w| {
        writeln!(w, "Package: {}", view.Package.id())?;
        writeln!(w, "URL: {}", view.Package.url)?;
        writeln!(w, "SHA-256: {}", view.Package.sha256)?;
        writeln!(w, "License: {}", view.Package.license)?;
        writeln!(w, "Compatibility: {}", view.Compatibility)?;
        writeln!(w, "Location: {}", view.Location)?;
        writeln!(w, "Cached: {} | Installed: {}", view.Cached, view.Installed)?;
        Ok(())
    })
}

fn install(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if args.len() != 2 {
        return Err(CliError::usage(
            "usage: devbase software install <software> <version>",
        ));
    }
    let name = args[0].trim().to_lowercase();
    let version = args[1].trim();
    if !known_software(&name) {
        return Err(CliError::usage(format!("unknown software {name:?}")));
    }
    let software = software(env);
    let package = software.resolve_installed_or_catalog(&name, version)?;
    software.install(&package, &|| false)?;
    let state = state(env)?;
    let installed = state
        .packages
        .get(&package.id())
        .cloned()
        .unwrap_or(package.clone());
    let payload = serde_json::json!({
        "installed": installed.id(),
        "location": devcore::state::Store::new(env.root.clone())
            .package_directory(&installed)
            .to_string_lossy(),
        "package": installed,
    });
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "Installed {} at {}.",
            payload["installed"].as_str().unwrap_or_default(),
            payload["location"].as_str().unwrap_or_default()
        )
    })
}

fn resolve_package_args(
    env: &CliEnv,
    args: &[String],
    usage: &str,
) -> CliResult<(Software, State, Package)> {
    if args.is_empty() || args.len() > 2 {
        return Err(CliError::usage(usage));
    }
    let software = software(env);
    let state = state(env)?;
    let package = if args.len() == 1 {
        let id = args[0].trim();
        state
            .packages
            .get(id)
            .cloned()
            .ok_or_else(|| CliError::operation(format!("package {id:?} is not installed")))?
    } else {
        let name = args[0].trim().to_lowercase();
        let version = args[1].trim().trim_start_matches('v');
        state
            .packages
            .values()
            .find(|package| package.name == name && package.version == version)
            .cloned()
            .ok_or_else(|| {
                CliError::operation(format!("package {name}@{version} is not installed"))
            })?
    };
    Ok((software, state, package))
}

fn run_uninstall(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new().bool("yes");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    require_yes(parsed.flag("yes"), "uninstalling a package requires --yes")?;
    let (software, state, package) = resolve_package_args(
        env,
        &parsed.positionals,
        "usage: devbase software uninstall (<package-id> | <software> <version-or-id>) --yes",
    )?;
    if package.name == "java" {
        return Err(unavailable("Java install removal"));
    }
    if package.name == "python" {
        return Err(unavailable("uv Python removal"));
    }
    software.uninstall(&package.id(), &|| false)?;
    let payload = serde_json::json!({
        "uninstalled": package.id(),
        "formula": state
            .packages
            .get(&package.id())
            .map(|package| package.formula.clone())
            .unwrap_or_default(),
    });
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "Uninstalled {}.",
            payload["uninstalled"].as_str().unwrap_or_default()
        )
    })
}

fn run_verify(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let (software, _state, package) = resolve_package_args(
        env,
        args,
        "usage: devbase software verify (<package-id> | <software> <version>)",
    )?;
    let result = software.verify_package(&package.id(), &|| false)?;
    #[derive(Serialize)]
    struct Payload {
        #[serde(rename = "packageId")]
        package_id: String,
        result: PackageIntegrityResult,
    }
    let payload = Payload {
        package_id: package.id(),
        result,
    };
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "{}: {} · {} entries checked",
            payload.package_id, payload.result.status, payload.result.files
        )
    })
}

fn run_repair(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let (software, _state, package) = resolve_package_args(
        env,
        args,
        "usage: devbase software repair (<package-id> | <software> <version>)",
    )?;
    software.repair_package(&package.id(), &|| false)?;
    #[derive(Serialize)]
    struct Payload {
        repaired: String,
    }
    let payload = Payload {
        repaired: package.id(),
    };
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(w, "Repaired {}.", payload.repaired)
    })
}

fn run_shell(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if args.is_empty() || args.len() > 2 {
        return Err(CliError::usage(
            "usage: devbase software shell <node|java> [status|enable|disable|bash-command]",
        ));
    }
    let name = args[0].trim().to_lowercase();
    if name != "node" && name != "java" {
        return Err(CliError::usage(format!(
            "shell integration is supported for node and java (got {name:?})"
        )));
    }
    let action = if args.len() == 2 {
        args[1].trim().to_lowercase()
    } else {
        "status".to_string()
    };
    let shell = shell(env);
    let enabled = || {
        if name == "node" {
            shell.node_shell_enabled()
        } else {
            shell.runtime_shell_enabled("java")
        }
    };
    match action.as_str() {
        "status" => {
            let payload = serde_json::json!({
                "software": name,
                "enabled": enabled(),
                "bashCommand": shell.bash_command(),
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "{name} shell integration enabled: {}",
                    payload["enabled"].as_bool().unwrap_or(false)
                )?;
                writeln!(
                    w,
                    "Bash command: {}",
                    payload["bashCommand"].as_str().unwrap_or_default()
                )
            })
        }
        "enable" | "disable" => {
            let enable = action == "enable";
            let detail = if name == "node" {
                if enable {
                    shell.enable_node_shell()
                } else {
                    shell.disable_node_shell()
                }
                .map_err(|error| CliError::operation(error.to_string()))?
            } else {
                shell
                    .configure_runtime_shell("java", enable)
                    .map_err(|error| CliError::operation(error.to_string()))?;
                String::new()
            };
            let payload = serde_json::json!({
                "software": name,
                "enabled": enable,
                "files": detail,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "{} {name} shell integration.",
                    if enable { "Enabled" } else { "Disabled" }
                )
            })
        }
        "bash-command" => {
            let payload = serde_json::json!({
                "software": name,
                "bashCommand": shell.bash_command(),
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "{}", payload["bashCommand"].as_str().unwrap_or_default())
            })
        }
        other => Err(CliError::usage(format!(
            "unknown software shell action {other:?}"
        ))),
    }
}

fn run_default(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if args.len() != 2 {
        return Err(CliError::usage(
            "usage: devbase software default <node|java|python> <version-or-id>",
        ));
    }
    let name = args[0].trim().to_lowercase();
    let target = args[1].trim().to_string();
    match name.as_str() {
        "node" => {
            let current = state(env)?;
            let id = if current.packages.contains_key(&target) {
                target.clone()
            } else {
                current
                    .packages
                    .values()
                    .find(|package| package.name == "node" && package.version == target)
                    .map(Package::id)
                    .ok_or_else(|| {
                        CliError::operation(format!("package node {target:?} is not installed"))
                    })?
            };
            shell(env)
                .set_node_default(&id)
                .map_err(|error| CliError::operation(error.to_string()))?;
            let payload = serde_json::json!({"software": "node", "default": id});
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Default Node.js set to {id}.")
            })
        }
        "python" => {
            let shell = shell(env);
            let installed = shell
                .uv_pythons(true)
                .map_err(|error| CliError::operation(error.to_string()))?;
            let key = installed
                .iter()
                .find(|python| python.key == target || python.version == target)
                .map(|python| python.key.clone())
                .ok_or_else(|| {
                    CliError::operation(format!("Python {target:?} is not installed by uv"))
                })?;
            shell
                .uv_python_action(&key, "default")
                .map_err(|error| CliError::operation(error.to_string()))?;
            let payload = serde_json::json!({"software": "python", "default": key});
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Global uv Python pin set to {key}.")
            })
        }
        "java" => {
            let current = state(env)?;
            let id = if current.packages.contains_key(&target) {
                target.clone()
            } else {
                current
                    .packages
                    .values()
                    .find(|package| package.name == "java" && package.version == target)
                    .map(Package::id)
                    .ok_or_else(|| {
                        CliError::operation(format!("package java {target:?} is not installed"))
                    })?
            };
            shell(env)
                .set_runtime_default("java", &id)
                .map_err(|error| CliError::operation(error.to_string()))?;
            let payload = serde_json::json!({"software": "java", "default": id});
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Default Java set to {id}.")
            })
        }
        other => Err(CliError::usage(format!(
            "default version is supported for node, java, and python (got {other:?})"
        ))),
    }
}

fn run_which(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new().value("dir").value("package");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase software which <software> [--dir <path>] [--package <id>]",
        ));
    }
    let name = parsed.positionals[0].trim().to_lowercase();
    let state = state(env)?;
    let software = software(env);
    let package_id = parsed.string("package");
    if !package_id.is_empty() {
        let binary = software.binary(&state, &package_id, &name)?;
        let payload = serde_json::json!({
            "software": name,
            "packageId": package_id,
            "binary": binary.to_string_lossy(),
        });
        return crate::commands::write_result(env, out, &payload, |w| {
            writeln!(w, "{}", payload["binary"].as_str().unwrap_or_default())
        });
    }
    match name.as_str() {
        "node" => {
            let dir = if parsed.string("dir").is_empty() {
                std::env::current_dir().unwrap_or_default()
            } else {
                std::path::PathBuf::from(parsed.string("dir"))
            };
            let shell = shell(env);
            let selection = shell
                .select_node(&dir, None)
                .map_err(|error| CliError::operation(error.to_string()))?;
            let binary = if selection.system {
                String::new()
            } else {
                selection.bin().join("node").to_string_lossy().into_owned()
            };
            let payload = serde_json::json!({
                "software": "node",
                "id": selection.id,
                "version": selection.version,
                "dir": selection.dir.to_string_lossy(),
                "binDir": selection.bin().to_string_lossy(),
                "binary": binary,
                "prefix": selection.prefix.to_string_lossy(),
                "source": selection.source,
                "spec": selection.spec,
                "system": selection.system,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                if selection.system {
                    writeln!(w, "system")
                } else {
                    writeln!(w, "{}", payload["binary"].as_str().unwrap_or_default())
                }
            })
        }
        "java" => {
            let id = state
                .runtime_defaults
                .get("java")
                .cloned()
                .unwrap_or_default();
            if id.is_empty() {
                return Err(CliError::operation("no default Java installation selected"));
            }
            let binary = software.binary(&state, &id, "java")?;
            let java_home = binary
                .parent()
                .and_then(|path| path.parent())
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            let payload = serde_json::json!({
                "software": "java",
                "id": id,
                "javaHome": java_home,
                "binary": binary.to_string_lossy(),
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "{}", payload["binary"].as_str().unwrap_or_default())
            })
        }
        "python" => {
            let shell = shell(env);
            let pin = shell
                .uv_global_pin()
                .map_err(|error| CliError::operation(error.to_string()))?;
            let installed = shell
                .uv_pythons(true)
                .map_err(|error| CliError::operation(error.to_string()))?;
            if !pin.is_empty()
                && let Some(row) = installed
                    .iter()
                    .find(|python| python.key == pin || python.version == pin)
            {
                let payload = serde_json::json!({
                    "software": "python",
                    "key": row.key,
                    "version": row.version,
                    "binary": row.path,
                });
                return crate::commands::write_result(env, out, &payload, |w| {
                    writeln!(w, "{}", payload["binary"].as_str().unwrap_or_default())
                });
            }
            Err(CliError::operation(
                "no global uv Python pin resolves to an installed interpreter",
            ))
        }
        "mysql" | "postgresql" | "redis" | "mongodb" => {
            let package = state
                .packages
                .values()
                .find(|package| package.name == name)
                .cloned()
                .ok_or_else(|| CliError::operation(format!("no {name} package installed")))?;
            let binary = software.binary(&state, &package.id(), &name)?;
            let payload = serde_json::json!({
                "software": name,
                "packageId": package.id(),
                "binary": binary.to_string_lossy(),
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "{}", payload["binary"].as_str().unwrap_or_default())
            })
        }
        other => Err(CliError::usage(format!("unsupported software {other:?}"))),
    }
}

fn run_uv_path(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if args.len() > 1 {
        return Err(CliError::usage(
            "usage: devbase software uv-path [<absolute-path>]",
        ));
    }
    let shell = shell(env);
    if let Some(path) = args.first() {
        let path = path.trim();
        if !path.is_empty() && !std::path::Path::new(path).is_absolute() {
            return Err(CliError::usage("uv path must be absolute"));
        }
        shell
            .set_uv_path(std::path::Path::new(path))
            .map_err(|error| CliError::operation(error.to_string()))?;
    }
    let path = shell
        .uv_path()
        .map_err(|error| CliError::operation(error.to_string()))?;
    let description = shell.uv_description().unwrap_or_default();
    let payload = serde_json::json!({
        "path": path.to_string_lossy(),
        "description": description,
    });
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(w, "{}", payload["path"].as_str().unwrap_or_default())
    })
}
