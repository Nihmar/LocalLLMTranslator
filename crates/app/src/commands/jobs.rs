//! `job_list` and `job_cancel`.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::repo;
use crate::error::Result;
use crate::scheduler::queue;
use crate::views::JobView;
use crate::AppState;

#[derive(Debug, Clone, Default, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct JobListRequest {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[tauri::command(rename = "job_list", rename_all = "snake_case")]
pub async fn job_list_command(
    state: State<'_, AppState>,
    req: JobListRequest,
) -> Result<Vec<JobView>> {
    job_list(&state, req).await
}

pub async fn job_list(state: &AppState, req: JobListRequest) -> Result<Vec<JobView>> {
    let jobs = queue::list_jobs(
        &state.pool,
        req.project_id.as_deref(),
        req.state.as_deref(),
        req.limit.unwrap_or(200).clamp(1, 1000),
    )
    .await?;
    Ok(jobs.into_iter().map(JobView::from).collect())
}

/// Request for `job_cancel`.
///
/// A list rather than a single id: the monitor sends one id for "interrupt this", the ids the
/// user selected for "interrupt these", and every in-flight id it can see for "interrupt all".
#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct JobCancelRequest {
    pub job_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct JobCancelResult {
    /// Ids that were unfinished and are now `cancelled`.
    pub cancelled: Vec<String>,
    /// Ids that had already finished (or never existed): nothing to interrupt.
    pub skipped: Vec<String>,
}

/// Interrupt jobs without stopping the queue.
///
/// A job the pool is executing has its in-flight call abandoned (the worker notices the flag and
/// drops the future); a queued one is cancelled in the database. Unlike `translation_cancel` this
/// does **not** pause the pool or abort the workers: the remaining jobs keep running, which is
/// the whole point of interrupting a single chunk.
///
/// An interrupted `translate_chunk` returns its chunk to `pending`, so a later `translation_start`
/// picks it up again instead of leaving it `running` forever.
#[tauri::command(rename = "job_cancel", rename_all = "snake_case")]
pub async fn job_cancel_command(
    state: State<'_, AppState>,
    req: JobCancelRequest,
) -> Result<JobCancelResult> {
    job_cancel(&state, req).await
}

pub async fn job_cancel(state: &AppState, req: JobCancelRequest) -> Result<JobCancelResult> {
    let mut cancelled = Vec::new();
    let mut skipped = Vec::new();

    for id in &req.job_ids {
        // Read the row before touching the state: the kind and the payload decide whether a chunk
        // has to be reset, and the row is what the UI gets back on `job://progress`.
        let job = queue::get_job(&state.pool, id).await?;
        // Flag a running job first, so the worker stops the call as early as possible; the row
        // write below is what tells whether there was anything to interrupt.
        state.worker.cancel_job(id);
        if !queue::cancel(&state.pool, id).await? {
            skipped.push(id.clone());
            continue;
        }

        if let Some(job) = &job {
            if job.kind == "translate_chunk" {
                if let Some(chunk_id) = payload_chunk_id(&job.payload_json) {
                    repo::reset_chunk_to_pending(&state.pool, &chunk_id).await?;
                }
            }
            if let Some(row) = queue::get_job(&state.pool, id).await? {
                crate::events::emit_job(&*state.emitter, &row);
            }
        }
        cancelled.push(id.clone());
    }

    Ok(JobCancelResult { cancelled, skipped })
}

/// The chunk a job works on, read from its `payload_json`.
///
/// A malformed or chunkless payload is not an error: most job kinds have no chunk, and the
/// cancellation itself does not depend on it.
fn payload_chunk_id(payload_json: &str) -> Option<String> {
    let payload: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    super::payload_str(&payload, "chunk_id")
}

#[cfg(test)]
mod tests {
    use super::payload_chunk_id;

    #[test]
    fn payload_chunk_id_reads_the_chunk_of_a_translation_job() {
        assert_eq!(
            payload_chunk_id(r#"{"chunk_id":"c1"}"#),
            Some("c1".to_string())
        );
    }

    #[test]
    fn payload_chunk_id_tolerates_chunkless_and_malformed_payloads() {
        assert_eq!(payload_chunk_id("{}"), None);
        assert_eq!(payload_chunk_id("null"), None);
        assert_eq!(payload_chunk_id("not json"), None);
        assert_eq!(payload_chunk_id(r#"{"chunk_id":""}"#), None);
        assert_eq!(payload_chunk_id(r#"{"chunk_id":7}"#), None);
    }
}
