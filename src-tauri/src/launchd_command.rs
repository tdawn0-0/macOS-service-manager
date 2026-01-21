use plist::Value;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

/// Represents a launchd service in the list view
#[derive(Serialize, Deserialize, Type, Clone)]
pub struct LaunchdService {
    pub label: String,
    pub path: String,
    pub domain: String, // "user" or "system"
    pub loaded: bool,
    pub running: bool,
    pub pid: Option<i32>,
}

/// Represents detailed launchd service info
#[derive(Serialize, Deserialize, Type, Clone)]
pub struct LaunchdServiceInfo {
    pub label: String,
    pub path: String,
    pub domain: String,
    pub loaded: bool,
    pub running: bool,
    pub pid: Option<i32>,
    pub program: Option<String>,
    pub program_arguments: Option<Vec<String>>,
    pub run_at_load: Option<bool>,
    pub keep_alive: Option<bool>,
    pub working_directory: Option<String>,
    pub standard_out_path: Option<String>,
    pub standard_error_path: Option<String>,
}

#[derive(Serialize, Deserialize, Type)]
pub enum LaunchdServiceCommand {
    Load,
    Unload,
    Start,
    Stop,
}

/// Get the user's LaunchAgents directories
fn get_launchagent_directories() -> Vec<(PathBuf, String)> {
    let mut dirs = Vec::new();

    // User LaunchAgents
    if let Some(home) = dirs::home_dir() {
        let user_agents = home.join("Library/LaunchAgents");
        if user_agents.exists() {
            dirs.push((user_agents, "user".to_string()));
        }
    }

    // System-wide LaunchAgents (for all users)
    let system_agents = PathBuf::from("/Library/LaunchAgents");
    if system_agents.exists() {
        dirs.push((system_agents, "system".to_string()));
    }

    dirs
}

/// Parse a plist file to extract service label
fn get_label_from_plist(path: &PathBuf) -> Option<String> {
    let value = plist::from_file::<_, Value>(path).ok()?;
    value
        .as_dictionary()?
        .get("Label")?
        .as_string()
        .map(|s| s.to_string())
}

/// Check if a service is loaded via launchctl
fn is_service_loaded(label: &str) -> bool {
    // Try to get info about the service using launchctl print
    let uid = users::get_current_uid();
    let domain = format!("gui/{}", uid);

    let output = Command::new("launchctl")
        .args(["print", &format!("{}/{}", domain, label)])
        .output();

    match output {
        Ok(Output { status, .. }) => status.success(),
        Err(_) => false,
    }
}

/// Get PID of a running service
fn get_service_pid(label: &str) -> Option<i32> {
    let output = Command::new("launchctl")
        .args(["list"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[2] == label {
            return parts[0].parse::<i32>().ok();
        }
    }

    None
}

/// Get all launchd services from LaunchAgent directories
#[tauri::command]
#[specta::specta]
pub async fn get_launchd_services() -> Result<Vec<LaunchdService>, String> {
    let directories = get_launchagent_directories();
    let mut services = Vec::new();

    for (dir, domain) in directories {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("plist") {
                    if let Some(label) = get_label_from_plist(&path) {
                        let loaded = is_service_loaded(&label);
                        let pid = if loaded { get_service_pid(&label) } else { None };
                        let running = pid.is_some();

                        services.push(LaunchdService {
                            label,
                            path: path.to_string_lossy().to_string(),
                            domain: domain.clone(),
                            loaded,
                            running,
                            pid,
                        });
                    }
                }
            }
        }
    }

    // Sort by label
    services.sort_by(|a, b| a.label.cmp(&b.label));

    Ok(services)
}

