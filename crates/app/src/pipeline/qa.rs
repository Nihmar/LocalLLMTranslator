//! QA scan (PLAN.md §11.4): run the sidecar's heuristics on a translated chunk
//! and persist the findings.
//!
//! The scan runs inline right after a chunk is translated — with the raw model
//! reply, so the placeholder check has real `⟦n⟧` tokens to inspect — and can be
//! re-run later through the `qa_scan` job when the glossary changed. Findings are
//! advisory: they never change a chunk's status, they only feed the report the
//! reviewer reads.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

use super::PipelineDeps;
use crate::db::models::{Chunk, QaFinding};
use crate::db::{new_id, now, repo};
use crate::error::{AppError, Result};
use crate::sidecar::SidecarFinding;

pub const JOB_KIND: &str = "qa_scan";

/// Payload of a `qa_scan` job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaScanPayload {
    pub chunk_id: String,
}

/// The glossary as the sidecar expects it: a `{source: target}` object of the
/// non-rejected terms.
pub async fn glossary_object(pool: &SqlitePool, project_id: &str) -> Result<Value> {
    let mut map = serde_json::Map::new();
    for term in repo::list_glossary_terms(pool, project_id).await? {
        if term.status == "rejected"
            || term.source.trim().is_empty()
            || term.target.trim().is_empty()
        {
            continue;
        }
        map.insert(term.source, Value::String(term.target));
    }
    Ok(Value::Object(map))
}

/// Scan the translation the pipeline just produced and validated. The
/// placeholder integrity is already enforced by `reinject` (a failure marks the
/// chunk `needs_review` instead), so the heuristics run on the placeholder-free
/// markdown and only the text checks apply.
pub async fn scan_translated_chunk(
    deps: &PipelineDeps,
    project_id: &str,
    chunk: &Chunk,
    target_text: &str,
) -> Result<usize> {
    let glossary = glossary_object(&deps.pool, project_id).await?;
    let result = deps
        .sidecar
        .qa_check(&chunk.source_md, target_text, &glossary, &[])
        .await?;
    persist(deps, project_id, &chunk.id, &result.findings).await
}

/// Re-scan a stored chunk using its validated `target_md`.
pub async fn scan_chunk(deps: &PipelineDeps, project_id: &str, chunk_id: &str) -> Result<usize> {
    let chunk = repo::get_chunk(&deps.pool, chunk_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("chunk {chunk_id}")))?;
    let Some(target) = chunk
        .target_md
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    else {
        repo::delete_qa_findings_for_chunk(&deps.pool, chunk_id).await?;
        return Ok(0);
    };
    let glossary = glossary_object(&deps.pool, project_id).await?;
    let result = deps
        .sidecar
        .qa_check(&chunk.source_md, target, &glossary, &[])
        .await?;
    persist(deps, project_id, chunk_id, &result.findings).await
}

/// Replace the chunk's findings with the new scan.
async fn persist(
    deps: &PipelineDeps,
    project_id: &str,
    chunk_id: &str,
    findings: &[SidecarFinding],
) -> Result<usize> {
    repo::delete_qa_findings_for_chunk(&deps.pool, chunk_id).await?;
    for finding in findings {
        repo::insert_qa_finding(
            &deps.pool,
            &QaFinding {
                id: new_id(),
                project_id: project_id.to_string(),
                chunk_id: Some(chunk_id.to_string()),
                block_id: finding.block_id.clone(),
                kind: finding.kind.clone(),
                severity: finding.severity.clone(),
                details_json: serde_json::to_string(&finding.details)?,
                status: "open".to_string(),
                created_at: now(),
            },
        )
        .await?;
    }
    Ok(findings.len())
}
