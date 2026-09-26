//! `metrics_get` command: resources + queue depth.

use serde::Serialize;
use tauri::State;

use crate::error::Result;
use crate::resources::{vram, ParallelReason, VramInfo};
use crate::scheduler::queue;
use crate::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct JobCount {
    pub state: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    pub vram: Option<VramInfo>,
    pub free_bytes: Option<u64>,
    pub suggested_parallel: usize,
    pub reason: ParallelReason,
    pub jobs: Vec<JobCount>,
    pub sidecar_in_flight: usize,
    pub worker_running: bool,
    pub worker_paused: bool,
}

#[tauri::command]
pub async fn metrics_get(state: State<'_, AppState>) -> Result<Metrics> {
    // VRAM probing shells out; keep it off the async worker thread.
    let detected = tokio::task::spawn_blocking(vram::detect)
        .await
        .unwrap_or(None);
    let snapshot = state.resources.snapshot(detected, None);

    let jobs = queue::count_by_state(&state.pool)
        .await?
        .into_iter()
        .map(|(state, count)| JobCount { state, count })
        .collect();

    Ok(Metrics {
        vram: snapshot.vram,
        free_bytes: snapshot.free_bytes,
        suggested_parallel: snapshot.suggested_parallel,
        reason: snapshot.reason,
        jobs,
        sidecar_in_flight: state.supervisor.in_flight(),
        worker_running: state.worker.is_running(),
        worker_paused: state.worker.is_paused(),
    })
}
