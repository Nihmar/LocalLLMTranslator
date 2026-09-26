//! Glossary proposals and the merge rule (PLAN.md §10).
//!
//! Sub-agents propose terms; they never overwrite a rendering. A conflicting
//! proposal leaves the existing row in place (candidate/conflict rows are marked
//! `conflict`, approved rows stay approved) and records both renderings in one
//! open `qa_finding(kind='glossary_conflict')`, deduplicated per source term.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;

use crate::db::models::{GlossaryTerm, Project, QaFinding};
use crate::db::{new_id, now, repo};
use crate::error::Result;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}
