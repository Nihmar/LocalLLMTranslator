//! `translation_start` / `translation_pause` / `translation_cancel`.

use serde::{Deserialize, Serialize};
use tauri::State;

use super::Ack;
use crate::db::repo;
use crate::error::{AppError, Result};
use crate::scheduler::{queue, NewJob};
use crate::AppState;

#[derive(Debug, Clone, Deserialize)]
pub struct TranslationStartRequest {
    #[serde(default)]
    pub project_id: Option<String>,
    /// Only re-enqueue chunks that previously failed or need review.
    #[serde(default)]
    pub only_retry: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranslationStartResult {
    pub enqueued: usize,
    pub running: bool,
}

#[tauri::command]
pub async fn translation_start(
    state: State<'_, AppState>,
    req: TranslationStartRequest,
) -> Result<TranslationStartResult> {
    // Collect the chunks that need work. Priority follows the chapter order, so
    // the book is translated sequentially for coherence.
    let project_id = req
        .project_id
        .clone()
        .ok_or_else(|| AppError::Invalid("project_id is required".into()))?;
    let chunks = repo::list_chunks_by_project(&state.pool, &project_id, None).await?;

    let mut enqueued = 0usize;
    for chunk in chunks {
        let eligible = if req.only_retry {
            matches!(chunk.status.as_str(), "failed" | "needs_review")
        } else {
            matches!(chunk.status.as_str(), "pending" | "failed" | "needs_review")
        };
        if !eligible {
            continue;
        }
        let payload = serde_json::json!({ "chunk_id": chunk.id });
        let job = NewJob::new(&project_id, "translate_chunk", payload)
            .with_priority(100 + chunk.order_index);
        queue::enqueue(&state.pool, &job).await?;
        enqueued += 1;
    }

    state.worker.start();
    Ok(TranslationStartResult {
        enqueued,
        running: state.worker.is_running(),
    })
}

#[tauri::command]
pub async fn translation_pause(state: State<'_, AppState>) -> Result<Ack> {
    state.worker.pause();
    Ok(Ack::done())
}

#[tauri::command]
pub async fn translation_cancel(state: State<'_, AppState>) -> Result<Ack> {
    state.worker.cancel();
    Ok(Ack::done())
}
