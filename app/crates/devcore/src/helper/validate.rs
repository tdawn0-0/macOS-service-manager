//! Every validation rule the Go helper server applies, ported one-to-one.
//!
//! These are deliberately strict: a relaxed path normalization, symlink
//! acceptance, helper self-protection gap or extra `launchctl` subcommand
//! would widen what an authorized caller can do as root.

use std::path::Path;

use super::paths::{
    base_name, clean_path, dir_path, is_digits, reject_symlink_if_exists,
    validate_clean_target_file,
};
use super::{HELPER_DIR, LABEL, PLIST_PATH};

/// Whether a launchd target or property-list path names the helper itself.
pub fn targets_helper(target: &str, path: &str) -> bool {
    if target == LABEL
        || target == format!("system/{LABEL}")
        || target.ends_with(&format!("/{LABEL}"))
    {
        return true;
    }
    if !path.is_empty() {
        let clean = clean_path(path);
        if clean == PLIST_PATH
            || base_name(&clean) == format!("{LABEL}.plist")
            || clean.starts_with(HELPER_DIR)
        {
            return true;
        }
    }
    false
}

/// A non-empty XML property list whose `Label` is present and is not the
/// helper's own. The label need not match the path or target.
pub fn validate_xml_plist(data: &[u8]) -> Result<(), String> {
    if data.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Err("plist data is empty".to_string());
    }
    let value = plist::Value::from_reader(std::io::Cursor::new(data))
        .map_err(|error| format!("invalid plist: {error}"))?;
    let plist::Value::Dictionary(dict) = value else {
        return Err("invalid plist: top level is not a dictionary".to_string());
    };
    let label = match dict.get("Label") {
        Some(plist::Value::String(text)) => text.as_str(),
        _ => "",
    };
    if label.is_empty() {
        return Err("plist is missing Label".to_string());
    }
    if label == LABEL {
        return Err(format!("cannot write helper label {label:?}"));
    }
    Ok(())
}

/// `system`, `gui/<digits>` or `user/<digits>`.
pub fn validate_domain(domain: &str) -> Result<(), String> {
    if domain == "system" {
        return Ok(());
    }
    if let Some(uid) = domain.strip_prefix("gui/")
        && is_digits(uid)
    {
        return Ok(());
    }
    if let Some(uid) = domain.strip_prefix("user/")
        && is_digits(uid)
    {
        return Ok(());
    }
    Err(format!("invalid launchd domain {domain:?}"))
}

/// `system/<label>` (no further slash), `gui/<digits>/<label>` or
/// `user/<digits>/<label>`.
pub fn validate_launchd_target(target: &str) -> Result<(), String> {
    let Some((domain, rest)) = target.split_once('/') else {
        return Err(format!("invalid launchd target {target:?}"));
    };
    if rest.is_empty() {
        return Err(format!("invalid launchd target {target:?}"));
    }
    if domain == "system" {
        if rest.contains('/') {
            return Err(format!("invalid launchd target {target:?}"));
        }
        return Ok(());
    }
    if domain != "gui" && domain != "user" {
        return Err(format!("invalid launchd target {target:?}"));
    }
    let Some((uid, label)) = rest.split_once('/') else {
        return Err(format!("invalid launchd target {target:?}"));
    };
    if !is_digits(uid) || label.is_empty() || label.contains('/') {
        return Err(format!("invalid launchd target {target:?}"));
    }
    Ok(())
}

/// A proxy socket target: valid launchd target, not the helper, and in the
/// Devbase system namespace.
pub fn validate_proxy_socket_target(target: &str) -> Result<(), String> {
    validate_launchd_target(target)?;
    if targets_helper(target, "") {
        return Err("cannot target privileged helper".to_string());
    }
    if !target.starts_with("system/xyz.jyeu.devbase.") {
        return Err(format!("unexpected proxy socket target {target:?}"));
    }
    Ok(())
}

