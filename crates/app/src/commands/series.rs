//! `series_*` and `project_set_series` commands.
//!
//! A series is the shared canon of a saga (PLAN.md §9.5): a pinned language pair, the
//! glossary layer its books inherit (a project term overrides the series rendering for
//! the same source) and memory values. Nothing is copied into a book, so editing the
//! series reaches every member immediately.

use serde::{Deserialize, Serialize};
use tauri::State;

use super::Ack;
use crate::db::models::{Project, Series, SeriesGlossaryTerm, SeriesGlossaryVariant, SeriesMemory};
use crate::db::{new_id, now, repo};
use crate::error::{AppError, Result};
use crate::pipeline::glossary::{self, ProposalOutcome};
use crate::pipeline::series_bundle;
use crate::AppState;

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesCreate {
    pub name: String,
    #[serde(default)]
    pub source_lang: Option<String>,
    #[serde(default)]
    pub target_lang: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesUpdate {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub source_lang: Option<String>,
    #[serde(default)]
    pub target_lang: Option<String>,
    /// Free-form settings JSON; `None` leaves the stored value untouched.
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
    /// Series style guide (written to `series_memory`), `None` leaves it untouched.
    #[serde(default)]
    pub style_guide: Option<String>,
    /// Series synopsis, `None` leaves it untouched.
    #[serde(default)]
    pub synopsis: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesDetail {
    pub series: Series,
    /// Member books, in `series_order`.
    pub projects: Vec<Project>,
    pub memory: Vec<SeriesMemory>,
    /// The whole series glossary + aliases, so the view loads with one call.
    pub glossary: Vec<SeriesGlossaryTerm>,
    pub variants: Vec<SeriesGlossaryVariant>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectSetSeries {
    pub project_id: String,
    /// `None` detaches the book from its series.
    #[serde(default)]
    pub series_id: Option<String>,
    #[serde(default)]
    pub series_order: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesGlossaryUpsert {
    /// Absent or `null` creates a term; present edits the existing row.
    #[serde(default)]
    pub id: Option<String>,
    pub series_id: String,
    pub source: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub source_lang: Option<String>,
    #[serde(default)]
    pub target_lang: Option<String>,
    #[serde(default)]
    pub expected_revision: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesVariantUpsert {
    pub term_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesPromote {
    pub project_id: String,
    pub term_id: String,
}

/// Serializable outcome of a promotion, so the UI can react without parsing strings.
#[derive(Debug, Clone, Serialize)]
pub struct PromoteOutcome {
    pub outcome: ProposalOutcome,
}

fn normalize_kind(raw: &str) -> &'static str {
    match raw.trim().to_lowercase().as_str() {
        "proper_noun" => "proper_noun",
        "do_not_translate" => "do_not_translate",
        _ => "term",
    }
}

fn normalize_status(raw: &str) -> &'static str {
    match raw.trim().to_lowercase().as_str() {
        "candidate" => "candidate",
        "rejected" => "rejected",
        "conflict" => "conflict",
        _ => "approved",
    }
}

#[tauri::command]
pub async fn series_list(state: State<'_, AppState>) -> Result<Vec<Series>> {
    repo::list_series(&state.pool).await
}

#[tauri::command]
pub async fn series_create(state: State<'_, AppState>, req: SeriesCreate) -> Result<Series> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::Invalid("series name is required".into()));
    }
    let timestamp = now();
    let series = Series {
        id: new_id(),
        name,
        source_lang: req.source_lang.filter(|lang| !lang.trim().is_empty()),
        target_lang: req.target_lang.filter(|lang| !lang.trim().is_empty()),
        settings_json: "{}".to_string(),
        created_at: timestamp.clone(),
        updated_at: timestamp,
    };
    repo::upsert_series(&state.pool, &series).await?;
    Ok(series)
}

#[tauri::command]
pub async fn series_get(state: State<'_, AppState>, id: String) -> Result<SeriesDetail> {
    let series = repo::get_series(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series {id}")))?;
    let projects = repo::list_projects_for_series(&state.pool, &id).await?;
    let memory = repo::list_series_memory(&state.pool, &id).await?;
    let glossary = repo::list_series_terms(&state.pool, &id).await?;
    let variants = repo::list_variants_for_series(&state.pool, &id).await?;
    Ok(SeriesDetail {
        series,
        projects,
        memory,
        glossary,
        variants,
    })
}

#[tauri::command]
pub async fn series_update(state: State<'_, AppState>, req: SeriesUpdate) -> Result<Series> {
    let mut series = repo::get_series(&state.pool, &req.id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series {}", req.id)))?;
    if let Some(name) = req.name {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::Invalid("series name cannot be empty".into()));
        }
        series.name = name;
    }
    if let Some(source_lang) = req.source_lang {
        series.source_lang = (!source_lang.trim().is_empty()).then_some(source_lang);
    }
    if let Some(target_lang) = req.target_lang {
        series.target_lang = (!target_lang.trim().is_empty()).then_some(target_lang);
    }
    if let Some(settings) = req.settings {
        series.settings_json = serde_json::to_string(&settings)?;
    }
    series.updated_at = now();
    repo::upsert_series(&state.pool, &series).await?;

    // The memory keys are ordinary values; writing them here keeps the series
    // surface in one command, like `recon_confirm` does for a book.
    if let Some(style_guide) = req.style_guide {
        repo::set_series_memory(&state.pool, &req.id, "style_guide", &style_guide).await?;
    }
    if let Some(synopsis) = req.synopsis {
        repo::set_series_memory(&state.pool, &req.id, "synopsis", &synopsis).await?;
    }
    Ok(series)
}

#[tauri::command]
pub async fn series_delete(state: State<'_, AppState>, id: String) -> Result<Ack> {
    repo::delete_series(&state.pool, &id).await?;
    Ok(Ack::done())
}

/// Place a project in a series (or detach it). The language pair must match: a book
/// translated into another target language cannot share the series canon.
#[tauri::command]
pub async fn project_set_series(
    state: State<'_, AppState>,
    req: ProjectSetSeries,
) -> Result<Project> {
    let project = repo::get_project(&state.pool, &req.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", req.project_id)))?;

    if let Some(series_id) = req.series_id.as_deref() {
        let series = repo::get_series(&state.pool, series_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("series {series_id}")))?;
        if let Some(target_lang) = series.target_lang.as_deref() {
            if !target_lang.eq_ignore_ascii_case(&project.target_lang) {
                return Err(AppError::Invalid(format!(
                    "the series is {target_lang}, the project is {}",
                    project.target_lang
                )));
            }
        }
        if let (Some(series_source), Some(project_source)) = (
            series.source_lang.as_deref(),
            project.source_lang.as_deref(),
        ) {
            if !series_source.eq_ignore_ascii_case(project_source) {
                return Err(AppError::Invalid(format!(
                    "the series is translated from {series_source}, the project from {project_source}"
                )));
            }
        }
    }

    repo::set_project_series(
        &state.pool,
        &req.project_id,
        req.series_id.as_deref(),
        req.series_order,
    )
    .await?;
    repo::get_project(&state.pool, &req.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", req.project_id)))
}

#[tauri::command]
pub async fn series_glossary_list(
    state: State<'_, AppState>,
    series_id: String,
) -> Result<Vec<SeriesGlossaryTerm>> {
    repo::list_series_terms(&state.pool, &series_id).await
}

#[tauri::command]
pub async fn series_glossary_upsert(
    state: State<'_, AppState>,
    req: SeriesGlossaryUpsert,
) -> Result<SeriesGlossaryTerm> {
    let series = repo::get_series(&state.pool, &req.series_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series {}", req.series_id)))?;

    let source = req.source.trim().to_string();
    if source.is_empty() {
        return Err(AppError::Invalid("glossary source is required".into()));
    }
    let kind = normalize_kind(req.kind.as_deref().unwrap_or_default());
    let target = req.target.unwrap_or_default().trim().to_string();
    let target = if target.is_empty() && kind == "do_not_translate" {
        source.clone()
    } else if target.is_empty() {
        return Err(AppError::Invalid(
            "glossary target is required for a translated term".into(),
        ));
    } else {
        target
    };

    let term = SeriesGlossaryTerm {
        id: req.id.unwrap_or_else(new_id),
        series_id: req.series_id.clone(),
        // An empty string instead of NULL keeps the unique key working in SQLite,
        // which treats NULLs as distinct.
        source_lang: req
            .source_lang
            .or_else(|| series.source_lang.clone())
            .or_else(|| Some(String::new())),
        target_lang: req
            .target_lang
            .or_else(|| series.target_lang.clone())
            .or_else(|| Some(String::new())),
        source,
        target,
        note: req.note.filter(|note| !note.trim().is_empty()),
        kind: kind.to_string(),
        origin: "manual".to_string(),
        revision: 1,
        status: normalize_status(req.status.as_deref().unwrap_or_default()).to_string(),
    };

    if let Some(expected_revision) = req.expected_revision {
        if !repo::update_series_term_checked(&state.pool, &term, expected_revision).await? {
            return Err(AppError::Invalid(format!(
                "series glossary term {} changed since it was loaded; reload the glossary",
                term.id
            )));
        }
    } else {
        repo::upsert_series_term(&state.pool, &term).await?;
    }

    // The canon changed: every member book rendering the same source differently must
    // see a conflict. A failure here never fails the write.
    if term.status != "rejected" {
        if let Err(error) =
            glossary::flag_series_conflicts(&state.pool, &req.series_id, &term.source, &term.target)
                .await
        {
            tracing::warn!(series_id = %req.series_id, %error, "could not flag series conflicts");
        }
    }

    // Read the persisted row back: the unique key may have updated an existing row
    // (and bumped its revision), so the constructed value would be stale.
    if let Some(stored) = repo::get_series_term(&state.pool, &term.id).await? {
        return Ok(stored);
    }
    if let Some(stored) =
        repo::get_series_term_by_source(&state.pool, &term.series_id, &term.source).await?
    {
        return Ok(stored);
    }
    Ok(term)
}

#[tauri::command]
pub async fn series_glossary_delete(state: State<'_, AppState>, id: String) -> Result<Ack> {
    repo::delete_series_term(&state.pool, &id).await?;
    Ok(Ack::done())
}

#[tauri::command]
pub async fn series_variant_upsert(
    state: State<'_, AppState>,
    req: SeriesVariantUpsert,
) -> Result<SeriesGlossaryVariant> {
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return Err(AppError::Invalid("variant text is required".into()));
    }
    repo::get_series_term(&state.pool, &req.term_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series glossary term {}", req.term_id)))?;
    let variant = SeriesGlossaryVariant {
        id: new_id(),
        term_id: req.term_id,
        text,
    };
    repo::upsert_series_variant(&state.pool, &variant).await?;
    Ok(variant)
}

#[tauri::command]
pub async fn series_variant_delete(state: State<'_, AppState>, id: String) -> Result<Ack> {
    repo::delete_series_variant(&state.pool, &id).await?;
    Ok(Ack::done())
}

/// Copy a book term into its series. A differing series rendering is kept and flagged,
/// never overwritten.
#[tauri::command]
pub async fn series_promote_term(
    state: State<'_, AppState>,
    req: SeriesPromote,
) -> Result<PromoteOutcome> {
    let outcome = glossary::promote_term(&state.pool, &req.project_id, &req.term_id).await?;
    Ok(PromoteOutcome { outcome })
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesExportRequest {
    pub series_id: String,
    /// Destination `.llmtsz`; when absent it goes under `<app data>/series/`.
    #[serde(default)]
    pub output_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesImportRequest {
    pub archive_path: String,
}

/// Write the series canon as a `.llmtsz` bundle.
#[tauri::command]
pub async fn series_export(
    state: State<'_, AppState>,
    req: SeriesExportRequest,
) -> Result<series_bundle::SeriesExportOutcome> {
    series_bundle::export_series(
        &state.pool,
        &state.data_dir,
        &req.series_id,
        req.output_path.as_deref(),
    )
    .await
}

/// Merge a series bundle into the local database (never overwriting a rendering).
#[tauri::command]
pub async fn series_import(
    state: State<'_, AppState>,
    req: SeriesImportRequest,
) -> Result<series_bundle::SeriesImportOutcome> {
    series_bundle::import_series(&state.pool, &req.archive_path).await
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesQaScanResult {
    pub enqueued: usize,
}

/// Re-run the QA scan on every translated chunk of every member book, for example after a
/// canon change. Uses the existing `qa_scan` job, so the worker pool and the events are the
/// usual ones.
#[tauri::command]
pub async fn series_qa_scan(
    state: State<'_, AppState>,
    series_id: String,
) -> Result<SeriesQaScanResult> {
    repo::get_series(&state.pool, &series_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series {series_id}")))?;
    let projects = repo::list_projects_for_series(&state.pool, &series_id).await?;
    let mut enqueued = 0usize;
    for project in projects {
        for chunk in repo::list_chunks_by_project(&state.pool, &project.id, None).await? {
            if !matches!(chunk.status.as_str(), "done" | "needs_review") {
                continue;
            }
            if chunk
                .target_md
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
            {
                continue;
            }
            let payload = serde_json::json!({ "chunk_id": chunk.id });
            if crate::scheduler::queue::has_pending(
                &state.pool,
                &project.id,
                crate::pipeline::qa::JOB_KIND,
                &payload,
            )
            .await?
            {
                continue;
            }
            let job =
                crate::scheduler::NewJob::new(&project.id, crate::pipeline::qa::JOB_KIND, payload)
                    .with_priority(60 + chunk.order_index);
            super::enqueue_and_emit(&state, &job).await?;
            enqueued += 1;
        }
    }
    if enqueued > 0 {
        state.worker.start();
    }
    Ok(SeriesQaScanResult { enqueued })
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesReconStart {
    pub series_id: String,
    /// Re-synthesize even when no book profile or canon changed.
    #[serde(default)]
    pub force: bool,
}

/// Enqueue the `series_recon` job: a candidate series profile built from the member
/// books' confirmed profiles and the canon glossary. The job belongs to the first member
/// book (the queue is project-scoped) and carries the `series_id` in its payload. Without
/// `force`, a run whose books and canon are unchanged completes without calling the model.
#[tauri::command]
pub async fn series_recon_start(
    state: State<'_, AppState>,
    req: SeriesReconStart,
) -> Result<super::ingest::JobStarted> {
    let series = repo::get_series(&state.pool, &req.series_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series {}", req.series_id)))?;
    let projects = repo::list_projects_for_series(&state.pool, &req.series_id).await?;
    let first = projects.first().ok_or_else(|| {
        AppError::Invalid("the series has no member book: attach one first".into())
    })?;
    if repo::role_binding_for(&state.pool, crate::pipeline::series_recon::ROLE)
        .await?
        .is_none()
    {
        return Err(AppError::Invalid(
            "no role_binding configured for 'orchestrator': bind a model to run the series reconnaissance"
                .into(),
        ));
    }

    let payload = serde_json::json!({ "series_id": series.id, "force": req.force });
    let job =
        crate::scheduler::NewJob::new(&first.id, crate::pipeline::series_recon::JOB_KIND, payload)
            .with_priority(20);
    let job = super::enqueue_and_emit(&state, &job).await?;
    state.worker.start();
    Ok(super::ingest::JobStarted { job_id: job.id })
}

/// Apply the user's decisions on the candidate series profile: accepted fields land in the
/// series memory, accepted characters become approved canon terms, rejected sources are
/// remembered so a later run skips them.
#[tauri::command]
pub async fn series_recon_confirm(
    state: State<'_, AppState>,
    req: crate::pipeline::series_recon::ConfirmRequest,
) -> Result<crate::pipeline::series_recon::ConfirmOutcome> {
    crate::pipeline::series_recon::confirm(&state.pool, &req).await
}
