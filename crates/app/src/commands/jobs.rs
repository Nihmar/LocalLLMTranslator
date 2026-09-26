//! `job_list` command.

use serde::Deserialize;
use tauri::State;

use crate::db::models::Job;
use crate::error::Result;
use crate::scheduler::queue;
use crate::AppState;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct JobListRequest {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[tauri::command]
pub async fn job_list(state: State<'_, AppState>, req: JobListRequest) -> Result<Vec<Job>> {
    queue::list_jobs(
        &state.pool,
        req.project_id.as_deref(),
        req.state.as_deref(),
        req.limit.unwrap_or(200).clamp(1, 1000),
    )
    .await
}
