//! One module per Go command file, as the port proposal describes.

pub mod databases;
pub mod https;
pub mod services;
pub mod settings;
pub mod software;
pub mod workspace;

use std::io::Write;

use crate::env::CliEnv;
use crate::error::{CliError, CliResult};
use crate::output::write_json;
use crate::wrapper;

/// `writeResult`: JSON when `--json`, otherwise the human writer.
pub fn write_result(
    env: &CliEnv,
    out: &mut dyn Write,
    payload: &impl serde::Serialize,
    human: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
) -> CliResult<()> {
    if env.json {
        write_json(out, payload)?;
        return Ok(());
    }
    human(out)?;
    Ok(())
}

/// The shared workspace store for this invocation.
pub fn store(env: &CliEnv) -> devcore::state::Store {
    devcore::state::Store::new(env.root.clone())
}

/// `CLIStatus(env.CLIBinDir, env.AppPath, env.LoginProgram)`.
pub fn cli_status(env: &CliEnv) -> wrapper::CLIInstallStatus {
    wrapper::cli_status(
        &env.cli_bin_dir.to_string_lossy(),
        &env.app_path,
        &env.login_program,
    )
}

/// Reject a positional count with the caller's usage text.
pub fn require_positionals(positionals: &[String], expected: usize, usage: &str) -> CliResult<()> {
    if positionals.len() != expected {
        return Err(CliError::usage(usage.to_string()));
    }
    Ok(())
}

/// `--yes` gating text: the same shape every destructive command uses.
pub fn require_yes(flag: bool, message: &str) -> CliResult<()> {
    if !flag {
        return Err(CliError::usage(message.to_string()));
    }
    Ok(())
}

/// Print a topic help page when `-h/--help` is anywhere before `--`.
pub fn maybe_help(
    env: &CliEnv,
    out: &mut dyn Write,
    args: &[String],
    topic: &str,
) -> CliResult<bool> {
    let _ = env;
    if crate::args::has_help_flag(args) {
        crate::schema::print_topic_help(out, topic)?;
        return Ok(true);
    }
    Ok(false)
}
