//! Global argument handling and the per-command flag parser.
//!
//! These are literal ports of `foundation.go`:
//! - secrets are rejected anywhere before `--`, exit 2;
//! - `--json` / `--json=true` / `--json=false` are stripped before `--`, the
//!   last occurrence winning;
//! - per-command flags may appear before or after positionals, `--` ends
//!   flag parsing, known non-boolean flags consume the following argument,
//!   and unknown flags are usage errors.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use crate::error::CliError;

/// The exact stdin advice strings. They are contract text.
pub const PASSWORD_STDIN_ADVICE: &str = "passing secrets in command-line arguments is forbidden; pipe the password on stdin and use --password-stdin";
pub const TOKEN_STDIN_ADVICE: &str = "passing secrets in command-line arguments is forbidden; pipe the token on stdin and use --token-stdin";
pub const API_TOKEN_STDIN_ADVICE: &str = "passing secrets in command-line arguments is forbidden; pipe the API token on stdin and use --token-stdin";

pub fn reject_secret_flags(args: &[String]) -> Result<(), CliError> {
    for arg in args {
        if arg == "--" {
            break;
        }
        if arg == "--password" || arg.starts_with("--password=") {
            return Err(CliError::usage(PASSWORD_STDIN_ADVICE));
        }
        if arg == "--token" || arg.starts_with("--token=") {
            return Err(CliError::usage(TOKEN_STDIN_ADVICE));
        }
        if arg == "--api-token" || arg.starts_with("--api-token=") {
            return Err(CliError::usage(API_TOKEN_STDIN_ADVICE));
        }
    }
    Ok(())
}

/// The settings `wantsJSONFlag` reads: before `--`, last flag wins.
pub fn wants_json_flag(args: &[String]) -> bool {
    let mut json = false;
    for arg in args {
        if arg == "--" {
            break;
        }
        if arg == "--json" || arg == "--json=true" {
            json = true;
        } else if arg == "--json=false" {
            json = false;
        }
    }
    json
}

/// Strip the global `--json*` flags; `--` is preserved.
pub fn strip_global_flags(args: &[String]) -> Result<(Vec<String>, bool), CliError> {
    reject_secret_flags(args)?;
    let mut cleaned = Vec::with_capacity(args.len());
    let mut json = false;
    let mut after_double_dash = false;
    for arg in args {
        if arg == "--" {
            after_double_dash = true;
            cleaned.push(arg.clone());
            continue;
        }
        if !after_double_dash {
            if arg == "--json" || arg == "--json=true" {
                json = true;
                continue;
            }
            if arg == "--json=false" {
                json = false;
                continue;
            }
        }
        cleaned.push(arg.clone());
    }
    Ok((cleaned, json))
}

/// `readSecretStdin`: at most 8192 bytes, trailing CR/LF trimmed, not empty.
pub fn read_secret_stdin(reader: &mut dyn Read, label: &str) -> Result<String, CliError> {
    let mut bytes = Vec::new();
    let mut limited = reader.take(8193);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| CliError::operation(error.to_string()))?;
    if bytes.len() > 8192 {
        return Err(CliError::usage(format!(
            "{label} on stdin exceeds 8192 bytes"
        )));
    }
    let secret = String::from_utf8_lossy(&bytes)
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if secret.trim().is_empty() {
        return Err(CliError::usage(format!("{label} on stdin cannot be empty")));
    }
    Ok(secret)
}

/// `readJSONStdin`: byte limit and non-empty.
pub fn read_json_stdin(reader: &mut dyn Read, limit: u64) -> Result<Vec<u8>, CliError> {
    let mut bytes = Vec::new();
    let mut limited = reader.take(limit + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| CliError::operation(error.to_string()))?;
    if bytes.len() as u64 > limit {
        return Err(CliError::usage(format!("JSON stdin exceeds {limit} bytes")));
    }
    if String::from_utf8_lossy(&bytes).trim().is_empty() {
        return Err(CliError::usage("JSON stdin cannot be empty"));
    }
    Ok(bytes)
}

/// The flags a command declares.
#[derive(Default, Clone)]
pub struct FlagSpecs {
    bools: BTreeSet<String>,
    values: BTreeSet<String>,
    repeatable: BTreeSet<String>,
}

impl FlagSpecs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bool(mut self, name: &str) -> Self {
        self.bools.insert(name.to_string());
        self
    }

    pub fn value(mut self, name: &str) -> Self {
        self.values.insert(name.to_string());
        self
    }

    pub fn repeatable(mut self, name: &str) -> Self {
        self.values.insert(name.to_string());
        self.repeatable.insert(name.to_string());
        self
    }

    pub fn is_bool(&self, name: &str) -> bool {
        self.bools.contains(name)
    }

    pub fn is_known(&self, name: &str) -> bool {
        self.bools.contains(name) || self.values.contains(name)
    }
}

/// Parsed flags plus positionals.
#[derive(Default, Debug, Clone)]
pub struct Parsed {
    values: BTreeMap<String, Vec<String>>,
    bools: BTreeSet<String>,
    pub positionals: Vec<String>,
}

