//! `settings`, `log-rotation`, and `cli` (ported from `cmd_settings.go`).
//!
//! The helper install/uninstall path needs the privileged-helper client, which
//! is a later slice: those subcommands report an explicit operation error.
//! Cache, log rotation, login item, and the CLI wrapper are fully wired.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;

use crate::args::{FlagSpecs, ParseOutcome, has_help_flag, parse_flags};
use crate::commands::require_yes;
use crate::env::CliEnv;
use crate::error::{CliError, CliResult};

use devcore::launchd::{
    SystemHost, SystemRunner, disable_launch_at_login, enable_launch_at_login,
    launch_at_login_enabled, login_agent_plist_path, resolve_login_executable,
};
use devcore::software::cache::DownloadCacheInfo;
use devcore::state::Store;

fn store(env: &CliEnv) -> Store {
    Store::new(env.root.clone())
}

fn login_agent_dir(_env: &CliEnv) -> PathBuf {
    match std::env::var("MSM_DEV_HOME") {
        Ok(home) if !home.is_empty() => PathBuf::from(home).join("login-agents"),
        _ => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Library/LaunchAgents"),
    }
}

pub fn run_settings_command(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
) -> CliResult<()> {
    if has_help_flag(args) && args.len() == 1 {
        crate::schema::print_topic_help(out, "settings")?;
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "status";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    match sub {
        "status" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for settings status"
                )));
            }
            settings_status(env, out)
        }
        "helper" => run_helper(env, out, rest),
        "cache" => run_cache(env, out, rest),
        "log-rotation" => run_log_rotation(env, out, rest),
        "login-item" => run_login_item(env, out, rest),
        other => Err(CliError::usage(format!(
            "unknown settings subcommand {other:?}"
        ))),
    }
}

fn settings_status(env: &mut CliEnv, out: &mut dyn Write) -> CliResult<()> {
    let state = store(env).read()?;
    let software = devcore::software::Software::new(store(env));
    let cache = software.download_cache_info()?;
    let login_dir = login_agent_dir(env);
    let plist = login_agent_plist_path(&login_dir)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut payload = BTreeMap::new();
    payload.insert(
        "workspace",
        serde_json::Value::String(env.root.to_string_lossy().into_owned()),
    );
    payload.insert("helper", serde_json::to_value(&env.helper_status).unwrap());
    payload.insert("downloadCache", serde_json::to_value(&cache).unwrap());
    payload.insert(
        "logRotationEnabled",
        serde_json::Value::Bool(!state.log_rotation_disabled),
    );
    payload.insert(
        "startAtLoginEnabled",
        serde_json::Value::Bool(launch_at_login_enabled(&login_dir)),
    );
    payload.insert("loginAgentPlist", serde_json::Value::String(plist));
    payload.insert(
        "cli",
        serde_json::to_value(crate::commands::cli_status(env)).unwrap(),
    );
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(w, "Workspace: {}", env.root.display())?;
        writeln!(w, "Privileged Helper: {}", env.helper_status.State)?;
        writeln!(
            w,
            "Download Cache: {} files, {} bytes ({})",
            cache.files,
            cache.bytes,
            cache.directory.display()
        )?;
        writeln!(
            w,
            "Log Rotation: {}",
            if state.log_rotation_disabled {
                "off"
            } else {
                "on"
            }
        )?;
        writeln!(w, "Start at Login: {}", launch_at_login_enabled(&login_dir))?;
        Ok(())
    })
}

fn run_helper(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let mut rest = args;
    let mut sub = "status";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    match sub {
        "status" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for settings helper status"
                )));
            }
            let payload = env.helper_status.clone();
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "{}", payload.Message)
            })
        }
        "install" => Err(CliError::operation(
            "installing the privileged helper is unavailable in this build (the helper client has not landed)",
        )),
        "uninstall" => {
            let specs = FlagSpecs::new().bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            require_yes(
                parsed.flag("yes"),
                "removing the privileged helper requires --yes",
            )?;
            Err(CliError::operation(
                "removing the privileged helper is unavailable in this build (the helper client has not landed)",
            ))
        }
        other => Err(CliError::usage(format!(
            "unknown settings helper subcommand {other:?}"
        ))),
    }
}

fn run_cache(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let mut rest = args;
    let mut sub = "status";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    let software = devcore::software::Software::new(store(env));
    match sub {
        "status" => {
            let info = software.download_cache_info()?;
            crate::commands::write_result(env, out, &info, |w| {
                writeln!(
                    w,
                    "{} files, {} bytes in {}",
                    info.files,
                    info.bytes,
                    info.directory.display()
                )
            })
        }
        "clear" => {
            let specs = FlagSpecs::new().bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            require_yes(
                parsed.flag("yes"),
                "clearing the download cache requires --yes",
            )?;
            let before = software.download_cache_info()?;
            let remaining = software.clear_download_cache(&|| false)?;
            let removed = DownloadCacheInfo {
                directory: before.directory.clone(),
                files: before.files.max(remaining.files) - remaining.files,
                bytes: before.bytes.max(remaining.bytes) - remaining.bytes,
            };
            let payload = serde_json::json!({"removed": removed, "remaining": remaining});
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "Removed {} cached archives ({} bytes).",
                    payload["removed"]["files"], payload["removed"]["bytes"]
                )
            })
        }
        other => Err(CliError::usage(format!(
            "unknown settings cache subcommand {other:?} (expected status or clear)"
        ))),
    }
}

