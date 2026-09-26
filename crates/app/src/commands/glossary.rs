//! `glossary_list` / `glossary_upsert` / `glossary_delete`.
//!
//! The glossary is user data: candidates proposed by the reconnaissance and the
//! summarizer are approved, edited or rejected here. The translator prompt only
//! ever sees non-rejected terms.

use serde::Deserialize;
use tauri::State;

use super::Ack;
use crate::db::models::GlossaryTerm;
use crate::db::{new_id, repo};
use crate::error::{AppError, Result};
use crate::AppState;

#[derive(Debug, Clone, Deserialize)]
pub struct GlossaryUpsert {
    /// Absent or `null` creates a term; present edits the existing row.
    #[serde(default)]
    pub id: Option<String>,
    pub project_id: String,
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
    /// Optimistic-lock revision the caller edited from; when `id` is present too,
    /// the write is rejected if another writer changed the row in the meantime.
    #[serde(default)]
    pub expected_revision: Option<i64>,
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
pub async fn glossary_list(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<GlossaryTerm>> {
    repo::list_glossary_terms(&state.pool, &project_id).await
}

#[tauri::command]
pub async fn glossary_upsert(
    state: State<'_, AppState>,
    req: GlossaryUpsert,
) -> Result<GlossaryTerm> {
    let project = repo::get_project(&state.pool, &req.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", req.project_id)))?;

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

    let is_update = req.id.is_some();
    let term = GlossaryTerm {
        id: req.id.unwrap_or_else(new_id),
        project_id: req.project_id.clone(),
        // An empty string instead of NULL keeps the unique key working in
        // SQLite, which treats NULLs as distinct.
        source_lang: req
            .source_lang
            .or_else(|| project.source_lang.clone())
            .or_else(|| Some(String::new())),
        target_lang: req
            .target_lang
            .or_else(|| Some(project.target_lang.clone())),
        source,
        target,
        note: req.note.filter(|note| !note.trim().is_empty()),
        kind: kind.to_string(),
        origin: "manual".to_string(),
        revision: 1,
        status: normalize_status(req.status.as_deref().unwrap_or_default()).to_string(),
    };

    // Optimistic path: the UI sends the revision it loaded, so a concurrent
    // summarizer write surfaces as an error instead of being overwritten.
    if is_update {
        if let Some(expected_revision) = req.expected_revision {
            if !repo::update_glossary_term_checked(&state.pool, &term, expected_revision).await? {
                return Err(AppError::Invalid(format!(
                    "glossary term {} changed since it was loaded; reload the glossary",
                    term.id
                )));
            }
            return repo::get_glossary_term(&state.pool, &term.id)
                .await?
                .ok_or_else(|| AppError::NotFound(format!("glossary term {}", term.id)));
        }
    }

    repo::upsert_glossary_term(&state.pool, &term).await?;
    Ok(term)
}

#[tauri::command]
pub async fn glossary_delete(state: State<'_, AppState>, id: String) -> Result<Ack> {
    repo::delete_glossary_term(&state.pool, &id).await?;
    Ok(Ack::done())
}

#[cfg(test)]
mod tests {
    use super::{normalize_kind, normalize_status};

    #[test]
    fn kinds_and_statuses_are_normalized_and_bounded() {
        assert_eq!(normalize_kind("term"), "term");
        assert_eq!(normalize_kind(" Proper_Noun "), "proper_noun");
        assert_eq!(normalize_kind("do_not_translate"), "do_not_translate");
        // Anything unexpected falls back to the safe default.
        assert_eq!(normalize_kind("nonsense"), "term");
        assert_eq!(normalize_kind(""), "term");

        assert_eq!(normalize_status("candidate"), "candidate");
        assert_eq!(normalize_status("REJECTED"), "rejected");
        assert_eq!(normalize_status(""), "approved");
        assert_eq!(normalize_status("nonsense"), "approved");
    }
}
