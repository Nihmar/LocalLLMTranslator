//! `open_path` command: hand a file to the OS default application.

use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;

use super::Ack;
use crate::error::{AppError, Result};

/// Open a path with the platform's default application. `Shell::command` is used
/// (rather than the deprecated `Shell::open`) so the launcher is explicit.
#[tauri::command]
pub async fn open_path(app: AppHandle, path: String) -> Result<Ack> {
    let (program, args): (&str, Vec<String>) = if cfg!(target_os = "macos") {
        ("open", vec![path])
    } else if cfg!(target_os = "windows") {
        (
            "cmd",
            vec!["/C".to_string(), "start".to_string(), String::new(), path],
        )
    } else {
        ("xdg-open", vec![path])
    };

    app.shell()
        .command(program)
        .args(args)
        .spawn()
        .map_err(|error| AppError::Other(anyhow::anyhow!("failed to open path: {error}")))?;
    Ok(Ack::done())
}
