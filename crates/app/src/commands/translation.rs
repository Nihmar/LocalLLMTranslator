//! `translation_start` / `translation_pause` / `translation_cancel`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{enqueue_if_absent_and_emit, Ack};
use crate::db::models::Chunk;
use crate::db::repo;
use crate::error::{AppError, Result};
use crate::pipeline::export::chunk_has_translation;
use crate::scheduler::{queue, NewJob};
use crate::AppState;

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct TranslationStartRequest {
    #[serde(default)]
    pub project_id: Option<String>,
    /// Only re-enqueue chunks that previously failed or need review.
    #[serde(default)]
    pub only_retry: bool,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TranslationStartResult {
    pub enqueued: usize,
    pub running: bool,
}

/// Request for `translation_cancel`.
///
/// The optional `project_id` scopes the cancellation to a single project; when
/// omitted every project's translation work is cancelled, preserving the old
/// global behaviour for callers that do not pass the argument.
#[derive(Debug, Clone, Default, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct TranslationCancelRequest {
    #[serde(default)]
    pub project_id: Option<String>,
}

/// Whether `translation_start` may enqueue a `translate_chunk` job for a chunk.
///
/// `running` is deliberately excluded: a chunk in that state has a live job, and
/// re-enqueuing it would duplicate work. Chunks left `running` by a crash are
/// returned to `pending` at boot (`repo::reset_running_chunks`) or on cancel, so
/// they become eligible again through the normal `pending` path.
///
/// A `needs_review` chunk that kept a translation is redone only on an explicit
/// retry: resuming must not spend a model call replacing text the user may already
/// have read. One without a translation (a block-count mismatch) is still owed one.
pub fn chunk_is_eligible(chunk: &Chunk, only_retry: bool) -> bool {
    match chunk.status.as_str() {
        "failed" => true,
        "needs_review" => only_retry || !chunk_has_translation(chunk),
        "pending" => !only_retry,
        _ => false,
    }
}

/// The chunks a start has to enqueue: eligible by status and **not already waiting**
/// in the queue.
///
/// Without the second condition, pressing "Avvia / Riprendi" while a queue is running
/// (or after a pause) enqueues a second job for the same chunk; that duplicate then
/// translates a chunk which by then is already `done`, and a refused duplicate would
/// overwrite the validated text.
pub fn enqueue_candidates<'a>(
    chunks: &'a [Chunk],
    only_retry: bool,
    queued: &HashSet<String>,
) -> Vec<&'a Chunk> {
    chunks
        .iter()
        .filter(|chunk| chunk_is_eligible(chunk, only_retry))
        .filter(|chunk| !queued.contains(&chunk.id))
        .collect()
}

#[tauri::command(rename = "translation_start", rename_all = "snake_case")]
pub async fn translation_start_command(
    state: State<'_, AppState>,
    req: TranslationStartRequest,
) -> Result<TranslationStartResult> {
    translation_start(&state, req).await
}

pub async fn translation_start(
    state: &AppState,
    req: TranslationStartRequest,
) -> Result<TranslationStartResult> {
    // Collect the chunks that need work. Priority follows the chapter order, so
    // the book is translated sequentially for coherence.
    let project_id = req
        .project_id
        .clone()
        .ok_or_else(|| AppError::Invalid("project_id is required".into()))?;
    let chunks = repo::list_chunks_by_project(&state.pool, &project_id, None).await?;
    let queued = queue::active_chunk_ids(&state.pool, &project_id, "translate_chunk").await?;

    let mut enqueued = 0usize;
    for chunk in enqueue_candidates(&chunks, req.only_retry, &queued) {
        let payload = serde_json::json!({ "chunk_id": chunk.id });
        let job = NewJob::new(&project_id, "translate_chunk", payload)
            .with_priority(100 + chunk.order_index);
        if enqueue_if_absent_and_emit(state, &job).await? {
            enqueued += 1;
        }
    }

    state.worker.start();
    Ok(TranslationStartResult {
        enqueued,
        running: state.worker.is_running(),
    })
}

#[tauri::command(rename = "translation_pause", rename_all = "snake_case")]
pub async fn translation_pause_command(state: State<'_, AppState>) -> Result<Ack> {
    translation_pause(&state).await
}

pub async fn translation_pause(state: &AppState) -> Result<Ack> {
    state.worker.pause();
    // Persist the explicit pause so it survives a relaunch: `build_state` used to
    // start the pool unconditionally, silently resuming LLM work the user had
    // stopped.
    repo::set_app_state(&state.pool, repo::KEY_WORKER_PAUSED, "1").await?;
    Ok(Ack::done())
}

/// Cancel the unfinished translation work of one project.
///
/// The optional `project_id` scopes the cancellation: with it only that project's
/// `translate_chunk` jobs are cancelled and its `running` chunks are returned to
/// `pending`; without it every project is cancelled. Previously the command took
/// no argument and cancelled *every* unfinished job in the database, so one
/// project's "Annulla" destroyed another project's queued work.
#[tauri::command(rename = "translation_cancel", rename_all = "snake_case")]
pub async fn translation_cancel_command(
    state: State<'_, AppState>,
    req: Option<TranslationCancelRequest>,
) -> Result<Ack> {
    translation_cancel(&state, req).await
}

