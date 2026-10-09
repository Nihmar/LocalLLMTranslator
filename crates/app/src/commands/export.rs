//! `export_build`, `export_preview` and `export_history` commands.

use tauri::{AppHandle, State};

use super::{emit, pipeline_deps, EVENT_EXPORT_PROGRESS};
use crate::error::Result;
use crate::pipeline::export::{
    self, ExportBuildRecord, ExportOutcome, ExportPreview, ExportPreviewRequest, ExportRequest,
};
use crate::AppState;

#[tauri::command(rename_all = "snake_case")]
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
            "from_cache": outcome.from_cache,
        }),
    );
    Ok(outcome)
}

/// The composed units and the `metadata.yaml` a build would use, without
/// invoking Pandoc.
#[tauri::command(rename_all = "snake_case")]
pub async fn export_preview(
    state: State<'_, AppState>,
    req: ExportPreviewRequest,
) -> Result<ExportPreview> {
    let deps = pipeline_deps(&state);
    export::run_export_preview(&deps, &req).await
}

/// The recent build records, newest first.
#[tauri::command(rename_all = "snake_case")]
pub async fn export_history(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<ExportBuildRecord>> {
    export::export_history(&state.pool, &project_id).await
}