/// A proxy socket job path and target pair.
pub fn validate_proxy_socket_request(
    job_path: &str,
    target: &str,
    socket_job_dirs: &[String],
) -> Result<String, String> {
    let clean_path = validate_clean_target_file(job_path, socket_job_dirs, ".plist")?;
    if targets_helper(target, &clean_path) {
        return Err("cannot target privileged helper".to_string());
    }
    let base = base_name(&clean_path);
    if !base.starts_with("xyz.jyeu.devbase.") {
        return Err(format!("unexpected socket job path {clean_path:?}"));
    }
    validate_proxy_socket_target(target)?;
    Ok(clean_path)
}

/// A `launchctl` command vector. Every argument is checked against the helper
/// before the subcommand is allowed.
pub fn validate_launchctl_command(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err("launchctl command requires arguments".to_string());
    }
    for argument in args {
        if argument.is_empty() || argument.contains('\0') {
            return Err(format!("invalid launchctl argument {argument:?}"));
        }
        if targets_helper(argument, argument) {
            return Err("cannot target privileged helper".to_string());
        }
    }
    let name = args[0].as_str();
    match name {
        "bootstrap" => {
            if args.len() != 3 {
                return Err("bootstrap expects domain and plist path".to_string());
            }
            validate_domain(&args[1])?;
            validate_launchctl_plist_arg(&args[2])
        }
        "bootout" => {
            if args.len() != 2 {
                return Err("bootout expects target".to_string());
            }
            validate_launchd_target(&args[1])
        }
        "enable" | "disable" | "print" => {
            if args.len() != 2 {
                return Err(format!("{name} expects target"));
            }
            validate_launchd_target(&args[1])
        }
        "kickstart" => {
            let index = if args.len() == 3 && (args[1] == "-k" || args[1] == "-p") {
                2
            } else if args.len() != 2 {
                return Err("unexpected kickstart arguments".to_string());
            } else {
                1
            };
            validate_launchd_target(&args[index])
        }
        "kill" => {
            if args.len() != 3 {
                return Err("kill expects signal and target".to_string());
            }
            match args[1].as_str() {
                "SIGTERM" | "SIGKILL" | "SIGINT" | "SIGHUP" => {}
                other => return Err(format!("unsupported signal {other:?}")),
            }
            validate_launchd_target(&args[2])
        }
        other => Err(format!("unsupported launchctl subcommand {other:?}")),
    }
}

/// A `bootstrap` property-list argument: absolute, normalized, `.plist`, and
/// not a symlink.
pub fn validate_launchctl_plist_arg(path: &str) -> Result<(), String> {
    if !Path::new(path).is_absolute() || clean_path(path) != path || !path.ends_with(".plist") {
        return Err(format!("invalid plist path {path:?}"));
    }
    reject_symlink_if_exists(path)
}

/// The basename `launchd` uses for a path.
pub fn path_basename(path: &str) -> String {
    base_name(path)
}

/// The parent directory `launchd` paths are validated against.
pub fn path_parent(path: &str) -> String {
    dir_path(path)
}

// ---------------------------------------------------------------------------
// Linux systemd & CA trust helper validators
// ---------------------------------------------------------------------------

/// Whether a systemd unit name or path targets the Linux privileged helper itself.
pub fn targets_linux_helper(unit_or_path: &str) -> bool {
    if unit_or_path.is_empty() {
        return false;
    }
    let base = base_name(&clean_path(unit_or_path));
    if matches!(
        base.as_str(),
        "devbase-helper" | "devbase-helper.service" | "devbase-helper.socket"
    ) {
        return true;
    }
    let clean = clean_path(unit_or_path);
    clean == super::systemd_unit::LINUX_HELPER_SOCKET_UNIT
        || clean == super::systemd_unit::LINUX_HELPER_SERVICE_UNIT
        || clean.starts_with(super::systemd_unit::LINUX_HELPER_DIR)
}