pub async fn translation_cancel(
    state: &AppState,
    req: Option<TranslationCancelRequest>,
) -> Result<Ack> {
    let project_id = req.and_then(|request| request.project_id);
    state.worker.cancel();
    // A chunk mid-run keeps `running` when its job task is aborted; return those
    // to `pending` so a later start can retry them, then cancel every unfinished
    // job of the project and announce each `cancelled` transition.
    repo::reset_running_chunks(&state.pool, project_id.as_deref()).await?;
    let cancelled = queue::cancel_active(&state.pool, project_id.as_deref()).await?;
    for job in &cancelled {
        crate::events::emit_job(&*state.emitter, job);
    }
    // Cancelling clears any persisted pause: there is no queued work to resume.
    repo::delete_app_state(&state.pool, repo::KEY_WORKER_PAUSED).await?;
    Ok(Ack::done())
}

#[cfg(test)]
mod tests {
    use super::{chunk_is_eligible, enqueue_candidates, Chunk, HashSet, TranslationCancelRequest};

    fn translated(id: &str, status: &str) -> Chunk {
        Chunk {
            target_md: Some("text".to_string()),
            ..chunk(id, status)
        }
    }

    fn chunk(id: &str, status: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            document_id: "d".to_string(),
            chapter_id: None,
            order_index: 0,
            block_ids_json: "[]".to_string(),
            source_md: "a".to_string(),
            token_estimate: 1,
            context_json: "{}".to_string(),
            flags_json: "[]".to_string(),
            status: status.to_string(),
            prompt_hash: None,
            model_id: None,
            params_json: None,
            context_manifest_json: None,
            target_md: None,
            error: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn cancel_request_project_id_is_optional() {
        let none: TranslationCancelRequest =
            serde_json::from_value(serde_json::json!({})).expect("empty request");
        assert!(none.project_id.is_none());

        let some: TranslationCancelRequest =
            serde_json::from_value(serde_json::json!({ "project_id": "p1" }))
                .expect("scoped request");
        assert_eq!(some.project_id.as_deref(), Some("p1"));
    }

    #[test]
    fn a_chunk_already_in_the_queue_is_not_enqueued_again() {
        let chunks = [
            chunk("c-pending", "pending"),
            chunk("c-queued", "pending"),
            chunk("c-running", "running"),
            chunk("c-review", "needs_review"),
            translated("c-flagged", "needs_review"),
            chunk("c-failed", "failed"),
            chunk("c-done", "done"),
        ];
        let queued = HashSet::from(["c-queued".to_string(), "c-running".to_string()]);

        // "Avvia / Riprendi": everything eligible and not already waiting; a flagged
        // chunk that kept its translation is left alone.
        let fresh: Vec<&str> = enqueue_candidates(&chunks, false, &queued)
            .iter()
            .map(|chunk| chunk.id.as_str())
            .collect();
        assert_eq!(fresh, ["c-pending", "c-review", "c-failed"]);

        // "Riprova falliti" re-queues what failed or was flagged, still not a queued chunk.
        let retry: Vec<&str> = enqueue_candidates(&chunks, true, &queued)
            .iter()
            .map(|chunk| chunk.id.as_str())
            .collect();
        assert_eq!(retry, ["c-review", "c-flagged", "c-failed"]);

        // A chunk already waiting is never queued twice, whatever its status.
        let all_queued: HashSet<String> = chunks.iter().map(|chunk| chunk.id.clone()).collect();
        assert!(enqueue_candidates(&chunks, false, &all_queued).is_empty());
    }

    #[test]
    fn eligibility_matches_chunk_lifecycle() {
        // A running chunk has a live job: never re-enqueued.
        assert!(!chunk_is_eligible(&chunk("c", "running"), false));
        assert!(!chunk_is_eligible(&chunk("c", "running"), true));
        // Fresh and retryable states.
        assert!(chunk_is_eligible(&chunk("c", "pending"), false));
        assert!(chunk_is_eligible(&chunk("c", "failed"), false));
        assert!(chunk_is_eligible(&chunk("c", "needs_review"), false));
        // Completed chunks are only re-enqueued on an explicit retry.
        assert!(!chunk_is_eligible(&chunk("c", "done"), false));
        assert!(!chunk_is_eligible(&chunk("c", "done"), true));
        assert!(!chunk_is_eligible(&chunk("c", "pending"), true));
        assert!(chunk_is_eligible(&chunk("c", "failed"), true));
        assert!(chunk_is_eligible(&chunk("c", "needs_review"), true));
        // A flagged chunk that kept its translation waits for an explicit retry.
        assert!(!chunk_is_eligible(&translated("c", "needs_review"), false));
        assert!(chunk_is_eligible(&translated("c", "needs_review"), true));
    }
}
