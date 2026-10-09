//! Tauri IPC surface: one module per command group, every command named in
//! PLAN.md §12.2.

pub mod chunks;
pub mod endpoint;
pub mod export;
pub mod glossary;
pub mod ingest;
pub mod jobs;
pub mod metrics;
pub mod misc;
pub mod project;
pub mod recon;
pub mod review;
pub mod role_binding;
pub mod series;
pub mod sidecar;
pub mod translation;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::models::Job;
use crate::error::{AppError, Result};
use crate::events::emit_event;
use crate::pipeline::PipelineDeps;
use crate::scheduler::NewJob;
use crate::AppState;

// Event names (PLAN.md §12.2, frozen). Defined once in `crate::events`; re-exported
// here so command modules keep importing them from this namespace.
pub use crate::events::{
    EVENT_EXPORT_PROGRESS, EVENT_JOB_PROGRESS, EVENT_LOG_LINE, EVENT_METRICS_TICK,
    EVENT_SIDECAR_PROGRESS, EVENT_SIDECAR_STATUS,
};

/// Emit a UI event, ignoring serialization failures (they are logged).
pub fn emit<T: Serialize>(state: &AppState, event: &str, payload: T) {
    emit_event(&*state.emitter, event, payload);
}

/// Enqueue a job and announce the `pending` transition on `job://progress`.
///
/// The emitted payload is the persisted [`Job`] row, identical to the shape
/// `job_list` returns.
pub async fn enqueue_and_emit(state: &AppState, job: &NewJob) -> Result<Job> {
    let id = crate::scheduler::queue::enqueue(&state.pool, job).await?;
    let stored = crate::scheduler::queue::get_job(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("job {id}")))?;
    crate::events::emit_job(&*state.emitter, &stored);
    Ok(stored)
}

/// Enqueue a job only when no equivalent one is already unfinished. Returns whether a new
/// job was created and announces its `pending` transition. The check and the insert are one
/// statement, so a double click or two overlapping calls cannot enqueue a twin.
pub async fn enqueue_if_absent_and_emit(state: &AppState, job: &NewJob) -> Result<bool> {
    let Some(id) = crate::scheduler::queue::enqueue_if_absent(&state.pool, job).await? else {
        return Ok(false);
    };
    if let Some(stored) = crate::scheduler::queue::get_job(&state.pool, &id).await? {
        crate::events::emit_job(&*state.emitter, &stored);
    }
    Ok(true)
}

/// Enqueue a job unless an equivalent one is unfinished, returning the stored row either
/// way so a command can report a job id without creating a duplicate.
pub async fn enqueue_once_and_emit(state: &AppState, job: &NewJob) -> Result<Job> {
    if let Some(id) = crate::scheduler::queue::enqueue_if_absent(&state.pool, job).await? {
        let stored = crate::scheduler::queue::get_job(&state.pool, &id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("job {id}")))?;
        crate::events::emit_job(&*state.emitter, &stored);
        return Ok(stored);
    }
    match crate::scheduler::queue::find_active(
        &state.pool,
        &job.project_id,
        &job.kind,
        &job.payload,
    )
    .await?
    {
        Some(existing) => Ok(existing),
        // The twin settled between the insert attempt and the lookup: enqueue for real.
        None => enqueue_and_emit(state, job).await,
    }
}

/// Build the pipeline dependencies from the shared state.
pub fn pipeline_deps(state: &AppState) -> PipelineDeps {
    PipelineDeps::new(
        state.pool.clone(),
        state.sidecar.clone(),
        state.resources.clone(),
        state.data_dir.clone(),
    )
}

/// Request for `project_create`.
#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct CreateProjectRequest {
    pub name: String,
    pub source_path: String,
    pub target_lang: String,
    #[serde(default)]
    pub source_lang: Option<String>,
    #[serde(default)]
    pub source_format: Option<String>,
    #[serde(default)]
    pub doc_title: Option<String>,
    #[serde(default)]
    pub doc_author: Option<String>,
    #[serde(default)]
    pub settings: Option<Value>,
    /// Series the book belongs to when it is created inside a saga (PLAN.md §9.5).
    #[serde(default)]
    pub series_id: Option<String>,
    #[serde(default)]
    pub series_order: Option<i64>,
}

/// Generic ok/err acknowledgement for commands with no interesting payload.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Ack {
    pub ok: bool,
}

impl Ack {
    pub fn done() -> Self {
        Self { ok: true }
    }
}

/// Extract a string field from a JSON payload.
pub fn payload_str(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}
