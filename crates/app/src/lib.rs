//! LocalLLMTranslator control plane (Tauri 2).
//!
//! Rust owns state, concurrency and orchestration; the Python sidecar owns file
//! formats and pure text transforms (see PLAN.md section 2).

pub mod commands;
pub mod context;
pub mod db;
pub mod diagnostics;
pub mod error;
pub mod events;
pub mod llm;
pub mod logging;
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

use crate::commands::{EVENT_METRICS_TICK, EVENT_SIDECAR_STATUS};
use crate::db::models::Job;
use crate::error::{AppError, Result};
use crate::events::{emit_event, sidecar_event_sink, EventEmitter};
use crate::pipeline::PipelineDeps;
use crate::resources::ResourceGovernor;
use crate::scheduler::{JobDispatcher, WorkerPool};
use crate::sidecar::{resolve_spawn_spec, SidecarClient, Supervisor};

/// Shared application state of the control plane.
///
/// It holds no Tauri type: the Tauri shell builds it in `run`, and a headless binary
/// (the standalone server) can build the same state from a data dir and an emitter.
pub struct AppState {
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
            "series_recon" => {
                let series_id = commands::payload_str(&payload, "series_id").ok_or_else(|| {
                    AppError::Invalid("series_recon job is missing series_id".into())
                })?;
                let force = payload
                    .get("force")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                crate::pipeline::series_recon::run_series_recon(
                    &self.deps,
                    Some(&job.id),
                    &series_id,
                    force,
                )
                .await?;
                Ok(())
            }
            "summarize" => {
                let request: crate::pipeline::summarize::SummarizePayload =
                    serde_json::from_value(payload)?;
                crate::pipeline::summarize::run_summarize(
                    &self.deps,
                    Some(&job.id),
                    &job.project_id,
                    &request,
                )
                .await?;
                Ok(())
            }
            "edit_chunk" => {
                let chunk_id = commands::payload_str(&payload, "chunk_id").ok_or_else(|| {
                    AppError::Invalid("edit_chunk job is missing chunk_id".into())
                })?;
                crate::pipeline::review::run_edit_chunk(&self.deps, Some(&job.id), &chunk_id)
                    .await?;
                Ok(())
            }
            "proofread_chunk" => {
                let chunk_id = commands::payload_str(&payload, "chunk_id").ok_or_else(|| {
                    AppError::Invalid("proofread_chunk job is missing chunk_id".into())
                })?;
                crate::pipeline::review::run_proofread_chunk(&self.deps, Some(&job.id), &chunk_id)
                    .await?;
                Ok(())
            }
            "qa_scan" => {
                let request: crate::pipeline::qa::QaScanPayload = serde_json::from_value(payload)?;
                crate::pipeline::qa::scan_chunk(&self.deps, &job.project_id, &request.chunk_id)
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
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            // The data dir is known here, so file logging starts before any state is built
            // and every boot message lands in the log a user can hand over.
            let data_dir = resolve_data_dir(&handle)?;
            logging::init(&data_dir);
            let resource_dir = handle.path().resource_dir().ok();
            let emitter: Arc<dyn EventEmitter> = Arc::new(handle.clone());
            let state =
                tauri::async_runtime::block_on(build_state(data_dir, resource_dir, emitter))?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::project::project_list_command,
            commands::project::project_create_command,
            commands::project::project_get_command,
            commands::project::project_delete_command,
            commands::project::project_export_command,
            commands::project::project_import_command,
            commands::endpoint::endpoint_list_command,
            commands::endpoint::endpoint_upsert_command,
            commands::endpoint::endpoint_delete_command,
            commands::endpoint::endpoint_test_command,
            commands::endpoint::endpoint_models_command,
            commands::role_binding::role_binding_list_command,
            commands::role_binding::role_binding_set_command,
            commands::role_binding::role_binding_delete_command,
            commands::ingest::document_inspect_command,
            commands::ingest::ingest_start_command,
            commands::translation::translation_start_command,
            commands::translation::translation_pause_command,
            commands::translation::translation_cancel_command,
            commands::recon::recon_start_command,
            commands::recon::recon_get_command,
            commands::recon::recon_confirm_command,
            commands::recon::project_set_dialogue_style_command,
            commands::glossary::glossary_list_command,
            commands::glossary::glossary_upsert_command,
            commands::glossary::glossary_delete_command,
            commands::series::series_list_command,
            commands::series::series_create_command,
            commands::series::series_get_command,
            commands::series::series_update_command,
            commands::series::series_delete_command,
            commands::series::project_set_series_command,
            commands::series::series_glossary_list_command,
            commands::series::series_glossary_upsert_command,
            commands::series::series_glossary_delete_command,
            commands::series::series_variant_upsert_command,
            commands::series::series_variant_delete_command,
            commands::series::series_promote_term_command,
            commands::series::series_export_command,
            commands::series::series_import_command,
            commands::series::series_qa_scan_command,
            commands::series::series_recon_start_command,
            commands::series::series_recon_confirm_command,
            commands::review::review_start_command,
            commands::review::suggestion_list_command,
            commands::review::suggestion_history_command,
            commands::review::suggestion_accept_command,
            commands::review::suggestion_reject_command,
            commands::review::qa_report_command,
            commands::review::qa_finding_set_status_command,
            commands::jobs::job_list_command,
            commands::jobs::job_cancel_command,
            commands::chunks::chunk_list_command,
            commands::chunks::chunk_get_command,
            commands::metrics::metrics_get_command,
            commands::sidecar::sidecar_status_command,
            commands::export::export_build_command,
            commands::export::export_preview_command,
            commands::export::export_history_command,
            commands::misc::open_path_command,
            commands::misc::log_frontend_error_command,
            commands::misc::diagnostics_paths_command,
            commands::misc::diagnostics_export_command,
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

fn resolve_data_dir(app: &tauri::AppHandle) -> Result<PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|error| AppError::Other(anyhow::anyhow!("no app data dir: {error}")))
}

