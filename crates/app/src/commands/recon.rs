//! `recon_start` / `recon_get` / `recon_confirm` (PLAN.md section 9.4) and
//! `project_set_dialogue_style`, the book convention that sits next to the profile.

use serde::Deserialize;
use tauri::State;

use super::enqueue_once_and_emit;
use super::ingest::JobStarted;
use crate::db::repo;
use crate::error::{AppError, Result};
use crate::pipeline::recon::{self, ConfirmRequest, ReconSnapshot};
use crate::scheduler::NewJob;
use crate::AppState;

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct ReconStartRequest {
    pub project_id: String,
    /// Optional text the user pasted themselves; the app never fetches a page.
    #[serde(default)]
    pub pasted_text: Option<String>,
}

/// Enqueue the `book_recon` job. Fails fast when the prerequisites are missing,
/// so the user gets a clear message instead of a failed job later.
#[tauri::command(rename = "recon_start", rename_all = "snake_case")]
pub async fn recon_start_command(
    state: State<'_, AppState>,
    req: ReconStartRequest,
) -> Result<JobStarted> {
    recon_start(&state, req).await
}

pub async fn recon_start(state: &AppState, req: ReconStartRequest) -> Result<JobStarted> {
    repo::get_project(&state.pool, &req.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", req.project_id)))?;
    if repo::get_document_for_project(&state.pool, &req.project_id)
        .await?
        .is_none()
    {
        return Err(AppError::Invalid(
            "the project has no ingested document: run ingestion first".into(),
        ));
    }
    if repo::role_binding_for(&state.pool, recon::ROLE)
        .await?
        .is_none()
    {
        return Err(AppError::Invalid(
            "no role_binding configured for 'orchestrator': bind a model to run the reconnaissance"
                .into(),
        ));
    }

    let payload = serde_json::json!({ "pasted_text": req.pasted_text });
    let job = NewJob::new(&req.project_id, recon::JOB_KIND, payload).with_priority(10);
    let job = enqueue_once_and_emit(state, &job).await?;
    state.worker.start();
    Ok(JobStarted { job_id: job.id })
}

/// Candidate profile, confirmed values and glossary for a project.
#[tauri::command(rename = "recon_get", rename_all = "snake_case")]
pub async fn recon_get_command(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ReconSnapshot> {
    recon_get(&state, project_id).await
}

pub async fn recon_get(state: &AppState, project_id: String) -> Result<ReconSnapshot> {
    recon::snapshot(&state.pool, &project_id).await
}

#[tauri::command(rename = "recon_confirm", rename_all = "snake_case")]
pub async fn recon_confirm_command(
    state: State<'_, AppState>,
    req: ConfirmRequest,
) -> Result<ReconSnapshot> {
    recon_confirm(&state, req).await
}

/// Persist the fields the user confirmed.
pub async fn recon_confirm(state: &AppState, req: ConfirmRequest) -> Result<ReconSnapshot> {
    recon::confirm(&state.pool, &req).await
}

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DialogueStyleRequest {
    pub project_id: String,
    /// `keep` or `quotes`.
    pub dialogue_style: String,
}

/// Choose how the translator renders dialogue. It applies to the chunks translated
/// from now on; already translated chunks keep their text until they are redone.
#[tauri::command(rename = "project_set_dialogue_style", rename_all = "snake_case")]
pub async fn project_set_dialogue_style_command(
    state: State<'_, AppState>,
    req: DialogueStyleRequest,
) -> Result<ReconSnapshot> {
    project_set_dialogue_style(&state, req).await
}

pub async fn project_set_dialogue_style(
    state: &AppState,
    req: DialogueStyleRequest,
) -> Result<ReconSnapshot> {
    let style = req.dialogue_style.trim();
    if !recon::DIALOGUE_STYLES.contains(&style) {
        return Err(AppError::Invalid(format!(
            "unknown dialogue style {style:?}; expected one of {:?}",
            recon::DIALOGUE_STYLES
        )));
    }
    repo::get_project(&state.pool, &req.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", req.project_id)))?;
    repo::set_memory(
        &state.pool,
        &req.project_id,
        recon::DIALOGUE_STYLE_KEY,
        style,
    )
    .await?;
    recon::snapshot(&state.pool, &req.project_id).await
}