/// Get detailed information about a specific launchd service
#[tauri::command]
#[specta::specta]
pub async fn get_launchd_service_info(path: &str) -> Result<LaunchdServiceInfo, String> {
    let path_buf = PathBuf::from(path);

    if !path_buf.exists() {
        return Err(format!("Plist file not found: {}", path));
    }

    let value = plist::from_file::<_, Value>(&path_buf)
        .map_err(|e| format!("Failed to parse plist: {}", e))?;

    let dict = value
        .as_dictionary()
        .ok_or("Invalid plist format: expected dictionary")?;

    let label = dict
        .get("Label")
        .and_then(|v| v.as_string())
        .ok_or("Missing Label in plist")?
        .to_string();

    // Determine domain based on path
    let domain = if path.contains("/Library/LaunchAgents") && !path.contains("~") {
        if path.starts_with("/Library") {
            "system".to_string()
        } else {
            "user".to_string()
        }
    } else {
        "user".to_string()
    };

    let loaded = is_service_loaded(&label);
    let pid = if loaded { get_service_pid(&label) } else { None };
    let running = pid.is_some();

    let program = dict
        .get("Program")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());

    let program_arguments = dict.get("ProgramArguments").and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_string().map(|s| s.to_string()))
                .collect()
        })
    });

    let run_at_load = dict.get("RunAtLoad").and_then(|v| v.as_boolean());

    let keep_alive = dict.get("KeepAlive").and_then(|v| v.as_boolean());

    let working_directory = dict
        .get("WorkingDirectory")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());

    let standard_out_path = dict
        .get("StandardOutPath")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());

    let standard_error_path = dict
        .get("StandardErrorPath")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());

    Ok(LaunchdServiceInfo {
        label,
        path: path.to_string(),
        domain,
        loaded,
        running,
        pid,
        program,
        program_arguments,
        run_at_load,
        keep_alive,
        working_directory,
        standard_out_path,
        standard_error_path,
    })
}

/// Manage a launchd service (load/unload/start/stop)
#[tauri::command]
#[specta::specta]
pub async fn manage_launchd_service(
    label: &str,
    path: &str,
    command: LaunchdServiceCommand,
) -> Result<String, String> {
    let uid = users::get_current_uid();
    let domain = format!("gui/{}", uid);

    let (cmd_args, description): (Vec<&str>, &str) = match command {
        LaunchdServiceCommand::Load => {
            // Use bootstrap to load the service
            (vec!["bootstrap", &domain, path], "load")
        }
        LaunchdServiceCommand::Unload => {
            // Use bootout to unload the service
            let target = format!("{}/{}", domain, label);
            return match Command::new("launchctl")
                .args(["bootout", &target])
                .output()
            {
                Ok(Output { status, stderr, .. }) => {
                    if status.success() {
                        Ok("Service unloaded successfully".to_string())
                    } else {
                        Err(String::from_utf8_lossy(&stderr).into_owned())
                    }
                }
                Err(e) => Err(format!("Failed to execute command: {}", e)),
            };
        }
        LaunchdServiceCommand::Start => {
            let target = format!("{}/{}", domain, label);
            return match Command::new("launchctl")
                .args(["kickstart", "-k", &target])
                .output()
            {
                Ok(Output { status, stderr, .. }) => {
                    if status.success() {
                        Ok("Service started successfully".to_string())
                    } else {
                        Err(String::from_utf8_lossy(&stderr).into_owned())
                    }
                }
                Err(e) => Err(format!("Failed to execute command: {}", e)),
            };
        }
        LaunchdServiceCommand::Stop => {
            let target = format!("{}/{}", domain, label);
            return match Command::new("launchctl")
                .args(["kill", "SIGTERM", &target])
                .output()
            {
                Ok(Output { status, stderr, .. }) => {
                    if status.success() {
                        Ok("Service stopped successfully".to_string())
                    } else {
                        Err(String::from_utf8_lossy(&stderr).into_owned())
                    }
                }
                Err(e) => Err(format!("Failed to execute command: {}", e)),
            };
        }
    };

    match Command::new("launchctl").args(&cmd_args).output() {
        Ok(Output { status, stderr, .. }) => {
            if status.success() {
                Ok(format!("Service {} successfully", description))
            } else {
                Err(String::from_utf8_lossy(&stderr).into_owned())
            }
        }
        Err(e) => Err(format!("Failed to execute command: {}", e)),
    }
}