impl Parsed {
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    pub fn all(&self, name: &str) -> &[String] {
        self.values.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn is_set(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.bools.contains(name)
    }

    pub fn string(&self, name: &str) -> String {
        self.value(name).unwrap_or_default().to_string()
    }

    pub fn integer(&self, name: &str) -> Result<Option<i64>, CliError> {
        match self.value(name) {
            None => Ok(None),
            Some(raw) => raw.parse::<i64>().map(Some).map_err(|_| {
                CliError::usage(format!("invalid value {raw:?} for -{name}: parse error"))
            }),
        }
    }

    pub fn boolean(&self, name: &str) -> Result<bool, CliError> {
        match self.value(name) {
            None => Ok(self.flag(name)),
            Some(raw) => match raw {
                "true" | "1" | "t" | "T" | "TRUE" | "True" => Ok(true),
                "false" | "0" | "f" | "F" | "FALSE" | "False" => Ok(false),
                _ => Err(CliError::usage(format!(
                    "invalid value {raw:?} for -{name}: parse error"
                ))),
            },
        }
    }
}

/// A parsed flag carrying help, which every command handles by printing help.
#[derive(Debug)]
pub enum ParseOutcome {
    Parsed(Parsed),
    Help,
}

/// `parseFlags`, including Go's interleaving and `--` rules.
pub fn parse_flags(args: &[String], specs: &FlagSpecs) -> Result<ParseOutcome, CliError> {
    let mut parsed = Parsed::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            parsed.positionals.extend(args[i + 1..].iter().cloned());
            break;
        }
        if arg.starts_with('-') && arg != "-" {
            let stripped = arg
                .strip_prefix("--")
                .unwrap_or_else(|| arg.strip_prefix('-').unwrap_or(arg));
            let (name, inline) = match stripped.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (stripped, None),
            };
            if name == "h" || name == "help" {
                return Ok(ParseOutcome::Help);
            }
            if !specs.is_known(name) {
                return Err(CliError::usage(format!(
                    "flag provided but not defined: -{name}"
                )));
            }
            if specs.is_bool(name) {
                match inline {
                    Some(value) => {
                        parsed
                            .values
                            .entry(name.to_string())
                            .or_default()
                            .push(value.to_string());
                    }
                    None => {
                        parsed.bools.insert(name.to_string());
                    }
                }
                i += 1;
                continue;
            }
            let value = match inline {
                Some(value) => value.to_string(),
                None => {
                    if i + 1 >= args.len() {
                        return Err(CliError::usage(format!("flag needs an argument: {arg}")));
                    }
                    i += 1;
                    args[i].clone()
                }
            };
            parsed
                .values
                .entry(name.to_string())
                .or_default()
                .push(value);
            i += 1;
            continue;
        }
        parsed.positionals.push(arg.clone());
        i += 1;
    }
    Ok(ParseOutcome::Parsed(parsed))
}

/// `hasHelpFlag`: `--help`/`-h` before `--`.
pub fn has_help_flag(args: &[String]) -> bool {
    for arg in args {
        if arg == "--" {
            return false;
        }
        if arg == "--help" || arg == "-h" {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn rejects_all_secret_forms() {
        for flag in [
            "--password",
            "--password=x",
            "--token",
            "--token=x",
            "--api-token",
            "--api-token=x",
        ] {
            let err = reject_secret_flags(&strings(&["db", flag])).unwrap_err();
            assert_eq!(err.exit_code(), 2);
        }
    }

    #[test]
    fn secret_after_double_dash_is_positional() {
        reject_secret_flags(&strings(&["--", "--password=keep"])).unwrap();
    }

    #[test]
    fn json_last_wins_and_double_dash_stops() {
        assert!(wants_json_flag(&strings(&["--json", "x"])));
        assert!(!wants_json_flag(&strings(&["--json", "--json=false"])));
        assert!(wants_json_flag(&strings(&["--json=false", "--json"])));
        assert!(!wants_json_flag(&strings(&["--", "--json"])));
        let (cleaned, json) =
            strip_global_flags(&strings(&["a", "--json", "--", "--json"])).unwrap();
        assert!(json);
        assert_eq!(cleaned, strings(&["a", "--", "--json"]));
    }

    #[test]
    fn flags_interleave_with_positionals() {
        let specs = FlagSpecs::new().bool("yes").value("source");
        let ParseOutcome::Parsed(parsed) = parse_flags(
            &strings(&["one", "--source", "daemon", "--yes", "two"]),
            &specs,
        )
        .unwrap() else {
            panic!("help");
        };
        assert_eq!(parsed.positionals, strings(&["one", "two"]));
        assert_eq!(parsed.value("source"), Some("daemon"));
        assert!(parsed.flag("yes"));
    }

    #[test]
    fn unknown_flag_is_usage_error() {
        let err = parse_flags(&strings(&["--nope"]), &FlagSpecs::new()).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn double_dash_keeps_positionals() {
        let specs = FlagSpecs::new();
        let ParseOutcome::Parsed(parsed) =
            parse_flags(&strings(&["--", "--looks-like-a-flag"]), &specs).unwrap()
        else {
            panic!("help");
        };
        assert_eq!(parsed.positionals, strings(&["--looks-like-a-flag"]));
    }

    #[test]
    fn stdin_readers_match_limits() {
        let mut input = &b"secret\r\n"[..];
        assert_eq!(read_secret_stdin(&mut input, "password").unwrap(), "secret");
        let mut empty = &b"\n"[..];
        assert_eq!(
            read_secret_stdin(&mut empty, "password")
                .unwrap_err()
                .exit_code(),
            2
        );
        let mut too_big = vec![b'a'; 8193];
        assert_eq!(
            read_secret_stdin(&mut &too_big[..], "password")
                .unwrap_err()
                .exit_code(),
            2
        );
        too_big.truncate(1);
        let mut json = &b"  "[..];
        assert_eq!(read_json_stdin(&mut json, 10).unwrap_err().exit_code(), 2);
    }
}
