//! `export_build` command.

use tauri::{AppHandle, State};

use super::{emit, pipeline_deps, EVENT_EXPORT_PROGRESS};
use crate::error::Result;
use crate::pipeline::export::{self, ExportOutcome, ExportRequest};
use crate::AppState;

#[tauri::command]
pub async fn export_build(
    state: State<'_, AppState>,
    app: AppHandle,
    req: ExportRequest,
) -> Result<ExportOutcome> {
    emit(
        &app,
        EVENT_EXPORT_PROGRESS,
        serde_json::json!({ "state": "started", "format": req.output_format }),
    );
    let deps = pipeline_deps(&state);
    let outcome = export::run_export(&deps, &req).await?;
    emit(
        &app,
        EVENT_EXPORT_PROGRESS,
        serde_json::json!({
            "state": "done",
            "output_path": outcome.output_path,
            "units": outcome.units,
        }),
    );
    Ok(outcome)
}
