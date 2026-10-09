//! Command-boundary views: row types with their `*_json` columns decoded once, so the UI
//! never parses a column (issue #9). The thin `From<Row>` conversions keep the schema out
//! of the views: a storage column may be renamed without touching the IPC contract, and the
//! raw columns stay where they belong (the database and the pipeline).

use serde::Serialize;

use crate::db::models::{Chunk, Job, QaFinding};

/// A chunk as `chunk_list` / `chunk_get` return it: `block_ids` and `flags` are arrays.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ChunkView {
    #[serde(flatten)]
    #[ts(flatten)]
    pub chunk: Chunk,
    /// Decoded `block_ids_json`.
    pub block_ids: Vec<String>,
    /// Decoded `flags_json`.
    pub flags: Vec<String>,
}

impl From<Chunk> for ChunkView {
    fn from(chunk: Chunk) -> Self {
        Self {
            block_ids: serde_json::from_str(&chunk.block_ids_json).unwrap_or_default(),
            flags: serde_json::from_str(&chunk.flags_json).unwrap_or_default(),
            chunk,
        }
    }
}

/// A job as `job_list` returns it and `job://progress` emits it: `payload` is decoded.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct JobView {
    #[serde(flatten)]
    #[ts(flatten)]
    pub job: Job,
    /// Decoded `payload_json`.
    pub payload: serde_json::Value,
}

impl From<Job> for JobView {
    fn from(job: Job) -> Self {
        let payload = serde_json::from_str(&job.payload_json).unwrap_or(serde_json::Value::Null);
        Self { job, payload }
    }
}

/// A QA finding as `qa_report` returns it: `details` is decoded.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct QaFindingView {
    #[serde(flatten)]
    #[ts(flatten)]
    pub finding: QaFinding,
    /// Decoded `details_json`.
    pub details: serde_json::Value,
}

impl From<QaFinding> for QaFindingView {
    fn from(finding: QaFinding) -> Self {
        let details =
            serde_json::from_str(&finding.details_json).unwrap_or(serde_json::Value::Null);
        Self { finding, details }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::now;

    fn chunk() -> Chunk {
        Chunk {
            id: "c1".into(),
            document_id: "d1".into(),
            chapter_id: Some("h1".into()),
            order_index: 3,
            block_ids_json: r#"["b1","b2"]"#.into(),
            source_md: "text".into(),
            token_estimate: 10,
            context_json: "{}".into(),
            flags_json: r#"["long"]"#.into(),
            status: "done".into(),
            prompt_hash: None,
            model_id: None,
            params_json: None,
            context_manifest_json: None,
            target_md: Some("translated".into()),
            error: None,
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn chunk_view_decodes_its_json_columns() {
        let view = ChunkView::from(chunk());
        assert_eq!(view.block_ids, vec!["b1".to_string(), "b2".to_string()]);
        assert_eq!(view.flags, vec!["long".to_string()]);
        assert_eq!(view.chunk.id, "c1");
        // Malformed columns degrade to empty, never fail the command.
        let mut broken = chunk();
        broken.block_ids_json = "not json".into();
        assert!(ChunkView::from(broken).block_ids.is_empty());
    }

    #[test]
    fn job_and_finding_views_decode_their_payloads() {
        let job = Job {
            id: "j1".into(),
            project_id: "p1".into(),
            kind: "translate_chunk".into(),
            payload_json: r#"{"chunk_id":"c1"}"#.into(),
            priority: 1,
            state: "pending".into(),
            attempts: 0,
            max_attempts: 3,
            lease_owner: None,
            lease_expires_at: None,
            run_after: None,
            last_error: None,
            created_at: now(),
            started_at: None,
            finished_at: None,
        };
        let view = JobView::from(job);
        assert_eq!(view.payload["chunk_id"], "c1");

        let finding = QaFinding {
            id: "f1".into(),
            project_id: "p1".into(),
            chunk_id: None,
            block_id: None,
            kind: "glossary_conflict".into(),
            severity: "major".into(),
            details_json: r#"{"source":"Gandalf"}"#.into(),
            status: "open".into(),
            created_at: now(),
        };
        assert_eq!(QaFindingView::from(finding).details["source"], "Gandalf");
    }
}