pub fn run_settings_log_rotation(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
) -> CliResult<()> {
    run_log_rotation(env, out, args)
}

fn run_log_rotation(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let mut sub = "status";
    if let Some(first) = args.first() {
        sub = first;
    }
    match sub {
        "status" => {
            let state = store(env).read()?;
            #[derive(Serialize)]
            struct Payload {
                enabled: bool,
            }
            let payload = Payload {
                enabled: !state.log_rotation_disabled,
            };
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "Log rotation: {}",
                    if payload.enabled { "on" } else { "off" }
                )
            })
        }
        "on" | "off" => {
            let enabled = sub == "on";
            store(env).with_state(|state| {
                state.log_rotation_disabled = !enabled;
                Ok(())
            })?;
            #[derive(Serialize)]
            struct Payload {
                enabled: bool,
            }
            crate::commands::write_result(env, out, &Payload { enabled }, |w| {
                writeln!(w, "Log rotation: {}", if enabled { "on" } else { "off" })
            })
        }
        other => Err(CliError::usage(format!(
            "unknown log rotation subcommand {other:?} (expected status, on, or off)"
        ))),
    }
}

fn run_login_item(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let mut sub = "status";
    if let Some(first) = args.first() {
        sub = first;
    }
    let dir = login_agent_dir(env);
    let plist = login_agent_plist_path(&dir)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    match sub {
        "status" => {
            #[derive(Serialize)]
            struct Payload {
                enabled: bool,
                plist: String,
            }
            let payload = Payload {
                enabled: launch_at_login_enabled(&dir),
                plist,
            };
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Start at Login: {}", payload.enabled)
            })
        }
        "on" => {
            let host = SystemHost::new();
            let app = if env.app_path.is_empty() {
                None
            } else {
                Some(std::path::Path::new(&env.app_path))
            };
            let executable = resolve_login_executable(&host, app, None)?;
            enable_launch_at_login(&dir, &executable, &SystemRunner::new())?;
            #[derive(Serialize)]
            struct Payload {
                enabled: bool,
                executable: String,
                plist: String,
            }
            crate::commands::write_result(
                env,
                out,
                &Payload {
                    enabled: true,
                    executable: executable.to_string_lossy().into_owned(),
                    plist,
                },
                |w| writeln!(w, "Start at Login enabled."),
            )
        }
        "off" => {
            disable_launch_at_login(&dir, &SystemRunner::new())?;
            #[derive(Serialize)]
            struct Payload {
                enabled: bool,
                plist: String,
            }
            crate::commands::write_result(
                env,
                out,
                &Payload {
                    enabled: false,
                    plist,
                },
                |w| writeln!(w, "Start at Login disabled."),
            )
        }
        other => Err(CliError::usage(format!(
            "unknown settings login-item subcommand {other:?}"
        ))),
    }
}

pub fn run_cli_command(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if has_help_flag(args) && args.len() == 1 {
        crate::schema::print_topic_help(out, "cli")?;
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "status";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    let specs = FlagSpecs::new().value("dir");
    let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
        return Ok(());
    };
    let dir = if parsed.string("dir").is_empty() {
        env.cli_bin_dir.to_string_lossy().into_owned()
    } else {
        parsed.string("dir")
    };
    match sub {
        "status" => {
            let status = crate::wrapper::cli_status(&dir, &env.app_path, &env.login_program);
            crate::commands::write_result(env, out, &status, |w| {
                writeln!(
                    w,
                    "CLI wrapper: installed={} path={} inPath={}",
                    status.installed, status.wrapper_path, status.in_path
                )
            })
        }
        "install" => {
            let status =
                crate::wrapper::install_cli_wrapper(&dir, &env.app_path, &env.login_program)?;
            crate::commands::write_result(env, out, &status, |w| {
                writeln!(w, "Installed CLI wrapper at {}.", status.wrapper_path)
            })
        }
        "uninstall" => {
            // The Go contract accepts and ignores `--yes` here.
            let _ = parsed.flag("yes");
            crate::wrapper::uninstall_cli_wrapper(&dir)?;
            let status = crate::wrapper::cli_status(&dir, &env.app_path, &env.login_program);
            #[derive(Serialize)]
            struct Payload {
                removed: bool,
                status: crate::wrapper::CLIInstallStatus,
            }
            crate::commands::write_result(
                env,
                out,
                &Payload {
                    removed: true,
                    status,
                },
                |w| writeln!(w, "Removed CLI wrapper."),
            )
        }
        other => Err(CliError::usage(format!("unknown cli subcommand {other:?}"))),
    }
}
