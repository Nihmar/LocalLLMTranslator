//! `ingest_start` command.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::repo;
use crate::error::{AppError, Result};
use crate::scheduler::NewJob;
use crate::AppState;

use super::enqueue_and_emit;

#[derive(Debug, Clone, Deserialize)]
pub struct IngestStartRequest {
    pub project_id: String,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub pdf_backend: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobStarted {
    pub job_id: String,
}

#[tauri::command]
pub async fn ingest_start(
    state: State<'_, AppState>,
    req: IngestStartRequest,
) -> Result<JobStarted> {
    let project = repo::get_project(&state.pool, &req.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", req.project_id)))?;

    let source_path = req.source_path.unwrap_or(project.source_path);
    let payload = serde_json::json!({
        "source_path": source_path,
        "pdf_backend": req.pdf_backend,
    });

    let job = NewJob::new(&req.project_id, "ingest", payload).with_priority(0);
    let job = enqueue_and_emit(&state, &job).await?;

    // Ensure the worker pool is running to pick the job up.
    state.worker.start();

    Ok(JobStarted { job_id: job.id })
}
