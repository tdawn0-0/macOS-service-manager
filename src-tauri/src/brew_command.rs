use std::path::Path;
use std::process::{Command, Output};
use specta::Type;
use serde::{Serialize, Deserialize};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

#[tauri::command]
#[specta::specta]
pub async  fn get_brew_services() -> Result<String, String> {
    match Command::new("brew")
        .args(["services", "list", "--json"])
        .output()
    {
        Ok(Output { stdout, stderr, status }) => {
            if !status.success() {
                return Err(String::from_utf8_lossy(&stderr).into_owned());
            }

            if !stderr.is_empty() {
                return Err(String::from_utf8_lossy(&stderr).into_owned());
            }

            String::from_utf8(stdout)
                .map_err(|e| format!("Invalid UTF-8 in stdout: {}", e))
        }
        Err(e) => Err(format!("Failed to execute command: {}", e)),
    }
}

#[derive(Serialize, Deserialize, Type)]
pub enum BrewServiceCommand {
    Start,
    Stop,
    Restart,
    Run,
    Kill,
}

#[tauri::command]
#[specta::specta]
pub async fn manage_brew_service(service_name: &str, command: BrewServiceCommand) -> Result<String, String> {
    let command_str = match command {
        BrewServiceCommand::Start => "start",
        BrewServiceCommand::Stop => "stop",
        BrewServiceCommand::Restart => "restart",
        BrewServiceCommand::Run => "run",
        BrewServiceCommand::Kill => "kill",
    };

    match Command::new("brew")
        .args(["services", command_str, service_name])
        .output()
    {
        Ok(Output { stdout, stderr, status }) => {
            if !status.success() {
                return Err(String::from_utf8_lossy(&stderr).into_owned());
            }

            // Return stdout if successful, even if empty
            String::from_utf8(stdout)
                .map_err(|e| format!("Invalid UTF-8 in stdout: {}", e))
        }
        Err(e) => Err(format!("Failed to execute command: {}", e)),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn get_brew_service_info(formula: &str) -> Result<String, String> {
    match Command::new("brew")
        .args(["services", "info", formula, "--json"])
        .output()
    {
        Ok(Output { stdout, stderr, status }) => {
            if !status.success() {
                return Err(String::from_utf8_lossy(&stderr).into_owned());
            }

            String::from_utf8(stdout)
                .map_err(|e| format!("Invalid UTF-8 in stdout: {}", e))
        }
        Err(e) => Err(format!("Failed to execute command: {}", e)),
    }
}
