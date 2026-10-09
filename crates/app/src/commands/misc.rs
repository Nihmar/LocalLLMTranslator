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
///
/// Windows does **not** go through `cmd /C start`: `std::process::Command` only quotes
/// arguments containing whitespace or quotes, so a path containing `&`, `|`, `^` or `%`
/// would be re-parsed by `cmd.exe` as shell syntax. `explorer` receives the path as a
/// normal argument and is not a shell.
#[tauri::command(rename = "open_path", rename_all = "snake_case")]
pub async fn open_path_command(app: AppHandle, path: String) -> Result<Ack> {
    let (program, args) = opener(path);

    app.shell()
        .command(program)
        .args(args)
        .spawn()
        .map_err(|error| AppError::Other(anyhow::anyhow!("failed to open path: {error}")))?;
    Ok(Ack::done())
}

/// The program and arguments that open `path` with the OS default handler.
///
/// Kept pure so the platform choice is testable without an `AppHandle`. On Windows the path
/// is handed to `explorer` as a single argument, never to `cmd`, because a path containing
/// shell metacharacters would otherwise be re-interpreted by `cmd.exe`.
fn opener(path: String) -> (&'static str, Vec<String>) {
    if cfg!(target_os = "macos") {
        ("open", vec![path])
    } else if cfg!(target_os = "windows") {
        ("explorer", vec![path])
    } else {
        ("xdg-open", vec![path])
    }
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

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
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

#[cfg(test)]
mod tests {
    use super::opener;

    #[test]
    fn the_opener_never_uses_a_shell() {
        let (program, args) = opener("C:\\Books\\A & B.pdf".to_string());
        #[cfg(target_os = "macos")]
        assert_eq!(program, "open");
        #[cfg(windows)]
        assert_eq!(program, "explorer");
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(program, "xdg-open");
        // The path travels as one argument, so no character of it is re-parsed by a shell.
        assert_eq!(args, vec!["C:\\Books\\A & B.pdf".to_string()]);
    }
}