async fn build_state(
    data_dir: PathBuf,
    resource_dir: Option<PathBuf>,
    emitter: Arc<dyn EventEmitter>,
) -> Result<AppState> {
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
    let bundled = crate::sidecar::bundled_sidecar_path(resource_dir.as_ref());
    let spec = resolve_spawn_spec(bundled.as_deref());

    let sink = sidecar_event_sink(emitter.clone());
    let supervisor = Supervisor::new(spec, sink, emitter.clone(), None);
    let sidecar = SidecarClient::new(supervisor.clone());

    // --- Resources -------------------------------------------------------
    let resources = ResourceGovernor::default();
    // Per-endpoint slots: a live `/props` probe per bound role (the persisted
    // props as fallback) feeds both the per-role limiter and the global cap.
    let endpoint_plan = crate::resources::endpoints::probe_endpoint_limits(&pool).await;
    // Count each endpoint once: two roles bound to the same server must not make the
    // global cap look twice as large as the hardware is.
    let endpoint_slots =
        Some(crate::resources::endpoints::plan_slots(&endpoint_plan)).filter(|slots| *slots > 0);
    let vram = tokio::task::spawn_blocking(crate::resources::vram::detect)
        .await
        .unwrap_or(None);
    let snapshot = resources.snapshot(vram, endpoint_slots);

    // --- Worker pool -----------------------------------------------------
    let deps = PipelineDeps::new(
        pool.clone(),
        sidecar.clone(),
        resources.clone(),
        data_dir.clone(),
    )
    .with_pandoc_dir(crate::pandoc::resolve_assets_dir(resource_dir.as_deref()));
    let dispatcher = Arc::new(PipelineDispatcher { deps });
    let limits = crate::resources::endpoints::EndpointLimits::from_plan(
        &endpoint_plan,
        snapshot.suggested_parallel,
    );
    let worker = Arc::new(WorkerPool::with_limits(
        pool.clone(),
        dispatcher,
        emitter.clone(),
        snapshot.suggested_parallel,
        4.max(snapshot.suggested_parallel),
        limits,
    ));
    tracing::info!(
        suggested_parallel = snapshot.suggested_parallel,
        reason = ?snapshot.reason,
        endpoints = ?worker.endpoint_usage(),
        "scheduler plan"
    );

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

    spawn_status_forwarder(emitter.clone(), supervisor.clone());
    spawn_metrics_ticker(
        emitter.clone(),
        pool.clone(),
        resources.clone(),
        worker.clone(),
        supervisor.clone(),
    );

    Ok(AppState {
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
fn spawn_status_forwarder(emitter: Arc<dyn EventEmitter>, supervisor: Arc<Supervisor>) {
    let mut receiver = supervisor.subscribe_status();
    tauri::async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(status) => emit_event(&*emitter, EVENT_SIDECAR_STATUS, status),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Periodically emit `metrics://tick` with queue depth and resource state.
fn spawn_metrics_ticker(
    emitter: Arc<dyn EventEmitter>,
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
            let snapshot = resources.snapshot(
                detected,
                Some(worker.endpoint_slots()).filter(|slots| *slots > 0),
            );
            // Same `{state, count}` shape `metrics_get` returns (the UI's
            // `JobCount` type expects objects, not `[state, count]` pairs).
            let jobs = crate::commands::metrics::job_counts(
                crate::scheduler::queue::count_by_state(&pool)
                    .await
                    .unwrap_or_default(),
            );
            emit_event(
                &*emitter,
                EVENT_METRICS_TICK,
                serde_json::json!({
                    "free_bytes": snapshot.free_bytes,
                    "suggested_parallel": snapshot.suggested_parallel,
                    "reason": snapshot.reason,
                    "jobs": jobs,
                    "endpoints": worker.endpoint_usage(),
                    "worker_running": worker.is_running(),
                    "worker_paused": worker.is_paused(),
                    "sidecar": supervisor.status(),
                }),
            );
        }
    });
}
