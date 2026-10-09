//! Glossary proposals and the merge rule (PLAN.md §10).
//!
//! Sub-agents propose terms; they never overwrite a rendering. A conflicting
//! proposal leaves the existing row in place (candidate/conflict rows are marked
//! `conflict`, approved rows stay approved) and records both renderings in one
//! open `qa_finding(kind='glossary_conflict')`, deduplicated per source term.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;

use crate::db::models::{GlossaryTerm, Project, QaFinding, SeriesGlossaryTerm};
use crate::db::{new_id, now, repo};
use crate::error::{AppError, Result};
use crate::util::sha256_hex_str;

/// A term an agent proposed, after clamping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedTerm {
    pub source: String,
    pub target: String,
    pub kind: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// What happened to a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ProposalOutcome {
    /// A new candidate row was inserted.
    Added,
    /// The same rendering already existed (or the term was rejected).
    Unchanged,
    /// A different rendering exists; the row and a finding record the conflict.
    Conflict,
}

/// Record an agent proposal without ever overwriting a different rendering.
pub async fn record_proposal(
    pool: &SqlitePool,
    project: &Project,
    proposed: &ProposedTerm,
    origin: &str,
) -> Result<ProposalOutcome> {
    let Some(existing) =
        repo::get_glossary_term_by_source(pool, &project.id, &proposed.source).await?
    else {
        repo::upsert_glossary_term(
            pool,
            &GlossaryTerm {
                id: new_id(),
                project_id: project.id.clone(),
                source_lang: Some(project.source_lang.clone().unwrap_or_default()),
                target_lang: Some(project.target_lang.clone()),
                source: proposed.source.clone(),
                target: proposed.target.clone(),
                note: proposed.note.clone(),
                kind: proposed.kind.clone(),
                origin: origin.to_string(),
                revision: 1,
                status: "candidate".to_string(),
            },
        )
        .await?;
        return Ok(ProposalOutcome::Added);
    };

    // The same rendering, or a term the user already rejected: nothing to do.
    if existing.target.eq_ignore_ascii_case(&proposed.target) || existing.status == "rejected" {
        return Ok(ProposalOutcome::Unchanged);
    }

    // Keep the existing rendering; only a not-yet-approved row can be marked as
    // conflicted, an approved one stays authoritative.
    if existing.status != "approved" {
        repo::set_glossary_status(pool, &existing.id, "conflict").await?;
    }
    if !repo::has_open_glossary_conflict(pool, &project.id, &proposed.source).await? {
        repo::insert_qa_finding(
            pool,
            &QaFinding {
                id: new_id(),
                project_id: project.id.clone(),
                chunk_id: None,
                block_id: None,
                kind: "glossary_conflict".to_string(),
                severity: "minor".to_string(),
                details_json: json!({
                    "source": proposed.source,
                    "existing_target": existing.target,
                    "proposed_target": proposed.target,
                    "origin": origin,
                })
                .to_string(),
                status: "open".to_string(),
                created_at: now(),
            },
        )
        .await?;
    }
    Ok(ProposalOutcome::Conflict)
}

// ---------------------------------------------------------------------------
// Effective glossary: project terms override the series canon (PLAN.md §9.5)
// ---------------------------------------------------------------------------

/// Where an effective term comes from. A project term always wins over a series term
/// with the same source, so a book can deviate from the canon without editing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlossaryScope {
    Project,
    Series,
}

/// One entry of the glossary the translator prompt and the QA check actually see.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedTerm {
    pub source: String,
    pub target: String,
    pub kind: String,
    /// Surface forms that should trigger the term (series terms only), excluding the
    /// canonical source itself.
    pub aliases: Vec<String>,
    pub scope: GlossaryScope,
}

