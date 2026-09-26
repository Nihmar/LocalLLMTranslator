//! LocalLLMTranslator control plane (Tauri 2).
//!
//! Rust owns state, concurrency and orchestration; the Python sidecar owns file
//! formats and pure text transforms (see PLAN.md section 2).

pub mod commands;
pub mod context;
pub mod db;
pub mod error;
pub mod llm;
pub mod pandoc;
pub mod pipeline;
pub mod resources;
pub mod scheduler;
pub mod sidecar;
pub mod util;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::SqlitePool;
use tauri::{Emitter, Manager};
use tokio::sync::broadcast;

use crate::commands::{emit, EVENT_JOB_PROGRESS, EVENT_METRICS_TICK, EVENT_SIDECAR_STATUS};
use crate::db::models::Job;
use crate::error::{AppError, Result};
use crate::pipeline::PipelineDeps;
use crate::resources::ResourceGovernor;
use crate::scheduler::{JobDispatcher, WorkerPool};
use crate::sidecar::{resolve_spawn_spec, EventSink, SidecarClient, Supervisor};

/// Shared application state, managed by Tauri and injected into commands.
pub struct AppState {
    pub app: tauri::AppHandle,
    pub pool: SqlitePool,
    pub sidecar: SidecarClient,
    pub supervisor: Arc<Supervisor>,
    pub worker: Arc<WorkerPool>,
    pub resources: ResourceGovernor,
    pub data_dir: PathBuf,
}

/// Dispatches claimed jobs to the pipeline stages.
pub struct PipelineDispatcher {
    deps: PipelineDeps,
    app: tauri::AppHandle,
}

