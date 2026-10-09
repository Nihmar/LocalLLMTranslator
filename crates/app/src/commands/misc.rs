//! Diagnostics commands: the OS handler, the UI error sink and the support bundle.

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_shell::ShellExt;

use super::Ack;
use crate::diagnostics::{self, DiagnosticsOutcome, DiagnosticsReport, QueueCount};
use crate::error::{AppError, Result};
use crate::scheduler::queue;
use crate::util::clamp_chars;
use crate::AppState;

/// Open a path with the platform's default application. `Shell::command` is used
/// (rather than the deprecated `Shell::open`) so the launcher is explicit.
#[tauri::command(rename = "open_path", rename_all = "snake_case")]
pub async fn open_path_command(app: AppHandle, path: String) -> Result<Ack> {
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

/// Record a UI-visible failure. The frontend calls this for every rejected `invoke`, so the
/// log file contains the errors the user actually saw, with the command name.
#[tauri::command(rename = "log_frontend_error", rename_all = "snake_case")]
pub async fn log_frontend_error_command(command: String, message: String) -> Result<Ack> {
    log_frontend_error(command, message).await
}

pub async fn log_frontend_error(command: String, message: String) -> Result<Ack> {
    // A runaway message must not fill the log; the frontend already truncates for display.
    let command = clamp_chars(command.trim(), 120);
    let message = clamp_chars(message.trim(), 4000);
    tracing::error!(target: "ui", command = %command, "{message}");
    Ok(Ack::done())
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsPaths {
    /// The application data directory (database, projects, logs).
    pub data_dir: String,
    /// Where the daily log files live.
    pub log_dir: String,
}

/// Where the application keeps its data and its logs, so the UI can reveal the folder.
#[tauri::command(rename = "diagnostics_paths", rename_all = "snake_case")]
pub async fn diagnostics_paths_command(state: State<'_, AppState>) -> Result<DiagnosticsPaths> {
    diagnostics_paths(&state).await
}

pub async fn diagnostics_paths(state: &AppState) -> Result<DiagnosticsPaths> {
    Ok(DiagnosticsPaths {
        data_dir: state.data_dir.to_string_lossy().to_string(),
        log_dir: crate::logging::log_dir(&state.data_dir)
            .to_string_lossy()
            .to_string(),
    })
}

/// Write a diagnostics bundle (newest logs + a report) and return where it landed. The
/// archive carries no book text, prompt, response or database.
#[tauri::command(rename = "diagnostics_export", rename_all = "snake_case")]
pub async fn diagnostics_export_command(state: State<'_, AppState>) -> Result<DiagnosticsOutcome> {
    diagnostics_export(&state).await
}

pub async fn diagnostics_export(state: &AppState) -> Result<DiagnosticsOutcome> {
    let queue_counts = queue::count_by_state(&state.pool)
        .await?
        .into_iter()
        .map(|(state, count)| QueueCount { state, count })
        .collect();
    let report: DiagnosticsReport = diagnostics::build_report(
        &state.pool,
        &state.data_dir,
        state.supervisor.status(),
        state.worker.is_running(),
        state.worker.is_paused(),
        queue_counts,
        state.worker.endpoint_usage(),
    )
    .await?;

    let data_dir = state.data_dir.clone();
    tokio::task::spawn_blocking(move || diagnostics::write_bundle(&data_dir, &report))
        .await
        .map_err(|error| AppError::Other(anyhow::anyhow!("diagnostics task panicked: {error}")))?
}