/// Resolve the effective glossary of a project: its own approved terms, then the
/// approved series terms whose source is not overridden by a project term
/// (case-insensitive).
///
/// Only `approved` rows count. Candidates and conflicts are proposals by a model that
/// may be wrong — the summarizer used to echo the target as the source, telling the
/// translator to keep words untranslated — so they wait for the user. A project term
/// that is not approved does not block the series term either: rejecting means "not
/// for this book", not "nothing here".
pub async fn effective_terms(pool: &SqlitePool, project_id: &str) -> Result<Vec<ResolvedTerm>> {
    let project = repo::get_project(pool, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {project_id}")))?;
    let mut terms: Vec<ResolvedTerm> = Vec::new();
    let mut overridden: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for term in repo::list_glossary_terms(pool, project_id).await? {
        if term.status != "approved" {
            continue;
        }
        let key = term.source.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        // Case-variant duplicates are possible (the unique key is case-sensitive). Only
        // the first one reaches the prompt, so the effective glossary is deterministic
        // and the translator never sees two renderings of the same term.
        if !overridden.insert(key) {
            continue;
        }
        terms.push(ResolvedTerm {
            source: term.source,
            target: term.target,
            kind: term.kind,
            aliases: Vec::new(),
            scope: GlossaryScope::Project,
        });
    }

    let Some(series_id) = project.series_id.as_deref() else {
        return Ok(terms);
    };
    let mut variants: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for variant in repo::list_variants_for_series(pool, series_id).await? {
        variants
            .entry(variant.term_id)
            .or_default()
            .push(variant.text);
    }
    for term in repo::list_series_terms(pool, series_id).await? {
        if term.status != "approved" {
            continue;
        }
        let key = term.source.trim().to_lowercase();
        if key.is_empty() || overridden.contains(&key) {
            continue;
        }
        overridden.insert(key);
        let aliases = variants.remove(&term.id).unwrap_or_default();
        terms.push(ResolvedTerm {
            source: term.source,
            target: term.target,
            kind: term.kind,
            aliases,
            scope: GlossaryScope::Series,
        });
    }
    Ok(terms)
}

/// Deterministic identity of an effective glossary, for the translation memory.
/// Order independent: the terms are sorted by their lowercased source first.
pub fn effective_glossary_hash(terms: &[ResolvedTerm]) -> String {
    let mut lines: Vec<String> = terms
        .iter()
        .map(|term| {
            let mut aliases: Vec<String> = term
                .aliases
                .iter()
                .map(|alias| alias.trim().to_lowercase())
                .filter(|alias| !alias.is_empty())
                .collect();
            aliases.sort();
            aliases.dedup();
            format!(
                "{}\u{0}{}\u{0}{}\u{0}{}",
                term.source.trim().to_lowercase(),
                term.target,
                term.kind,
                aliases.join("\u{1}")
            )
        })
        .collect();
    lines.sort();
    sha256_hex_str(&lines.join("\n"))
}

/// The project memory value followed by the series one, so the book's own guidance is
/// paid first and the series fills the gaps when the piece is later truncated.
pub async fn memory_with_series(pool: &SqlitePool, project: &Project, key: &str) -> Result<String> {
    let book = repo::get_memory(pool, &project.id, key)
        .await?
        .unwrap_or_default();
    let Some(series_id) = project.series_id.as_deref() else {
        return Ok(book);
    };
    let series = repo::get_series_memory(pool, series_id, key)
        .await?
        .unwrap_or_default();
    Ok(match (book.trim().is_empty(), series.trim().is_empty()) {
        (true, _) => series,
        (false, true) => book,
        (false, false) => format!("{book}\n\nSERIES {}\n{series}", key.to_uppercase()),
    })
}

/// Open a `glossary_conflict` finding for every member book whose own glossary renders
/// `source` differently from the series rendering. Deduplicated per book and source, so
/// re-running an edit does not pile up findings. Returns how many findings were created.
pub async fn flag_series_conflicts(
    pool: &SqlitePool,
    series_id: &str,
    source: &str,
    series_target: &str,
) -> Result<usize> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT g.project_id, g.target FROM glossary_term g \
         JOIN project p ON p.id = g.project_id \
         WHERE p.series_id = ?1 AND lower(g.source) = lower(?2) AND g.status != 'rejected'",
    )
    .bind(series_id)
    .bind(source)
    .fetch_all(pool)
    .await?;

    let mut created = 0;
    for (project_id, project_target) in rows {
        if project_target.eq_ignore_ascii_case(series_target) {
            continue;
        }
        if repo::has_open_glossary_conflict(pool, &project_id, source).await? {
            continue;
        }
        repo::insert_qa_finding(
            pool,
            &QaFinding {
                id: new_id(),
                project_id,
                chunk_id: None,
                block_id: None,
                kind: "glossary_conflict".to_string(),
                severity: "minor".to_string(),
                details_json: json!({
                    "source": source,
                    "scope": "series",
                    "series_target": series_target,
                    "project_target": project_target,
                })
                .to_string(),
                status: "open".to_string(),
                created_at: now(),
            },
        )
        .await?;
        created += 1;
    }
    Ok(created)
}

