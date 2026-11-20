//! The public `devbase` CLI, ported from `internal/devcli` in pure Rust.
//!
//! This crate is a library: the shipped app dispatches into it before nsh
//! starts. The `devcli` binary in this crate is a test/verification harness
//! for the same functions, not a second shipped executable.
//!
//! ```no_run
//! let code = devcli::run_cli(
//!     &["schema".to_string()],
//!     &mut std::io::empty(),
//!     &mut std::io::sink(),
//!     &mut std::io::sink(),
//! );
//! assert_eq!(code, 0);
//! ```

pub mod args;
pub mod commands;
pub mod env;
pub mod error;
pub mod internal;
pub mod output;
pub mod schema;
pub mod wrapper;

use std::io::{Read, Write};

pub use env::{CliEnv, HelperStatus};
pub use error::{CliError, CliResult};
pub use internal::{
    INTERNAL_COMMANDS, InternalRunner, NO_COMMAND_LINE, SystemInternal, parse_owner_uid,
    run_internal, run_internal_with,
};

use crate::args::{strip_global_flags, wants_json_flag};
use crate::output::{write_action_error_json, write_human_error};

/// `RunCLI`: returns the process exit code 0, 1, or 2.
pub fn run_cli(
    args: &[String],
    input: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let mut input_bytes = Vec::new();
    let _ = input.read_to_end(&mut input_bytes);
    let mut env = CliEnv::default_env(Box::new(std::io::Cursor::new(input_bytes)));
    env.json = wants_json_flag(args);
    match run_cli_with_env(args, &mut env, out, err) {
        Ok(()) => 0,
        Err(error) => {
            if env.json {
                let _ = write_action_error_json(err, error.message(), error.exit_code());
            } else {
                let _ = write_human_error(err, error.message());
            }
            error.exit_code()
        }
    }
}

/// `RunCLIWithEnv`: execute against an injectable environment.
pub fn run_cli_with_env(
    args: &[String],
    env: &mut CliEnv,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> CliResult<()> {
    if wants_json_flag(args) {
        env.json = true;
    }
    let (cleaned, json_flag) = strip_global_flags(args)?;
    if json_flag {
        env.json = true;
    }
    let cleaned = if cleaned.is_empty() {
        vec!["help".to_string()]
    } else {
        cleaned
    };
    let command = cleaned[0].as_str();
    let rest = &cleaned[1..];
    match command {
        "help" | "--help" | "-h" => schema::run_help_command(out, rest),
        "version" | "--version" | "-v" => schema::run_version_command(out, env.json, rest),
        "schema" => schema::run_schema_command(out, rest),
        "status" => commands::workspace::run_status_command(env, out, rest),
        "diagnostics" => commands::workspace::run_diagnostics_command(env, out, rest),
        "tasks" => commands::workspace::run_tasks_command(env, out, rest),
        "logs" => commands::workspace::run_logs_command(env, out, rest),
        "ports" => commands::workspace::run_ports_command(env, out, rest),
        "shutdown" => commands::workspace::run_shutdown_command(env, out, rest),
        "software" => commands::software::run_software_command(env, out, rest),
        "install" => commands::software::run_software_install_command(env, out, rest),
        "db" | "database" => commands::databases::run_database_command(env, out, rest),
        "https" | "proxy" => commands::https::run_https_command(env, out, rest),
        "tunnel" => commands::https::run_tunnel_command(env, out, rest),
        "ca" => commands::https::run_ca_command(env, out, rest),
        "services" | "service" => commands::services::run_services_command(env, out, rest),
        "settings" => commands::settings::run_settings_command(env, out, rest),
        "log-rotation" => commands::settings::run_settings_log_rotation(env, out, rest),
        "cli" => commands::settings::run_cli_command(env, out, rest),
        other => Err(CliError::usage(format!(
            "unknown command {other:?} (run \"devbase help\" or \"devbase schema --json\")"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|value| value.to_string()).collect();
        let mut input = std::io::empty();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_cli(&args, &mut input, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn exit_codes_are_zero_one_two() {
        assert_eq!(run(&["version"]).0, 0);
        assert_eq!(run(&["nope"]).0, 2);
        assert_eq!(run(&["--json", "nope"]).0, 2);
    }

    #[test]
    fn json_error_shape_matches_go() {
        let (code, out, err) = run(&["--json", "nope"]);
        assert_eq!(code, 2);
        assert!(out.is_empty());
        assert_eq!(
            err,
            "{\n  \"error\": \"unknown command \\\"nope\\\" (run \\\"devbase help\\\" or \\\"devbase schema --json\\\")\",\n  \"exitCode\": 2\n}\n"
        );
    }

    #[test]
    fn argv_secret_is_rejected_with_exit_two_and_no_stdout() {
        for flag in ["--password=x", "--token=x", "--api-token=x"] {
            let (code, out, err) = run(&["status", flag]);
            assert_eq!(code, 2);
            assert!(out.is_empty());
            assert!(
                err.starts_with("Error: passing secrets in command-line arguments is forbidden")
            );
        }
    }

    #[test]
    fn help_lists_ported_subcommands() {
        let (code, out, _) = run(&["help"]);
        assert_eq!(code, 0);
        for topic in ["services", "software", "db", "https", "schema", "cli"] {
            assert!(out.contains(topic), "missing {topic}");
        }
    }
}
