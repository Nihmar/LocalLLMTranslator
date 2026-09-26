//! `translation_start` / `translation_pause` / `translation_cancel`.

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{enqueue_and_emit, Ack};
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

/// Request for `translation_cancel`.
///
/// The optional `project_id` scopes the cancellation to a single project; when
/// omitted every project's translation work is cancelled, preserving the old
/// global behaviour for callers that do not pass the argument.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TranslationCancelRequest {
    #[serde(default)]
    pub project_id: Option<String>,
}

/// Whether `translation_start` may enqueue a `translate_chunk` job for a chunk in
/// the given state.
///
/// `running` is deliberately excluded: a chunk in that state has a live job, and
/// re-enqueuing it would duplicate work. Chunks left `running` by a crash are
/// returned to `pending` at boot (`repo::reset_running_chunks`) or on cancel, so
/// they become eligible again through the normal `pending` path.
pub fn chunk_is_eligible(status: &str, only_retry: bool) -> bool {
    if only_retry {
        matches!(status, "failed" | "needs_review")
    } else {
        matches!(status, "pending" | "failed" | "needs_review")
    }
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
        if !chunk_is_eligible(&chunk.status, req.only_retry) {
            continue;
        }
        let payload = serde_json::json!({ "chunk_id": chunk.id });
        let job = NewJob::new(&project_id, "translate_chunk", payload)
            .with_priority(100 + chunk.order_index);
        enqueue_and_emit(&state, &job).await?;
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
#[tauri::command]
pub async fn translation_cancel(
    state: State<'_, AppState>,
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
    use super::{chunk_is_eligible, TranslationCancelRequest};

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
    fn eligibility_matches_chunk_lifecycle() {
        // A running chunk has a live job: never re-enqueued.
        assert!(!chunk_is_eligible("running", false));
        assert!(!chunk_is_eligible("running", true));
        // Fresh and retryable states.
        assert!(chunk_is_eligible("pending", false));
        assert!(chunk_is_eligible("failed", false));
        assert!(chunk_is_eligible("needs_review", false));
        // Completed chunks are only re-enqueued on an explicit retry.
        assert!(!chunk_is_eligible("done", false));
        assert!(!chunk_is_eligible("done", true));
        assert!(!chunk_is_eligible("pending", true));
        assert!(chunk_is_eligible("failed", true));
        assert!(chunk_is_eligible("needs_review", true));
    }
}