/// Copy a project term into its series. The series row is never overwritten by a
/// differing rendering: the existing one is kept (and marked `conflict` unless it is
/// already approved) and a finding records the disagreement.
pub async fn promote_term(
    pool: &SqlitePool,
    project_id: &str,
    term_id: &str,
) -> Result<ProposalOutcome> {
    let project = repo::get_project(pool, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {project_id}")))?;
    let series_id = project.series_id.clone().ok_or_else(|| {
        AppError::Invalid(format!("project {project_id} does not belong to a series"))
    })?;
    let term = repo::get_glossary_term(pool, term_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("glossary term {term_id}")))?;
    if term.project_id != project_id {
        return Err(AppError::Invalid(format!(
            "glossary term {term_id} does not belong to project {project_id}"
        )));
    }

    let existing = repo::get_series_term_by_source(pool, &series_id, &term.source).await?;
    let Some(existing) = existing else {
        repo::upsert_series_term(
            pool,
            &SeriesGlossaryTerm {
                id: new_id(),
                series_id: series_id.clone(),
                source_lang: term.source_lang.clone(),
                target_lang: term.target_lang.clone(),
                source: term.source.clone(),
                target: term.target.clone(),
                note: term.note.clone(),
                kind: term.kind.clone(),
                origin: "promoted".to_string(),
                revision: 1,
                status: term.status.clone(),
            },
        )
        .await?;
        return Ok(ProposalOutcome::Added);
    };

    if existing.target.eq_ignore_ascii_case(&term.target) || existing.status == "rejected" {
        return Ok(ProposalOutcome::Unchanged);
    }
    if existing.status != "approved" {
        repo::set_series_term_status(pool, &existing.id, "conflict").await?;
    }
    if !repo::has_open_glossary_conflict(pool, project_id, &term.source).await? {
        repo::insert_qa_finding(
            pool,
            &QaFinding {
                id: new_id(),
                project_id: project_id.to_string(),
                chunk_id: None,
                block_id: None,
                kind: "glossary_conflict".to_string(),
                severity: "minor".to_string(),
                details_json: json!({
                    "source": term.source,
                    "scope": "series_promote",
                    "series_target": existing.target,
                    "project_target": term.target,
                })
                .to_string(),
                status: "open".to_string(),
                created_at: now(),
            },
        )
        .await?;
    }
    Ok(ProposalOutcome::Conflict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::Project;

    async fn seed(pool: &sqlx::SqlitePool, source_lang: Option<&str>) -> Project {
        let timestamp = now();
        let project = Project {
            id: "p1".to_string(),
            name: "p".to_string(),
            source_path: "/x".to_string(),
            source_hash: "h".to_string(),
            source_format: "epub".to_string(),
            source_lang: source_lang.map(str::to_string),
            target_lang: "it".to_string(),
            doc_title: None,
            doc_author: None,
            series_id: None,
            series_order: None,
            prompts_snapshot_dir: None,
            settings_json: "{}".to_string(),
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        repo::insert_project(pool, &project).await.expect("project");
        project
    }

    fn term(source: &str, target: &str) -> ProposedTerm {
        ProposedTerm {
            source: source.to_string(),
            target: target.to_string(),
            kind: "term".to_string(),
            note: None,
        }
    }

    #[tokio::test]
    async fn a_new_proposal_becomes_a_candidate() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;

        let outcome = record_proposal(&pool, &project, &term("keeper", "guardiano"), "proposed")
            .await
            .expect("proposal");
        assert_eq!(outcome, ProposalOutcome::Added);

        let terms = repo::list_glossary_terms(&pool, &project.id)
            .await
            .expect("terms");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].status, "candidate");
        assert_eq!(terms[0].origin, "proposed");
    }

    #[tokio::test]
    async fn the_same_rendering_is_unchanged_and_repeated() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        record_proposal(&pool, &project, &term("keeper", "guardiano"), "proposed")
            .await
            .expect("first");

        let outcome = record_proposal(&pool, &project, &term("keeper", "Guardiano"), "proposed")
            .await
            .expect("second");
        assert_eq!(outcome, ProposalOutcome::Unchanged);
        let terms = repo::list_glossary_terms(&pool, &project.id)
            .await
            .expect("terms");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].revision, 1, "no write happened");
    }

    #[tokio::test]
    async fn a_conflicting_proposal_marks_the_candidate_and_records_one_finding() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        record_proposal(&pool, &project, &term("keeper", "guardiano"), "proposed")
            .await
            .expect("first");

        let outcome = record_proposal(&pool, &project, &term("keeper", "custode"), "proposed")
            .await
            .expect("conflict");
        assert_eq!(outcome, ProposalOutcome::Conflict);

        let terms = repo::list_glossary_terms(&pool, &project.id)
            .await
            .expect("terms");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].status, "conflict", "the row is flagged");
        assert_eq!(
            terms[0].target, "guardiano",
            "the existing rendering is never overwritten"
        );

        let findings = repo::list_qa_findings(&pool, &project.id)
            .await
            .expect("findings");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "glossary_conflict");
        let details: serde_json::Value =
            serde_json::from_str(&findings[0].details_json).expect("details");
        assert_eq!(details["existing_target"], "guardiano");
        assert_eq!(details["proposed_target"], "custode");

        // A third proposal for the same source does not pile up findings.
        record_proposal(
            &pool,
            &project,
            &term("keeper", "guardiano capo"),
            "proposed",
        )
        .await
        .expect("third");
        let findings = repo::list_qa_findings(&pool, &project.id)
            .await
            .expect("findings");
        assert_eq!(findings.len(), 1, "one open conflict per source term");
    }

    #[tokio::test]
    async fn an_approved_term_is_never_demoted_by_a_proposal() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        repo::upsert_glossary_term(
            &pool,
            &GlossaryTerm {
                id: new_id(),
                project_id: project.id.clone(),
                source_lang: Some("en".to_string()),
                target_lang: Some("it".to_string()),
                source: "keeper".to_string(),
                target: "custode".to_string(),
                note: None,
                kind: "term".to_string(),
                origin: "manual".to_string(),
                revision: 1,
                status: "approved".to_string(),
            },
        )
        .await
        .expect("approved");

        let outcome = record_proposal(&pool, &project, &term("keeper", "guardiano"), "proposed")
            .await
            .expect("conflict");
        assert_eq!(outcome, ProposalOutcome::Conflict);

        let terms = repo::list_glossary_terms(&pool, &project.id)
            .await
            .expect("terms");
        assert_eq!(terms[0].status, "approved");
        assert_eq!(terms[0].target, "custode");
        let findings = repo::list_qa_findings(&pool, &project.id)
            .await
            .expect("findings");
        assert_eq!(findings.len(), 1);
    }

    #[tokio::test]
    async fn a_rejected_term_is_not_resurrected() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        repo::upsert_glossary_term(
            &pool,
            &GlossaryTerm {
                id: new_id(),
                project_id: project.id.clone(),
                source_lang: Some("en".to_string()),
                target_lang: Some("it".to_string()),
                source: "keeper".to_string(),
                target: "guardiano".to_string(),
                note: None,
                kind: "term".to_string(),
                origin: "manual".to_string(),
                revision: 1,
                status: "rejected".to_string(),
            },
        )
        .await
        .expect("rejected");

        let outcome = record_proposal(&pool, &project, &term("keeper", "custode"), "proposed")
            .await
            .expect("proposal");
        assert_eq!(outcome, ProposalOutcome::Unchanged);
        assert!(repo::list_qa_findings(&pool, &project.id)
            .await
            .expect("findings")
            .is_empty());
    }

    // -----------------------------------------------------------------------
    // Effective glossary: series + project overrides (PLAN.md §9.5)
    // -----------------------------------------------------------------------

    async fn assign_to_series(pool: &sqlx::SqlitePool, project_id: &str) {
        let timestamp = now();
        repo::upsert_series(
            pool,
            &crate::db::models::Series {
                id: "s1".to_string(),
                name: "The Saga".to_string(),
                source_lang: Some("en".to_string()),
                target_lang: Some("it".to_string()),
                settings_json: "{}".to_string(),
                created_at: timestamp.clone(),
                updated_at: timestamp,
            },
        )
        .await
        .expect("series");
        repo::set_project_series(pool, project_id, Some("s1"), Some(1))
            .await
            .expect("assign");
    }

    async fn add_series_term(
        pool: &sqlx::SqlitePool,
        id: &str,
        source: &str,
        target: &str,
        status: &str,
    ) {
        repo::upsert_series_term(
            pool,
            &SeriesGlossaryTerm {
                id: id.to_string(),
                series_id: "s1".to_string(),
                source_lang: Some("en".to_string()),
                target_lang: Some("it".to_string()),
                source: source.to_string(),
                target: target.to_string(),
                note: None,
                kind: "term".to_string(),
                origin: "manual".to_string(),
                revision: 1,
                status: status.to_string(),
            },
        )
        .await
        .expect("series term");
    }

    async fn add_project_term(
        pool: &sqlx::SqlitePool,
        project_id: &str,
        source: &str,
        target: &str,
        status: &str,
    ) -> String {
        let id = new_id();
        repo::upsert_glossary_term(
            pool,
            &GlossaryTerm {
                id: id.clone(),
                project_id: project_id.to_string(),
                source_lang: Some("en".to_string()),
                target_lang: Some("it".to_string()),
                source: source.to_string(),
                target: target.to_string(),
                note: None,
                kind: "term".to_string(),
                origin: "manual".to_string(),
                revision: 1,
                status: status.to_string(),
            },
        )
        .await
        .expect("project term");
        id
    }

    #[tokio::test]
    async fn two_books_of_a_series_share_the_canon_term() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let first = seed(&pool, Some("en")).await;
        assign_to_series(&pool, &first.id).await;
        // A second book with its own id, in the same series and language pair.
        let timestamp = now();
        sqlx::query(
            "INSERT INTO project (id, name, source_path, source_hash, source_format, \
             source_lang, target_lang, series_id, series_order, settings_json, created_at, updated_at) \
             VALUES ('p2','second','/x','h','epub','en','it','s1',2,'{}',?1,?1)",
        )
        .bind(&timestamp)
        .execute(&pool)
        .await
        .expect("second project");
        add_series_term(&pool, "st1", "keeper", "custode", "approved").await;

        for project_id in [&first.id, "p2"] {
            let terms = effective_terms(&pool, project_id).await.expect("terms");
            let keeper = terms
                .iter()
                .find(|term| term.source == "keeper")
                .unwrap_or_else(|| panic!("{project_id} does not see the series term"));
            assert_eq!(keeper.target, "custode");
            assert_eq!(keeper.scope, GlossaryScope::Series);
        }
    }

    #[tokio::test]
    async fn a_project_term_overrides_the_series_rendering() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        assign_to_series(&pool, &project.id).await;
        add_series_term(&pool, "st1", "keeper", "custode", "approved").await;
        add_series_term(&pool, "st2", "ship", "nave", "approved").await;
        add_project_term(&pool, &project.id, "keeper", "guardiano", "approved").await;

        let terms = effective_terms(&pool, &project.id).await.expect("terms");
        assert_eq!(
            terms.len(),
            2,
            "the series term is overridden, not duplicated"
        );
        let keeper = terms
            .iter()
            .find(|term| term.source == "keeper")
            .expect("keeper");
        assert_eq!(keeper.target, "guardiano");
        assert_eq!(keeper.scope, GlossaryScope::Project);
        let ship = terms
            .iter()
            .find(|term| term.source == "ship")
            .expect("ship");
        assert_eq!(ship.scope, GlossaryScope::Series);
    }

    #[tokio::test]
    async fn a_rejected_project_term_does_not_block_the_series_term() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        assign_to_series(&pool, &project.id).await;
        add_series_term(&pool, "st1", "keeper", "custode", "approved").await;
        add_project_term(&pool, &project.id, "keeper", "guardiano", "rejected").await;

        let terms = effective_terms(&pool, &project.id).await.expect("terms");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].target, "custode");
        assert_eq!(terms[0].scope, GlossaryScope::Series);
    }

    #[tokio::test]
    async fn only_approved_terms_reach_the_effective_glossary() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        assign_to_series(&pool, &project.id).await;
        add_project_term(&pool, &project.id, "Figés", "Figés", "candidate").await;
        add_project_term(&pool, &project.id, "Sentinelles", "Watchers", "conflict").await;
        add_project_term(&pool, &project.id, "Spires", "Spires", "approved").await;
        add_series_term(&pool, "st1", "keeper", "custode", "candidate").await;
        add_series_term(&pool, "st2", "ship", "nave", "approved").await;
        // A project candidate does not hide the approved series rendering.
        add_project_term(&pool, &project.id, "ship", "vascello", "candidate").await;

        let terms = effective_terms(&pool, &project.id).await.expect("terms");
        let mut seen: Vec<(&str, &str)> = terms
            .iter()
            .map(|term| (term.source.as_str(), term.target.as_str()))
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, [("Spires", "Spires"), ("ship", "nave")]);
    }

    #[tokio::test]
    async fn case_variant_project_terms_collapse_to_one_entry() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        // The unique key is case-sensitive, so both rows can exist.
        add_project_term(&pool, &project.id, "Keeper", "custode", "approved").await;
        add_project_term(&pool, &project.id, "keeper", "guardiano", "approved").await;

        let terms = effective_terms(&pool, &project.id).await.expect("terms");
        assert_eq!(terms.len(), 1, "one entry per case-folded source");
        // `list_glossary_terms` orders by source, so the first (`Keeper`) wins.
        assert_eq!(terms[0].source, "Keeper");
        assert_eq!(terms[0].target, "custode");
    }

    #[tokio::test]
    async fn series_aliases_reach_the_resolved_terms() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        assign_to_series(&pool, &project.id).await;
        add_series_term(&pool, "st1", "Keeper", "Custode", "approved").await;
        repo::upsert_series_variant(
            &pool,
            &crate::db::models::SeriesGlossaryVariant {
                id: new_id(),
                term_id: "st1".to_string(),
                text: "the Keeper".to_string(),
            },
        )
        .await
        .expect("variant");

        let terms = effective_terms(&pool, &project.id).await.expect("terms");
        assert_eq!(terms[0].aliases, vec!["the Keeper".to_string()]);
    }

    #[tokio::test]
    async fn the_effective_glossary_hash_is_order_independent_and_tracks_content() {
        let term = |source: &str, target: &str| ResolvedTerm {
            source: source.to_string(),
            target: target.to_string(),
            kind: "term".to_string(),
            aliases: Vec::new(),
            scope: GlossaryScope::Series,
        };
        let mut first = vec![term("keeper", "custode"), term("ship", "nave")];
        let mut second = first.clone();
        second.reverse();
        assert_eq!(
            effective_glossary_hash(&first),
            effective_glossary_hash(&second),
            "the hash must not depend on the query order"
        );

        first[0].target = "guardiano".to_string();
        assert_ne!(
            effective_glossary_hash(&first),
            effective_glossary_hash(&second),
            "a changed rendering is a different glossary"
        );

        let mut with_alias = second.clone();
        with_alias[0].aliases = vec!["the Keeper".to_string()];
        assert_ne!(
            effective_glossary_hash(&with_alias),
            effective_glossary_hash(&second)
        );
    }

    #[tokio::test]
    async fn a_series_change_flags_every_book_with_a_different_rendering() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let first = seed(&pool, Some("en")).await;
        assign_to_series(&pool, &first.id).await;
        // A second book of the same series.
        let timestamp = now();
        sqlx::query(
            "INSERT INTO project (id, name, source_path, source_hash, source_format, \
             source_lang, target_lang, series_id, series_order, settings_json, created_at, updated_at) \
             VALUES ('p2','second','/x','h','epub','en','it','s1',2,'{}',?1,?1)",
        )
        .bind(&timestamp)
        .execute(&pool)
        .await
        .expect("second project");

        add_project_term(&pool, &first.id, "keeper", "guardiano", "approved").await;
        add_project_term(&pool, "p2", "keeper", "custode", "approved").await;

        let created = flag_series_conflicts(&pool, "s1", "keeper", "custode")
            .await
            .expect("conflicts");
        assert_eq!(
            created, 1,
            "only the book with a different rendering is flagged"
        );
        let findings = repo::list_qa_findings(&pool, &first.id)
            .await
            .expect("findings");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "glossary_conflict");
        assert!(findings[0].details_json.contains("guardiano"));
        assert!(repo::list_qa_findings(&pool, "p2")
            .await
            .expect("findings")
            .is_empty());

        // Re-running the same change does not pile up findings.
        assert_eq!(
            flag_series_conflicts(&pool, "s1", "keeper", "custode")
                .await
                .expect("again"),
            0
        );
    }

    #[tokio::test]
    async fn promoting_never_overwrites_a_different_series_rendering() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let project = seed(&pool, Some("en")).await;
        assign_to_series(&pool, &project.id).await;
        add_series_term(&pool, "st1", "keeper", "custode", "approved").await;
        let term_id = add_project_term(&pool, &project.id, "keeper", "guardiano", "approved").await;

        let outcome = promote_term(&pool, &project.id, &term_id)
            .await
            .expect("promote");
        assert_eq!(outcome, ProposalOutcome::Conflict);
        let series = repo::list_series_terms(&pool, "s1").await.expect("terms");
        assert_eq!(
            series[0].target, "custode",
            "the canon is never overwritten"
        );
        assert_eq!(series[0].status, "approved");
        let findings = repo::list_qa_findings(&pool, &project.id)
            .await
            .expect("findings");
        assert_eq!(findings.len(), 1);
        assert!(findings[0].details_json.contains("series_promote"));

        // A promotion with the same rendering is a no-op and adds no term.
        let same = add_project_term(&pool, &project.id, "ship", "nave", "approved").await;
        add_series_term(&pool, "st2", "ship", "nave", "approved").await;
        assert_eq!(
            promote_term(&pool, &project.id, &same).await.expect("same"),
            ProposalOutcome::Unchanged
        );
        assert_eq!(
            repo::list_series_terms(&pool, "s1")
                .await
                .expect("terms")
                .len(),
            2
        );
    }
}
