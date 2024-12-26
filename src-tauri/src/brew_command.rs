use std::process::{Command, Output};

#[tauri::command]
#[specta::specta]
pub fn get_brew_services() -> Result<String, String> {
    if !check_brew_exists() {
        return Err("Homebrew is not installed".into());
    }

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

fn check_brew_exists() -> bool {
    Command::new("which")
        .arg("brew")
        .output()
        .map_or(false, |output| output.status.success())
}
