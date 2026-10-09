//! `metrics_get` command: resources + queue depth.

use serde::Serialize;
use tauri::State;

use crate::error::Result;
use crate::resources::endpoints::EndpointUsage;
use crate::resources::{vram, ParallelReason, VramInfo};
use crate::scheduler::queue;
use crate::sidecar::SidecarStatus;
use crate::AppState;

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct JobCount {
    pub state: String,
    pub count: i64,
}

/// Build the queue-depth histogram from `(state, count)` rows.
///
/// Shared by `metrics_get` and the `metrics://tick` ticker so both emit the same
/// `{state, count}` objects the UI's `JobCount` type declares. Emitting the raw
/// pairs instead produced `[["done", 3], …]`, which the UI cannot parse.
pub fn job_counts(rows: Vec<(String, i64)>) -> Vec<JobCount> {
    rows.into_iter()
        .map(|(state, count)| JobCount { state, count })
        .collect()
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Metrics {
    pub vram: Option<VramInfo>,
    pub free_bytes: Option<u64>,
    pub suggested_parallel: usize,
    pub reason: ParallelReason,
    pub jobs: Vec<JobCount>,
    /// Per-role endpoint capacity and in-flight counts, so the UI can show why
    /// the sub-agents are capped (PLAN.md §10).
    pub endpoints: Vec<EndpointUsage>,
    pub sidecar_in_flight: usize,
    pub worker_running: bool,
    pub worker_paused: bool,
}

/// Payload of `metrics://tick`: the periodic snapshot, without a fresh VRAM probe
/// and with the sidecar status the desktop banner reads (`PLAN.md` §12.2).
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct MetricsTick {
    pub free_bytes: Option<u64>,
    pub suggested_parallel: usize,
    pub reason: ParallelReason,
    pub jobs: Vec<JobCount>,
    pub endpoints: Vec<EndpointUsage>,
    pub worker_running: bool,
    pub worker_paused: bool,
    pub sidecar: SidecarStatus,
}

#[tauri::command(rename = "metrics_get", rename_all = "snake_case")]
pub async fn metrics_get_command(state: State<'_, AppState>) -> Result<Metrics> {
    metrics_get(&state).await
}

pub async fn metrics_get(state: &AppState) -> Result<Metrics> {
    // VRAM probing shells out; keep it off the async worker thread.
    let detected = tokio::task::spawn_blocking(vram::detect)
        .await
        .unwrap_or(None);
    let snapshot = state.resources.snapshot(
        detected,
        Some(state.worker.endpoint_slots()).filter(|slots| *slots > 0),
    );

    let jobs = job_counts(queue::count_by_state(&state.pool).await?);

    Ok(Metrics {
        vram: snapshot.vram,
        free_bytes: snapshot.free_bytes,
        suggested_parallel: snapshot.suggested_parallel,
        reason: snapshot.reason,
        jobs,
        endpoints: state.worker.endpoint_usage(),
        sidecar_in_flight: state.supervisor.in_flight(),
        worker_running: state.worker.is_running(),
        worker_paused: state.worker.is_paused(),
    })
}

#[cfg(test)]
mod tests {
    use super::job_counts;

    #[test]
    fn job_counts_serialize_as_state_and_count_objects() {
        // The same predicate the UI's `JobCount` type expects, shared by
        // `metrics_get` and the `metrics://tick` ticker.
        let rows = vec![("done".to_string(), 3i64), ("pending".to_string(), 1)];
        let value = serde_json::to_value(job_counts(rows)).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!([
                { "state": "done", "count": 3 },
                { "state": "pending", "count": 1 }
            ])
        );
    }
}
