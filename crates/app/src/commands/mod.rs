//! Tauri IPC surface: one module per command group, every command named in
//! AGENTS.md section "UI -> Tauri".

pub mod chunks;
pub mod endpoint;
pub mod export;
pub mod ingest;
pub mod jobs;
pub mod metrics;
pub mod misc;
pub mod project;
pub mod role_binding;
pub mod sidecar;
pub mod translation;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::pipeline::PipelineDeps;
use crate::AppState;

// Event names (AGENTS.md, frozen).
pub const EVENT_JOB_PROGRESS: &str = "job://progress";
pub const EVENT_LOG_LINE: &str = "log://line";
pub const EVENT_METRICS_TICK: &str = "metrics://tick";
pub const EVENT_SIDECAR_STATUS: &str = "sidecar://status";
pub const EVENT_EXPORT_PROGRESS: &str = "export://progress";

/// Emit a UI event, ignoring failures (there may be no window listening).
pub fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if let Err(error) = app.emit(event, payload) {
        tracing::debug!(%error, event, "failed to emit UI event");
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
#[derive(Debug, Clone, Deserialize)]
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
}

/// Generic ok/err acknowledgement for commands with no interesting payload.
#[derive(Debug, Clone, Serialize)]
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