#[async_trait]
impl JobDispatcher for PipelineDispatcher {
    async fn dispatch(&self, job: &Job) -> Result<()> {
        let payload: Value = serde_json::from_str(&job.payload_json).unwrap_or(Value::Null);
        match job.kind.as_str() {
            "ingest" => {
                let source_path = commands::payload_str(&payload, "source_path")
                    .ok_or_else(|| AppError::Invalid("ingest job is missing source_path".into()))?;
                let pdf_backend = payload.get("pdf_backend").and_then(Value::as_str);
                let outcome = crate::pipeline::ingest::run_ingest(
                    &self.deps,
                    &job.project_id,
                    &source_path,
                    pdf_backend,
                    None,
                )
                .await?;
                emit(
                    &self.app,
                    EVENT_JOB_PROGRESS,
                    serde_json::json!({
                        "job_id": job.id,
                        "kind": job.kind,
                        "state": "done",
                        "chunks": outcome.chunks,
                        "blocks": outcome.blocks,
                    }),
                );
                Ok(())
            }
            "translate_chunk" => {
                let chunk_id = commands::payload_str(&payload, "chunk_id").ok_or_else(|| {
                    AppError::Invalid("translate_chunk job is missing chunk_id".into())
                })?;
                let outcome = crate::pipeline::translate::run_translate_chunk(
                    &self.deps,
                    Some(&job.id),
                    &chunk_id,
                )
                .await?;
                emit(
                    &self.app,
                    EVENT_JOB_PROGRESS,
                    serde_json::json!({
                        "job_id": job.id,
                        "kind": job.kind,
                        "chunk_id": outcome.chunk_id,
                        "state": outcome.status,
                        "from_cache": outcome.from_cache,
                    }),
                );
                Ok(())
            }
            "export_unit" => {
                let request: crate::pipeline::export::ExportRequest =
                    serde_json::from_value(payload)?;
                let outcome = crate::pipeline::export::run_export(&self.deps, &request).await?;
                emit(
                    &self.app,
                    EVENT_JOB_PROGRESS,
                    serde_json::json!({
                        "job_id": job.id,
                        "kind": job.kind,
                        "state": "done",
                        "output_path": outcome.output_path,
                    }),
                );
                Ok(())
            }
            other => Err(AppError::Invalid(format!(
                "job kind '{other}' is not implemented in this build"
            ))),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = tauri::async_runtime::block_on(build_state(&handle))?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::project::project_list,
            commands::project::project_create,
            commands::project::project_get,
            commands::project::project_delete,
            commands::endpoint::endpoint_list,
            commands::endpoint::endpoint_upsert,
            commands::endpoint::endpoint_delete,
            commands::endpoint::endpoint_test,
            commands::endpoint::endpoint_models,
            commands::role_binding::role_binding_list,
            commands::role_binding::role_binding_set,
            commands::ingest::ingest_start,
            commands::translation::translation_start,
            commands::translation::translation_pause,
            commands::translation::translation_cancel,
            commands::jobs::job_list,
            commands::chunks::chunk_list,
            commands::chunks::chunk_get,
            commands::metrics::metrics_get,
            commands::sidecar::sidecar_status,
            commands::export::export_build,
            commands::misc::open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running LocalLLMTranslator");
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

async fn build_state(app: &tauri::AppHandle) -> Result<AppState> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(anyhow::anyhow!("no app data dir: {error}")))?;
    tokio::fs::create_dir_all(&data_dir).await?;

    let pool = db::connect(&data_dir.join("app.sqlite")).await?;

    // --- Sidecar ---------------------------------------------------------
    let bundled = crate::sidecar::bundled_sidecar_path(app.path().resource_dir().ok().as_ref());
    let spec = resolve_spawn_spec(bundled.as_deref());

    let sink: EventSink = {
        let handle = app.clone();
        Arc::new(move |method: &str, params: &Value| {
            if method == "progress" {
                let _ = handle.emit(EVENT_JOB_PROGRESS, params.clone());
            }
            let _ = handle.emit(&format!("sidecar://{method}"), params.clone());
        })
    };
    let supervisor = Supervisor::new(spec, sink, None);
    let sidecar = SidecarClient::new(supervisor.clone());

    // --- Resources -------------------------------------------------------
    let resources = ResourceGovernor::default();
    let vram = tokio::task::spawn_blocking(crate::resources::vram::detect)
        .await
        .unwrap_or(None);
    let snapshot = resources.snapshot(vram, None);

    // --- Worker pool -----------------------------------------------------
    let deps = PipelineDeps::new(
        pool.clone(),
        sidecar.clone(),
        resources.clone(),
        data_dir.clone(),
    );
    let dispatcher = Arc::new(PipelineDispatcher {
        deps,
        app: app.clone(),
    });
    let worker = Arc::new(WorkerPool::new(
        pool.clone(),
        dispatcher,
        snapshot.suggested_parallel,
        4.max(snapshot.suggested_parallel),
    ));

    spawn_status_forwarder(app.clone(), supervisor.clone());
    spawn_metrics_ticker(
        app.clone(),
        pool.clone(),
        resources.clone(),
        worker.clone(),
        supervisor.clone(),
    );

    Ok(AppState {
        app: app.clone(),
        pool,
        sidecar,
        supervisor,
        worker,
        resources,
        data_dir,
    })
}

/// Forward sidecar status changes to the `sidecar://status` event.
fn spawn_status_forwarder(app: tauri::AppHandle, supervisor: Arc<Supervisor>) {
    let mut receiver = supervisor.subscribe_status();
    tauri::async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(status) => emit(&app, EVENT_SIDECAR_STATUS, status),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Periodically emit `metrics://tick` with queue depth and resource state.
fn spawn_metrics_ticker(
    app: tauri::AppHandle,
    pool: SqlitePool,
    resources: ResourceGovernor,
    worker: Arc<WorkerPool>,
    supervisor: Arc<Supervisor>,
) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3));
        loop {
            ticker.tick().await;
            let detected = tokio::task::spawn_blocking(crate::resources::vram::detect)
                .await
                .unwrap_or(None);
            let snapshot = resources.snapshot(detected, None);
            let jobs = crate::scheduler::queue::count_by_state(&pool)
                .await
                .unwrap_or_default();
            emit(
                &app,
                EVENT_METRICS_TICK,
                serde_json::json!({
                    "free_bytes": snapshot.free_bytes,
                    "suggested_parallel": snapshot.suggested_parallel,
                    "reason": snapshot.reason,
                    "jobs": jobs,
                    "worker_running": worker.is_running(),
                    "worker_paused": worker.is_paused(),
                    "sidecar": supervisor.status(),
                }),
            );
        }
    });
}
