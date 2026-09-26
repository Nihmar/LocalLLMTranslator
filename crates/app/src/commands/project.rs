//! `project_*` commands.

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{Ack, CreateProjectRequest};
use crate::db::models::{Chapter, Project};
use crate::db::{new_id, now, repo};
use crate::error::Result;
use crate::util::{sha256_hex, sha256_hex_str};
use crate::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct ProjectDetail {
    pub project: Project,
    pub chapters: Vec<Chapter>,
    pub chunks_total: i64,
    pub chunks_done: i64,
}

#[tauri::command]
pub async fn project_list(state: State<'_, AppState>) -> Result<Vec<Project>> {
    repo::list_projects(&state.pool).await
}

#[tauri::command]
pub async fn project_create(
    state: State<'_, AppState>,
    req: CreateProjectRequest,
) -> Result<Project> {
    let source_hash = match tokio::fs::read(&req.source_path).await {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => sha256_hex_str(&req.source_path),
    };
    let timestamp = now();
    let project = Project {
        id: new_id(),
        name: req.name,
        source_path: req.source_path,
        source_hash,
        source_format: req.source_format.unwrap_or_else(|| "unknown".to_string()),
        source_lang: req.source_lang,
        target_lang: req.target_lang,
        doc_title: req.doc_title,
        doc_author: req.doc_author,
        prompts_snapshot_dir: None,
        settings_json: req
            .settings
            .and_then(|s| serde_json::to_string(&s).ok())
            .unwrap_or_else(|| "{}".to_string()),
        created_at: timestamp.clone(),
        updated_at: timestamp,
    };
    repo::insert_project(&state.pool, &project).await?;
    Ok(project)
}

#[tauri::command]
pub async fn project_get(state: State<'_, AppState>, id: String) -> Result<ProjectDetail> {
    let project = repo::get_project(&state.pool, &id)
        .await?
        .ok_or_else(|| crate::error::AppError::NotFound(format!("project {id}")))?;

    let document_id: Option<String> =
        sqlx::query_scalar("SELECT id FROM document WHERE project_id = ?1 LIMIT 1")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;
    let chapters = match &document_id {
        Some(document_id) => repo::list_chapters(&state.pool, document_id).await?,
        None => Vec::new(),
    };
    let chunks_total: i64 = if let Some(document_id) = &document_id {
        sqlx::query_scalar("SELECT COUNT(*) FROM chunk WHERE document_id = ?1")
            .bind(document_id)
            .fetch_one(&state.pool)
            .await?
    } else {
        0
    };
    let chunks_done: i64 = if let Some(document_id) = &document_id {
        sqlx::query_scalar("SELECT COUNT(*) FROM chunk WHERE document_id = ?1 AND status = 'done'")
            .bind(document_id)
            .fetch_one(&state.pool)
            .await?
    } else {
        0
    };

    Ok(ProjectDetail {
        project,
        chapters,
        chunks_total,
        chunks_done,
    })
}

#[tauri::command]
pub async fn project_delete(state: State<'_, AppState>, id: String) -> Result<Ack> {
    repo::delete_project(&state.pool, &id).await?;
    Ok(Ack::done())
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExportBundleRequest {
    pub project_id: String,
    /// Destination `.llmtz`; when absent it goes to the project output directory.
    #[serde(default)]
    pub output_path: Option<String>,
}

/// Write the project as a `.llmtz` bundle (PLAN.md §6).
#[tauri::command]
pub async fn project_export(
    state: State<'_, AppState>,
    req: ExportBundleRequest,
) -> Result<crate::pipeline::bundle::ExportBundleOutcome> {
    crate::pipeline::bundle::export_project(
        &state.pool,
        &state.data_dir,
        &req.project_id,
        req.output_path.as_deref(),
    )
    .await
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportBundleRequest {
    pub archive_path: String,
}

/// Import a `.llmtz` bundle as a new project; an existing id is rejected.
#[tauri::command]
pub async fn project_import(
    state: State<'_, AppState>,
    req: ImportBundleRequest,
) -> Result<Project> {
    crate::pipeline::bundle::import_project(&state.pool, &state.data_dir, &req.archive_path).await
}
