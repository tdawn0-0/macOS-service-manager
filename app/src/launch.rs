//! The same-binary dispatch, ported from `main.go:24-65`.
//!
//! `--dev` goes to the shipped internal commands, a public-CLI-shaped argv
//! goes to the `devbase` contract, and only an empty or GUI-shaped argv falls
//! through to the window. [`classify`] is the Go precedence verbatim;
//! [`run`] hands each verdict to a [`Seams`] implementation, which is the
//! production process in `main.rs` and a recording fake in tests.

use std::process::ExitCode;

/// What an argv (without the program name) asks the binary to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// `--dev` followed by the internal command and its arguments.
    Internal(Vec<String>),
    /// The public `devbase` CLI and its arguments, `-psn_` entries removed.
    PublicCli(Vec<String>),
    /// The nsh window, opened hidden in the menu bar when `background`.
    Gui { background: bool },
}

/// The three ways the binary can run.
pub trait Seams {
    /// `devcli::run_internal_with`, with only the shipped internal commands.
    fn internal(&self, args: &[String]) -> ExitCode;
    /// `devcli::run_cli_with_env`, with the public contract.
    fn public_cli(&self, args: &[String]) -> ExitCode;
    /// Starts the nsh app; `background` is the login item's `--background`.
    fn gui(&self, background: bool) -> ExitCode;
}

/// `main.go:68-91`: whether the arguments name the public CLI.
///
/// Finder launch arguments (`-psn_…`) are dropped. `gui` and `open` are the
/// wrapper's window verbs. An argv of only `--background` and AppKit launch
/// arguments (`-NS…`, `-Apple…`, each optionally with a following value) is a
/// window launch, not a CLI invocation.
pub fn should_run_public_cli(args: &[String]) -> Option<Vec<String>> {
    let filtered: Vec<String> = args
        .iter()
        .filter(|argument| !argument.starts_with("-psn_"))
        .cloned()
        .collect();
    if filtered.is_empty() {
        return None;
    }
    if filtered[0] == "gui" || filtered[0] == "open" {
        return None;
    }
    let mut only_gui_launch_args = true;
    let mut index = 0;
    while index < filtered.len() {
        let argument = &filtered[index];
        if argument == "--background" {
            index += 1;
            continue;
        }
        if argument.starts_with("-NS") || argument.starts_with("-Apple") {
            if !argument.contains('=')
                && index + 1 < filtered.len()
                && !filtered[index + 1].starts_with('-')
            {
                index += 1;
            }
            index += 1;
            continue;
        }
        only_gui_launch_args = false;
        break;
    }
    if only_gui_launch_args {
        return None;
    }
    Some(filtered)
}

/// `main.go:92-113`: internal first, then the public CLI, then the window.
pub fn classify(args: &[String]) -> Launch {
    if args.first().is_some_and(|argument| argument == "--dev") {
        return Launch::Internal(args[1..].to_vec());
    }
    if let Some(cli_args) = should_run_public_cli(args) {
        return Launch::PublicCli(cli_args);
    }
    Launch::Gui {
        background: args.iter().any(|argument| argument == "--background"),
    }
}

/// Runs `argv` (without the program name) through the classified seam.
pub fn run(args: &[String], seams: &dyn Seams) -> ExitCode {
    match classify(args) {
        Launch::Internal(args) => seams.internal(&args),
        Launch::PublicCli(args) => seams.public_cli(&args),
        Launch::Gui { background } => seams.gui(background),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn dev_wins_before_everything_else() {
        assert_eq!(
            classify(&args(&["--dev", "serve", "/tmp/root"])),
            Launch::Internal(args(&["serve", "/tmp/root"]))
        );
        assert_eq!(classify(&args(&["--dev"])), Launch::Internal(Vec::new()));
        // Only the first argument, as `main.go:81` checks.
        assert_eq!(
            classify(&args(&["version", "--dev"])),
            Launch::PublicCli(args(&["version", "--dev"]))
        );
    }

    #[test]
    fn a_cli_argv_goes_to_the_public_contract() {
        assert_eq!(
            classify(&args(&["version"])),
            Launch::PublicCli(args(&["version"]))
        );
        assert_eq!(
            classify(&args(&["-psn_0_12345", "status"])),
            Launch::PublicCli(args(&["status"]))
        );
        assert_eq!(
            classify(&args(&["--json", "status"])),
            Launch::PublicCli(args(&["--json", "status"]))
        );
    }

    #[test]
    fn gui_launches_fall_through() {
        assert_eq!(
            classify(&[]),
            Launch::Gui { background: false }
        );
        assert_eq!(
            classify(&args(&["--background"])),
            Launch::Gui { background: true }
        );
        assert_eq!(
            classify(&args(&["gui", "anything"])),
            Launch::Gui { background: false }
        );
        assert_eq!(
            classify(&args(&["-NSFoo"])) ,
            Launch::Gui { background: false }
        );
        assert_eq!(
            classify(&args(&["-ApplePersistenceIgnoreState", "YES"])),
            Launch::Gui { background: false }
        );
        assert_eq!(
            classify(&args(&["--background", "-NSFoo", "bar"])),
            Launch::Gui { background: true }
        );
    }
}
