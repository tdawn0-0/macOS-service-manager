use std::process::Command;
use std::path::{PathBuf};

#[tauri::command]
#[specta::specta]
pub async  fn open_log_in_console(log_path: String) -> Result<(), String> {
    let path = PathBuf::from(log_path);

    // Verify the file exists
    if !path.exists() {
        return Err(format!("Log file does not exist: {}", path.display()));
    }

    // Verify it's a file
    if !path.is_file() {
        return Err(format!("Path is not a file: {}", path.display()));
    }

    // Get the absolute path
    let absolute_path = path.canonicalize()
        .map_err(|e| format!("Failed to get absolute path: {}", e))?;

    // Use 'open' command to launch Console.app with the log file
    let result = Command::new("open")
        .arg("-a")
        .arg("Console")
        .arg(absolute_path)
        .output()
        .map_err(|e| format!("Failed to execute command: {}", e))?;

    if !result.status.success() {
        let error = String::from_utf8_lossy(&result.stderr);
        return Err(format!("Failed to open log file: {}", error));
    }

    Ok(())
}