/// Validates a `systemctl` command vector accepted by `/v1/systemd/control`.
pub fn validate_systemctl_command(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("systemctl command requires arguments".to_string());
    }
    for argument in args {
        if argument.is_empty() || argument.contains('\0') {
            return Err(format!("invalid systemctl argument {argument:?}"));
        }
        if targets_linux_helper(argument) {
            return Err("cannot target privileged helper".to_string());
        }
    }
    match args[0].as_str() {
        "daemon-reload" => {
            if args.len() != 1 {
                return Err("daemon-reload takes no arguments".to_string());
            }
            Ok(())
        }
        "start" | "stop" | "restart" | "enable" | "disable" | "reset-failed" | "is-active" => {
            if args.len() != 2 {
                return Err(format!("{} expects a single unit name", args[0]));
            }
            validate_systemd_unit_name(&args[1])
        }
        other => Err(format!("unsupported systemctl subcommand {other:?}")),
    }
}

/// Validates a systemd unit name (no slashes, no leading `-`, not the helper).
pub fn validate_systemd_unit_name(name: &str) -> Result<(), String> {
    crate::systemd::validate_any_unit_name(name).map_err(|error| error.to_string())?;
    if targets_linux_helper(name) {
        return Err("cannot target privileged helper".to_string());
    }
    if !(name.ends_with(".service") || name.ends_with(".socket") || name.ends_with(".timer")) {
        return Err(format!(
            "unit name {name:?} must end with .service, .socket, or .timer"
        ));
    }
    Ok(())
}

/// Validates a target systemd unit file path inside `allowed_dirs`.
pub fn validate_systemd_unit_file(path: &str, allowed_dirs: &[String]) -> Result<String, String> {
    let suffix = if path.ends_with(".service") {
        ".service"
    } else if path.ends_with(".socket") {
        ".socket"
    } else if path.ends_with(".timer") {
        ".timer"
    } else {
        return Err(format!(
            "path {path:?} must end with .service, .socket, or .timer"
        ));
    };
    let clean = validate_clean_target_file(path, allowed_dirs, suffix)?;
    if targets_linux_helper(&clean) {
        return Err("cannot target privileged helper".to_string());
    }
    Ok(clean)
}

/// Validates a CA anchor base name (alphanumeric + `._-`, no slashes or `..`).
pub fn validate_ca_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.len() > 128
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed.starts_with('.')
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(format!("invalid CA anchor name {name:?}"));
    }
    Ok(trimmed.to_string())
}

/// Validates that `pem` contains a PEM-encoded X.509 certificate block.
pub fn validate_ca_pem(pem: &[u8]) -> Result<(), String> {
    if pem.is_empty() || pem.len() > 256 * 1024 {
        return Err("CA certificate PEM has invalid size".to_string());
    }
    let text = std::str::from_utf8(pem)
        .map_err(|_| "CA certificate PEM must be valid UTF-8".to_string())?;
    if !text.contains("-----BEGIN CERTIFICATE-----") || !text.contains("-----END CERTIFICATE-----")
    {
        return Err("CA certificate data is not a PEM certificate".to_string());
    }
    Ok(())
}

/// Validates a Linux proxy socket + service unit pair.
pub fn validate_linux_proxy_unit_names(
    socket_unit: &str,
    service_unit: &str,
) -> Result<(), String> {
    validate_systemd_unit_name(socket_unit)?;
    validate_systemd_unit_name(service_unit)?;
    let Some(sock_stem) = socket_unit.strip_suffix(".socket") else {
        return Err(format!("expected .socket unit, got {socket_unit:?}"));
    };
    let Some(svc_stem) = service_unit.strip_suffix(".service") else {
        return Err(format!("expected .service unit, got {service_unit:?}"));
    };
    if !sock_stem.starts_with("devbase-proxy-") || sock_stem != svc_stem {
        return Err(format!(
            "proxy socket units must match devbase-proxy-* stem: {socket_unit:?} / {service_unit:?}"
        ));
    }
    Ok(())
}
