//! LocalLLMTranslator control plane (Tauri 2).
//!
//! Rust owns state, concurrency and orchestration; the Python sidecar owns file
//! formats and pure text transforms (see PLAN.md section 2).

pub mod commands;
pub mod context;
pub mod db;
pub mod error;
pub mod events;
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
use tauri::Manager;
use tokio::sync::broadcast;

use crate::commands::{emit, EVENT_METRICS_TICK, EVENT_SIDECAR_STATUS};
use crate::db::models::Job;
use crate::error::{AppError, Result};
use crate::events::{sidecar_event_sink, EventEmitter};
use crate::pipeline::PipelineDeps;
use crate::resources::ResourceGovernor;
use crate::scheduler::{JobDispatcher, WorkerPool};
use crate::sidecar::{resolve_spawn_spec, SidecarClient, Supervisor};

/// Shared application state, managed by Tauri and injected into commands.
pub struct AppState {
    pub app: tauri::AppHandle,
    /// UI event emitter, shared by the worker pool and the commands.
    pub emitter: Arc<dyn EventEmitter>,
    pub pool: SqlitePool,
    pub sidecar: SidecarClient,
    pub supervisor: Arc<Supervisor>,
    pub worker: Arc<WorkerPool>,
    pub resources: ResourceGovernor,
    pub data_dir: PathBuf,
}

/// Dispatches claimed jobs to the pipeline stages.
///
/// It does not emit events: the worker pool owns the `job://progress` lifecycle,
/// so every producer shares one consistent payload shape.
pub struct PipelineDispatcher {
    deps: PipelineDeps,
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
                crate::pipeline::ingest::run_ingest(
                    &self.deps,
                    &job.project_id,
                    &source_path,
                    pdf_backend,
                    None,
                )
                .await?;
                Ok(())
            }
            "translate_chunk" => {
                let chunk_id = commands::payload_str(&payload, "chunk_id").ok_or_else(|| {
                    AppError::Invalid("translate_chunk job is missing chunk_id".into())
                })?;
                crate::pipeline::translate::run_translate_chunk(
                    &self.deps,
                    Some(&job.id),
                    &chunk_id,
                )
                .await?;
                Ok(())
            }
            "book_recon" => {
                let pasted_text = payload.get("pasted_text").and_then(Value::as_str);
                crate::pipeline::recon::run_recon(
                    &self.deps,
                    Some(&job.id),
                    &job.project_id,
                    pasted_text,
                )
                .await?;
                Ok(())
            }
            "export_unit" => {
                let request: crate::pipeline::export::ExportRequest =
                    serde_json::from_value(payload)?;
                crate::pipeline::export::run_export(&self.deps, &request).await?;
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

    let app = tauri::Builder::default()
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
            commands::recon::recon_start,
            commands::recon::recon_get,
            commands::recon::recon_confirm,
            commands::jobs::job_list,
            commands::chunks::chunk_list,
            commands::chunks::chunk_get,
            commands::metrics::metrics_get,
            commands::sidecar::sidecar_status,
            commands::export::export_build,
            commands::misc::open_path,
        ])
        .build(tauri::generate_context!())
        .expect("error while building LocalLLMTranslator");

    app.run(|app_handle, event| {
        // Release the worker pool and the sidecar child process on exit.
        if matches!(
            event,
            tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
        ) {
            shutdown(app_handle);
        }
    });
}

/// Stop the worker pool and shut the sidecar down deterministically.
///
/// Idempotent: both `ExitRequested` and `Exit` may fire.
fn shutdown(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        tracing::info!("shutting down: stopping worker pool and sidecar");
        state.worker.cancel();
        state.supervisor.shutdown();
    }
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

    // --- Crash recovery ---------------------------------------------------
    // Jobs left `leased`/`running` by a previous run are stale (this process
    // owns no lease), and chunks left `running` belong to interrupted jobs.
    // Return both to their claimable state so the worker pool resumes them.
    let requeued = crate::scheduler::queue::requeue_in_flight(&pool)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "could not requeue in-flight jobs at boot");
            0
        });
    let reset_chunks = crate::db::repo::reset_running_chunks(&pool, None)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "could not reset running chunks at boot");
            0
        });
    if requeued > 0 || reset_chunks > 0 {
        tracing::info!(
            requeued,
            reset_chunks,
            "recovered interrupted work from a previous run"
        );
    }

    // --- Sidecar ---------------------------------------------------------
    let bundled = crate::sidecar::bundled_sidecar_path(app.path().resource_dir().ok().as_ref());
    let spec = resolve_spawn_spec(bundled.as_deref());

    let emitter: Arc<dyn EventEmitter> = Arc::new(app.clone());
    let sink = sidecar_event_sink(emitter.clone());
    let supervisor = Supervisor::new(spec, sink, emitter.clone(), None);
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
    let dispatcher = Arc::new(PipelineDispatcher { deps });
    let worker = Arc::new(WorkerPool::new(
        pool.clone(),
        dispatcher,
        emitter.clone(),
        snapshot.suggested_parallel,
        4.max(snapshot.suggested_parallel),
    ));

    // Restore an explicit pause persisted by `translation_pause`: without this
    // the unconditional start below would silently resume LLM work the user had
    // stopped. The flag is read before starting so `start_paused` can publish it
    // before any worker loop is scheduled.
    let restored_paused = crate::db::repo::get_app_state(&pool, crate::db::repo::KEY_WORKER_PAUSED)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the persisted worker pause flag");
            None
        })
        .as_deref()
        == Some("1");

    // Start the pool at boot: this claims pending/leased jobs (resume) and runs
    // the lease reaper, so recovery does not depend on a user action. A persisted
    // pause is honoured instead of auto-resuming.
    if restored_paused {
        tracing::info!("restoring the paused worker pool from the previous session");
        worker.start_paused();
    } else {
        worker.start();
    }

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
        emitter,
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
            // Same `{state, count}` shape `metrics_get` returns (the UI's
            // `JobCount` type expects objects, not `[state, count]` pairs).
            let jobs = crate::commands::metrics::job_counts(
                crate::scheduler::queue::count_by_state(&pool)
                    .await
                    .unwrap_or_default(),
            );
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
