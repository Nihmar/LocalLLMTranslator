//! `chunk_list` and `chunk_get`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::models::{Block, BlockTranslation, Chunk, LlmCall};
use crate::db::repo;
use crate::error::{AppError, Result};
use crate::AppState;

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct ChunkListRequest {
    pub project_id: String,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ChunkDetail {
    pub chunk: Chunk,
    pub blocks: Vec<Block>,
    pub translations: Vec<BlockTranslation>,
    pub llm_calls: Vec<LlmCall>,
}

#[tauri::command(rename = "chunk_list", rename_all = "snake_case")]
pub async fn chunk_list_command(
    state: State<'_, AppState>,
    req: ChunkListRequest,
) -> Result<Vec<Chunk>> {
    chunk_list(&state, req).await
}

pub async fn chunk_list(state: &AppState, req: ChunkListRequest) -> Result<Vec<Chunk>> {
    repo::list_chunks_by_project(&state.pool, &req.project_id, req.status.as_deref()).await
}

#[tauri::command(rename = "chunk_get", rename_all = "snake_case")]
pub async fn chunk_get_command(
    state: State<'_, AppState>,
    chunk_id: String,
) -> Result<ChunkDetail> {
    chunk_get(&state, chunk_id).await
}

pub async fn chunk_get(state: &AppState, chunk_id: String) -> Result<ChunkDetail> {
    let chunk = repo::get_chunk(&state.pool, &chunk_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("chunk {chunk_id}")))?;

    let block_ids: Vec<String> = serde_json::from_str(&chunk.block_ids_json).unwrap_or_default();
    let wanted: HashSet<&str> = block_ids.iter().map(String::as_str).collect();
    let all_blocks = repo::list_blocks(&state.pool, &chunk.document_id).await?;
    let blocks: Vec<Block> = all_blocks
        .into_iter()
        .filter(|b| wanted.contains(b.id.as_str()))
        .collect();

    let translations = repo::list_block_translations(&state.pool, &chunk.id).await?;

    let llm_calls = sqlx::query_as::<_, LlmCall>(
        "SELECT * FROM llm_call WHERE chunk_id = ?1 ORDER BY created_at",
    )
    .bind(&chunk.id)
    .fetch_all(&state.pool)
    .await?;

    Ok(ChunkDetail {
        chunk,
        blocks,
        translations,
        llm_calls,
    })
}
