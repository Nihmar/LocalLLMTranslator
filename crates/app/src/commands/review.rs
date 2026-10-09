//! `review_start`, `suggestion_list`, `suggestion_history`, `suggestion_accept`,
//! `suggestion_reject` and `qa_report` (PLAN.md §11.4).

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{pipeline_deps, Ack};
use crate::db::models::Suggestion;
use crate::db::repo;
use crate::error::{AppError, Result};
use crate::pipeline::review;
use crate::scheduler::queue;
use crate::views::QaFindingView;
use crate::AppState;

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct ReviewStartRequest {
    pub project_id: String,
    /// Restrict to these chunks; omitted means every eligible chunk.
    #[serde(default)]
    pub chunk_ids: Option<Vec<String>>,
    #[serde(default)]
    pub chapter_id: Option<String>,
    /// `editor` | `proofreader` | `both` (default).
    #[serde(default)]
    pub pass: Option<String>,
    /// Also re-run the QA scan on the selected chunks.
    #[serde(default)]
    pub with_qa: bool,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ReviewStartResult {
    pub enqueued: usize,
}

/// Enqueue the review passes for the eligible chunks. A chunk is eligible when
/// it is `done`/`needs_review` and carries a translation; an equivalent pending
/// job suppresses a duplicate.
#[tauri::command(rename = "review_start", rename_all = "snake_case")]
pub async fn review_start_command(
    state: State<'_, AppState>,
    req: ReviewStartRequest,
) -> Result<ReviewStartResult> {
    review_start(&state, req).await
}

pub async fn review_start(state: &AppState, req: ReviewStartRequest) -> Result<ReviewStartResult> {
    let pass = req.pass.as_deref().unwrap_or("both");
    if !matches!(pass, "editor" | "proofreader" | "both") {
        return Err(AppError::Invalid(format!("unknown review pass '{pass}'")));
    }

    let job_ids = review::enqueue_review_jobs(
        &state.pool,
        &req.project_id,
        req.chunk_ids.as_deref(),
        req.chapter_id.as_deref(),
        pass,
        req.with_qa,
    )
    .await?;

    for job_id in &job_ids {
        if let Some(job) = queue::get_job(&state.pool, job_id).await? {
            crate::events::emit_job(&*state.emitter, &job);
        }
    }
    if !job_ids.is_empty() {
        state.worker.start();
    }
    Ok(ReviewStartResult {
        enqueued: job_ids.len(),
    })
}

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct SuggestionListRequest {
    pub project_id: String,
    #[serde(default)]
    pub chunk_id: Option<String>,
    #[serde(default)]
    pub pass: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[tauri::command(rename = "suggestion_list", rename_all = "snake_case")]
pub async fn suggestion_list_command(
    state: State<'_, AppState>,
    req: SuggestionListRequest,
) -> Result<Vec<Suggestion>> {
    suggestion_list(&state, req).await
}

pub async fn suggestion_list(
    state: &AppState,
    req: SuggestionListRequest,
) -> Result<Vec<Suggestion>> {
    repo::list_suggestions(
        &state.pool,
        &req.project_id,
        req.chunk_id.as_deref(),
        req.pass.as_deref(),
        req.status.as_deref(),
    )
    .await
}

/// How many decisions `suggestion_history` returns when the caller sets no limit.
const DEFAULT_HISTORY_LIMIT: u32 = 1000;

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct SuggestionHistoryRequest {
    pub project_id: String,
    #[serde(default)]
    pub chunk_id: Option<String>,
    #[serde(default)]
    pub pass: Option<String>,
    /// `accepted` | `rejected`; omitted returns both.
    #[serde(default)]
    pub status: Option<String>,
    /// Newest decisions first; defaults to 1000.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// The project's correction history: the decided suggestions, newest first.
#[tauri::command(rename = "suggestion_history", rename_all = "snake_case")]
pub async fn suggestion_history_command(
    state: State<'_, AppState>,
    req: SuggestionHistoryRequest,
) -> Result<Vec<Suggestion>> {
    suggestion_history(&state, req).await
}

pub async fn suggestion_history(
    state: &AppState,
    req: SuggestionHistoryRequest,
) -> Result<Vec<Suggestion>> {
    if let Some(status) = req.status.as_deref() {
        if !matches!(status, "accepted" | "rejected") {
            return Err(AppError::Invalid(format!(
                "unknown suggestion status '{status}'"
            )));
        }
    }
    repo::list_decided_suggestions(
        &state.pool,
        &req.project_id,
        req.chunk_id.as_deref(),
        req.pass.as_deref(),
        req.status.as_deref(),
        i64::from(req.limit.unwrap_or(DEFAULT_HISTORY_LIMIT)),
    )
    .await
}

/// Accept a proposal: the block translation is rewritten and the chunk's
/// `target_md` recomposed, after the local markup guard.
#[tauri::command(rename = "suggestion_accept", rename_all = "snake_case")]
pub async fn suggestion_accept_command(
    state: State<'_, AppState>,
    id: String,
) -> Result<Suggestion> {
    suggestion_accept(&state, id).await
}

pub async fn suggestion_accept(state: &AppState, id: String) -> Result<Suggestion> {
    let deps = pipeline_deps(state);
    review::accept_suggestion(&deps, &id).await
}

#[tauri::command(rename = "suggestion_reject", rename_all = "snake_case")]
pub async fn suggestion_reject_command(
    state: State<'_, AppState>,
    id: String,
) -> Result<Suggestion> {
    suggestion_reject(&state, id).await
}

pub async fn suggestion_reject(state: &AppState, id: String) -> Result<Suggestion> {
    review::reject_suggestion(&state.pool, &id).await
}

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct QaReportRequest {
    pub project_id: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub chunk_id: Option<String>,
    /// `open` | `resolved` | `ignored`; `None` returns every status.
    #[serde(default)]
    pub status: Option<String>,
}

#[tauri::command(rename = "qa_report", rename_all = "snake_case")]
pub async fn qa_report_command(
    state: State<'_, AppState>,
    req: QaReportRequest,
) -> Result<Vec<QaFindingView>> {
    qa_report(&state, req).await
}

pub async fn qa_report(state: &AppState, req: QaReportRequest) -> Result<Vec<QaFindingView>> {
    let findings = repo::list_qa_findings_filtered(
        &state.pool,
        &req.project_id,
        req.kind.as_deref(),
        req.severity.as_deref(),
        req.chunk_id.as_deref(),
        req.status.as_deref(),
    )
    .await?;
    Ok(findings.into_iter().map(QaFindingView::from).collect())
}

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct QaFindingStatusRequest {
    pub id: String,
    /// `open` | `resolved` | `ignored`.
    pub status: String,
}

/// Close or reopen a QA finding. Conflicts resolved from the Series view use this; the
/// re-scan of a chunk replaces its findings anyway, so a closed finding is only a decision
/// marker until the next scan.
#[tauri::command(rename = "qa_finding_set_status", rename_all = "snake_case")]
pub async fn qa_finding_set_status_command(
    state: State<'_, AppState>,
    req: QaFindingStatusRequest,
) -> Result<Ack> {
    qa_finding_set_status(&state, req).await
}

pub async fn qa_finding_set_status(state: &AppState, req: QaFindingStatusRequest) -> Result<Ack> {
    let status = req.status.as_str();
    if !matches!(status, "open" | "resolved" | "ignored") {
        return Err(AppError::Invalid(format!(
            "unknown QA finding status '{status}'"
        )));
    }
    repo::set_qa_finding_status(&state.pool, &req.id, status).await?;
    Ok(Ack::done())
}
