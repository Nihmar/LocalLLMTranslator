//! `export_build`, `export_preview` and `export_history` commands.

use tauri::State;

use super::{emit, pipeline_deps, EVENT_EXPORT_PROGRESS};
use crate::error::Result;
use crate::pipeline::export::{
    self, ExportBuildRecord, ExportOutcome, ExportPreview, ExportPreviewRequest, ExportRequest,
};
use crate::AppState;

#[tauri::command(rename = "export_build", rename_all = "snake_case")]
pub async fn export_build_command(
    state: State<'_, AppState>,
    req: ExportRequest,
) -> Result<ExportOutcome> {
    export_build(&state, req).await
}

pub async fn export_build(state: &AppState, req: ExportRequest) -> Result<ExportOutcome> {
    emit(
        state,
        EVENT_EXPORT_PROGRESS,
        export::ExportProgress {
            state: "started".to_string(),
            format: Some(req.output_format.clone()),
            output_path: None,
            units: None,
            from_cache: None,
        },
    );
    let deps = pipeline_deps(state);
    let outcome = export::run_export(&deps, &req).await?;
    emit(
        state,
        EVENT_EXPORT_PROGRESS,
        export::ExportProgress {
            state: "done".to_string(),
            format: None,
            output_path: Some(outcome.output_path.clone()),
            units: Some(outcome.units),
            from_cache: Some(outcome.from_cache),
        },
    );
    Ok(outcome)
}

/// The composed units and the `metadata.yaml` a build would use, without
/// invoking Pandoc.
#[tauri::command(rename = "export_preview", rename_all = "snake_case")]
pub async fn export_preview_command(
    state: State<'_, AppState>,
    req: ExportPreviewRequest,
) -> Result<ExportPreview> {
    export_preview(&state, req).await
}

pub async fn export_preview(state: &AppState, req: ExportPreviewRequest) -> Result<ExportPreview> {
    let deps = pipeline_deps(state);
    export::run_export_preview(&deps, &req).await
}

/// The recent build records, newest first.
#[tauri::command(rename = "export_history", rename_all = "snake_case")]
pub async fn export_history_command(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<ExportBuildRecord>> {
    export_history(&state, project_id).await
}

pub async fn export_history(
    state: &AppState,
    project_id: String,
) -> Result<Vec<ExportBuildRecord>> {
    export::export_history(&state.pool, &project_id).await
}
