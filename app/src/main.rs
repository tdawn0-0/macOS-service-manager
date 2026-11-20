//! Runs the app directly on the machine's native backend (`cargo run`).
//!
//! Before the window opens, the argv is dispatched exactly as the Go
//! `main.go:81-97` does: `--dev` runs the shipped internal commands, a
//! public-CLI-shaped argv runs the `devbase` contract, and only the window
//! falls through to nsh. The production seams are `devcli`'s env and
//! internal runner (`settings.go:31-54` wires the same client into the
//! screen) and `nsh_platform`'s `run_app`.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

use std::{
    io::{Read, Write},
    process::ExitCode,
};

use nsh_app_devbase::launch::{self, Seams};

/// The production dispatch: devcli for both CLI shapes, nsh for the window.
struct SystemSeams;

impl Seams for SystemSeams {
    fn internal(&self, args: &[String]) -> ExitCode {
        let mut out = std::io::stdout();
        let mut err = std::io::stderr();
        match devcli::run_internal_with(args, &devcli::SystemInternal, &mut out, &mut err) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                // `main.go:85-87` logs the error and exits 1.
                let _ = writeln!(err, "{error}");
                ExitCode::FAILURE
            }
        }
    }

    fn public_cli(&self, args: &[String]) -> ExitCode {
        let mut input = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut input);
        let mut env =
            devcli::CliEnv::default_env(Box::new(std::io::Cursor::new(input)));
        let mut out = std::io::stdout();
        let mut err = std::io::stderr();
        match devcli::run_cli_with_env(args, &mut env, &mut out, &mut err) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                // `run_cli` formats a failed command after `RunCLIWithEnv`;
                // keep that contract while dispatching through the env seam.
                if env.json {
                    let _ =
                        devcli::output::write_action_error_json(&mut err, error.message(), error.exit_code());
                } else {
                    let _ = devcli::output::write_human_error(&mut err, error.message());
                }
                ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1))
            }
        }
    }

    fn gui(&self, background: bool) -> ExitCode {
        platform::run_app(
            platform::AppSource::Rust(platform::Root::of::<nsh_app_devbase::App>()),
            platform::AppOptions {
                title: "Devbase".to_owned(),
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                app_id: "xyz.jyeu.devbase".to_owned(),
                size: (1100, 720),
                background,
                keep_alive_on_close: true,
                ..Default::default()
            },
        )
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> ExitCode {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    launch::run(&args, &SystemSeams)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!(
    "Run this binary on macOS, Windows or Linux; the app targets macOS 26 and later."
);
