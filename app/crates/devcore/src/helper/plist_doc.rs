//! The helper's own property list and artifact identity.
//!
//! Ported from `client.go`: `BinaryInfo`, `CurrentBinaryInfo`,
//! `appBundleForExecutable`, `expectedStagedExecutable`, `hasOwnerArg`,
//! `helperPlistDoc` and `buildHelperPlist`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use plist::{Dictionary, Value};

use super::client::BinaryInfo;
use super::{HELPER_DIR, LABEL, SOCKET_NAME, STDERR_LOG};

/// The subset of the helper plist `Status` inspects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelperPlistDoc {
    pub label: String,
    pub program_arguments: Vec<String>,
    pub sockets: BTreeMap<String, HelperSocketDoc>,
}

/// One entry in the helper plist's `Sockets` dictionary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelperSocketDoc {
    pub sock_path_name: String,
    pub sock_path_mode: i64,
}

/// Reads the helper property list, rejecting malformed fields the way Go's
/// plist decoder into `helperPlistDoc` does.
pub fn parse_helper_plist(data: &[u8]) -> Result<HelperPlistDoc, String> {
    let value = Value::from_reader(std::io::Cursor::new(data))
        .map_err(|error| format!("invalid plist: {error}"))?;
    let Value::Dictionary(dict) = value else {
        return Err("invalid plist: top level is not a dictionary".to_string());
    };
    let label = match dict.get("Label") {
        None => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(_) => return Err("invalid plist: Label is not a string".to_string()),
    };
    let mut program_arguments = Vec::new();
    if let Some(value) = dict.get("ProgramArguments") {
        let Value::Array(items) = value else {
            return Err("invalid plist: ProgramArguments is not an array".to_string());
        };
        for item in items {
            let Value::String(text) = item else {
                return Err("invalid plist: ProgramArguments item is not a string".to_string());
            };
            program_arguments.push(text.clone());
        }
    }
    let mut sockets = BTreeMap::new();
    if let Some(value) = dict.get("Sockets") {
        let Value::Dictionary(entries) = value else {
            return Err("invalid plist: Sockets is not a dictionary".to_string());
        };
        for (name, entry) in entries {
            let Value::Dictionary(entry) = entry else {
                return Err(format!("invalid plist: socket {name} is not a dictionary"));
            };
            let sock_path_name = match entry.get("SockPathName") {
                None => String::new(),
                Some(Value::String(text)) => text.clone(),
                Some(_) => {
                    return Err(format!("invalid plist: socket {name} has no SockPathName"));
                }
            };
            let sock_path_mode = match entry.get("SockPathMode") {
                None => 0,
                Some(Value::Integer(value)) => value
                    .as_signed()
                    .ok_or_else(|| format!("invalid plist: socket {name} mode is too large"))?,
                Some(_) => {
                    return Err(format!("invalid plist: socket {name} mode is not a number"));
                }
            };
            sockets.insert(
                name.clone(),
                HelperSocketDoc {
                    sock_path_name,
                    sock_path_mode,
                },
            );
        }
    }
    Ok(HelperPlistDoc {
        label,
        program_arguments,
        sockets,
    })
}

/// Builds the helper's LaunchDaemon property list.
///
/// launchd runs the staged executable as root, on demand, on the named socket.
/// There is no `UserName`, `RunAtLoad` or `KeepAlive`.
pub fn build_helper_plist(
    staged_exe: &Path,
    owner_uid: u32,
    socket_path: &str,
) -> Result<Vec<u8>, String> {
    let staged = staged_exe
        .to_str()
        .ok_or_else(|| "staged helper path is not valid UTF-8".to_string())?;
    let mut socket = Dictionary::new();
    socket.insert("SockPathMode".to_string(), Value::Integer(0o666.into()));
    socket.insert(
        "SockPathName".to_string(),
        Value::String(socket_path.to_string()),
    );
    let mut sockets = Dictionary::new();
    sockets.insert(SOCKET_NAME.to_string(), Value::Dictionary(socket));
    let mut doc = Dictionary::new();
    doc.insert("Label".to_string(), Value::String(LABEL.to_string()));
    doc.insert(
        "ProgramArguments".to_string(),
        Value::Array(vec![
            Value::String(staged.to_string()),
            Value::String("--dev".to_string()),
            Value::String("helper".to_string()),
            Value::String("--owner-uid".to_string()),
            Value::String(owner_uid.to_string()),
        ]),
    );
    doc.insert("Sockets".to_string(), Value::Dictionary(sockets));
    doc.insert(
        "StandardErrorPath".to_string(),
        Value::String(STDERR_LOG.to_string()),
    );
    let mut out = Vec::new();
    Value::Dictionary(doc)
        .to_writer_xml(&mut out)
        .map_err(|error| error.to_string())?;
    Ok(out)
}

/// The path launchd runs after installation: the whole staged bundle's
/// executable when the running app is a bundle, else a staged single binary.
pub fn expected_staged_executable(helper_dir: &Path, info: &BinaryInfo) -> PathBuf {
    if info.bundle_path.is_some() {
        let exe = info
            .executable
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return helper_dir
            .join("bundle")
            .join("Contents")
            .join("MacOS")
            .join(exe);
    }
    helper_dir.join("devbase-helper")
}

/// Whether `--owner-uid <uid>` appears as an adjacent pair anywhere in the
/// helper's program arguments.
pub fn has_owner_arg(args: &[String], uid: u32) -> bool {
    let want = uid.to_string();
    args.windows(2)
        .any(|pair| pair[0] == "--owner-uid" && pair[1] == want)
}

/// The application bundle containing `exe`, when the executable sits at
/// `<Envelope>/Contents/MacOS/<binary>` and `Contents/Info.plist` exists.
pub fn app_bundle_for_executable(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    if dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = dir.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    if !contents.join("Info.plist").exists() {
        return None;
    }
    contents.parent().map(Path::to_path_buf)
}

/// The artifact identity for an executable and owner, with symlinks resolved
/// when the executable resolves.
pub fn binary_info_for(executable: &Path, owner_uid: u32) -> BinaryInfo {
    let resolved = std::fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    let resolved = if resolved.as_os_str().is_empty() {
        executable.to_path_buf()
    } else {
        resolved
    };
    BinaryInfo {
        bundle_path: app_bundle_for_executable(&resolved),
        executable: resolved,
        owner_uid,
    }
}

/// The artifact identity of the running process.
pub fn current_binary_info() -> BinaryInfo {
    let executable = std::env::current_exe().unwrap_or_default();
    binary_info_for(&executable, current_euid())
}

/// The effective UID of this process.
pub fn current_euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// The default helper directory, exposed for callers that need it for display.
pub const fn default_helper_dir() -> &'static str {
    HELPER_DIR
}
