//! Row types for every table in the schema, plus the IR types exchanged with
//! the Python sidecar.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub source_path: String,
    pub source_hash: String,
    pub source_format: String,
    pub source_lang: Option<String>,
    pub target_lang: String,
    pub doc_title: Option<String>,
    pub doc_author: Option<String>,
    /// Series the book belongs to (PLAN.md §9.5); `None` for a standalone book.
    pub series_id: Option<String>,
    /// Position inside the series, for ordering the books in the UI and the exports.
    pub series_order: Option<i64>,
    pub prompts_snapshot_dir: Option<String>,
    pub settings_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Document {
    pub id: String,
    pub project_id: String,
    pub markdown_path: String,
    pub front_matter_json: String,
    pub extractor: String,
    pub extractor_version: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Chapter {
    pub id: String,
    pub document_id: String,
    pub order_index: i64,
    pub title: String,
    pub level: i64,
    pub block_first: i64,
    pub block_last: i64,
    pub summary: Option<String>,
    pub summary_model: Option<String>,
    pub summary_hash: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Block {
    pub id: String,
    pub document_id: String,
    pub chapter_id: Option<String>,
    pub order_index: i64,
    pub kind: String,
    pub level: i64,
    pub source_md: String,
    pub source_text: String,
    pub translatable: bool,
    pub attrs_json: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Chunk {
    pub id: String,
    pub document_id: String,
    pub chapter_id: Option<String>,
    pub order_index: i64,
    pub block_ids_json: String,
    pub source_md: String,
    pub token_estimate: i64,
    pub context_json: String,
    pub flags_json: String,
    pub status: String,
    pub prompt_hash: Option<String>,
    pub model_id: Option<String>,
    pub params_json: Option<String>,
    pub context_manifest_json: Option<String>,
    pub target_md: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct BlockTranslation {
    pub block_id: String,
    pub chunk_id: String,
    pub text_md: String,
    pub placeholders_ok: bool,
    pub origin: String,
    pub edited_by_user: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Suggestion {
    pub id: String,
    pub chunk_id: String,
    pub pass: String,
    pub block_id: Option<String>,
    pub field: Option<String>,
    pub original: Option<String>,
    pub proposed: Option<String>,
    pub reason: Option<String>,
    pub severity: Option<String>,
    pub quote: Option<String>,
    pub status: String,
    pub created_at: String,
    pub decided_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct QaFinding {
    pub id: String,
    pub project_id: String,
    pub chunk_id: Option<String>,
    pub block_id: Option<String>,
    pub kind: String,
    pub severity: String,
    pub details_json: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct GlossaryTerm {
    pub id: String,
    pub project_id: String,
    pub source_lang: Option<String>,
    pub target_lang: Option<String>,
    pub source: String,
    pub target: String,
    pub note: Option<String>,
    pub kind: String,
    pub origin: String,
    pub revision: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProjectMemory {
    pub project_id: String,
    pub key: String,
    pub value: String,
    pub revision: i64,
    pub updated_at: String,
}

/// A book series: the shared canon a project inherits (PLAN.md §9.5).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Series {
    pub id: String,
    pub name: String,
    /// Pinned language pair every member book shares.
    pub source_lang: Option<String>,
    pub target_lang: Option<String>,
    pub settings_json: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A series glossary term. Same shape as [`GlossaryTerm`], scoped to a series.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SeriesGlossaryTerm {
    pub id: String,
    pub series_id: String,
    pub source_lang: Option<String>,
    pub target_lang: Option<String>,
    pub source: String,
    pub target: String,
    pub note: Option<String>,
    pub kind: String,
    pub origin: String,
    pub revision: i64,
    pub status: String,
}

/// A surface form of a series term (`the Keeper`, `Keeper's`).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SeriesGlossaryVariant {
    pub id: String,
    pub term_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SeriesMemory {
    pub series_id: String,
    pub key: String,
    pub value: String,
    pub revision: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct LlmEndpoint {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key_ref: Option<String>,
    pub max_concurrency: Option<i64>,
    pub notes: Option<String>,
    pub last_health_at: Option<String>,
    pub last_health_ok: Option<bool>,
    pub props_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RoleBinding {
    pub id: String,
    pub endpoint_id: String,
    pub role: String,
    pub model: String,
    pub params_json: String,
    pub priority: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Job {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub payload_json: String,
    pub priority: i64,
    pub state: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
    pub run_after: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct LlmCall {
    pub id: String,
    pub job_id: Option<String>,
    pub chunk_id: Option<String>,
    pub role: String,
    pub endpoint_id: Option<String>,
    pub model: String,
    pub params_json: String,
    pub seed: Option<i64>,
    pub prompt_hash: String,
    pub prompt_text: Option<String>,
    pub prompt_compressed: Option<bool>,
    pub response_text: Option<String>,
    /// The thinking of a reasoning model, as streamed in `reasoning_content`: kept for
    /// audit only, never parsed as the answer.
    pub reasoning_text: Option<String>,
    pub finish_reason: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub latency_ms: Option<i64>,
    pub attempt: i64,
    pub error: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TranslationCacheRow {
    pub prompt_hash: String,
    pub model: String,
    pub params_hash: String,
    pub target_lang: String,
    pub response_text: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TranslationMemoryRow {
    pub content_hash: String,
    pub model: String,
    pub target_lang: String,
    /// Hash of the effective glossary the row was produced with.
    pub glossary_hash: String,
    pub text_md: String,
    pub hits: i64,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Intermediate representation exchanged with the Python sidecar (PLAN.md §12.1).
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}

fn default_order() -> i64 {
    0
}

/// `Block` as produced by the sidecar (`order` + `attrs`, not the DB columns).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockIr {
    pub id: String,
    #[serde(default)]
    pub chapter_id: Option<String>,
    #[serde(default = "default_order")]
    pub order: i64,
    pub kind: String,
    #[serde(default)]
    pub level: i64,
    #[serde(default)]
    pub source_md: String,
    #[serde(default)]
    pub source_text: String,
    #[serde(default = "default_true")]
    pub translatable: bool,
    #[serde(default)]
    pub attrs: serde_json::Value,
    #[serde(default)]
    pub content_hash: String,
}

/// `Chapter` as produced by the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterIr {
    pub id: String,
    #[serde(default = "default_order")]
    pub order: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub level: i64,
    #[serde(default)]
    pub block_first: i64,
    #[serde(default)]
    pub block_last: i64,
}

/// `Chunk` as produced by the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkIr {
    pub id: String,
    #[serde(default)]
    pub chapter_id: Option<String>,
    #[serde(default = "default_order")]
    pub order: i64,
    #[serde(default)]
    pub block_ids: Vec<String>,
    #[serde(default)]
    pub source_md: String,
    #[serde(default)]
    pub token_estimate: i64,
    #[serde(default)]
    pub context_carrier: serde_json::Value,
    #[serde(default)]
    pub flags: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_ir_defaults_are_lenient() {
        let json = r#"{"id":"b000001","kind":"para","source_md":"hi"}"#;
        let block: BlockIr = serde_json::from_str(json).expect("parse");
        assert_eq!(block.id, "b000001");
        assert!(block.translatable);
        assert_eq!(block.order, 0);
    }
}
