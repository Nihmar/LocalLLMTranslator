//! Repository functions. Every function takes an explicit `&SqlitePool` so the
//! layer stays testable with an in-memory or temp-file database.

use sqlx::SqlitePool;

use super::models::*;
use super::{new_id, now};
use crate::error::Result;

/// Outcome of executing a chunk, written as a single idempotent upsert keyed on
/// the chunk identity (`chunk_id` + `prompt_hash`).
#[derive(Debug, Clone)]
pub struct ChunkOutcome {
    pub chunk_id: String,
    pub status: String,
    pub prompt_hash: Option<String>,
    pub model_id: Option<String>,
    pub params_json: Option<String>,
    pub context_manifest_json: Option<String>,
    pub target_md: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// project
// ---------------------------------------------------------------------------

pub async fn insert_project(pool: &SqlitePool, p: &Project) -> Result<()> {
    sqlx::query(
        "INSERT INTO project (id, name, source_path, source_hash, source_format, source_lang, \
         target_lang, doc_title, doc_author, prompts_snapshot_dir, settings_json, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
    )
    .bind(p.id.as_str())
    .bind(p.name.as_str())
    .bind(p.source_path.as_str())
    .bind(p.source_hash.as_str())
    .bind(p.source_format.as_str())
    .bind(p.source_lang.as_deref())
    .bind(p.target_lang.as_str())
    .bind(p.doc_title.as_deref())
    .bind(p.doc_author.as_deref())
    .bind(p.prompts_snapshot_dir.as_deref())
    .bind(p.settings_json.as_str())
    .bind(p.created_at.as_str())
    .bind(p.updated_at.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_projects(pool: &SqlitePool) -> Result<Vec<Project>> {
    let rows = sqlx::query_as::<_, Project>("SELECT * FROM project ORDER BY created_at DESC")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn get_project(pool: &SqlitePool, id: &str) -> Result<Option<Project>> {
    let row = sqlx::query_as::<_, Project>("SELECT * FROM project WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn delete_project(pool: &SqlitePool, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM project WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn touch_project(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("UPDATE project SET updated_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(now())
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// llm_endpoint
// ---------------------------------------------------------------------------

pub async fn upsert_endpoint(pool: &SqlitePool, e: &LlmEndpoint) -> Result<()> {
    sqlx::query(
        "INSERT INTO llm_endpoint (id, name, base_url, api_key_ref, max_concurrency, notes, \
         last_health_at, last_health_ok, props_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) \
         ON CONFLICT(id) DO UPDATE SET name=excluded.name, base_url=excluded.base_url, \
         api_key_ref=excluded.api_key_ref, max_concurrency=excluded.max_concurrency, \
         notes=excluded.notes, last_health_at=excluded.last_health_at, \
         last_health_ok=excluded.last_health_ok, props_json=excluded.props_json",
    )
    .bind(e.id.as_str())
    .bind(e.name.as_str())
    .bind(e.base_url.as_str())
    .bind(e.api_key_ref.as_deref())
    .bind(e.max_concurrency)
    .bind(e.notes.as_deref())
    .bind(e.last_health_at.as_deref())
    .bind(e.last_health_ok)
    .bind(e.props_json.as_deref())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_endpoints(pool: &SqlitePool) -> Result<Vec<LlmEndpoint>> {
    let rows = sqlx::query_as::<_, LlmEndpoint>("SELECT * FROM llm_endpoint ORDER BY name")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn get_endpoint(pool: &SqlitePool, id: &str) -> Result<Option<LlmEndpoint>> {
    let row = sqlx::query_as::<_, LlmEndpoint>("SELECT * FROM llm_endpoint WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn delete_endpoint(pool: &SqlitePool, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM llm_endpoint WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

// ---------------------------------------------------------------------------
// role_binding
// ---------------------------------------------------------------------------

pub async fn upsert_role_binding(pool: &SqlitePool, b: &RoleBinding) -> Result<()> {
    sqlx::query(
        "INSERT INTO role_binding (id, endpoint_id, role, model, params_json, priority) \
         VALUES (?1,?2,?3,?4,?5,?6) \
         ON CONFLICT(id) DO UPDATE SET endpoint_id=excluded.endpoint_id, role=excluded.role, \
         model=excluded.model, params_json=excluded.params_json, priority=excluded.priority",
    )
    .bind(b.id.as_str())
    .bind(b.endpoint_id.as_str())
    .bind(b.role.as_str())
    .bind(b.model.as_str())
    .bind(b.params_json.as_str())
    .bind(b.priority)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_role_bindings(pool: &SqlitePool) -> Result<Vec<RoleBinding>> {
    let rows =
        sqlx::query_as::<_, RoleBinding>("SELECT * FROM role_binding ORDER BY priority DESC, role")
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

pub async fn role_binding_for(pool: &SqlitePool, role: &str) -> Result<Option<RoleBinding>> {
    let row = sqlx::query_as::<_, RoleBinding>(
        "SELECT * FROM role_binding WHERE role = ?1 ORDER BY priority DESC LIMIT 1",
    )
    .bind(role)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// project_memory
// ---------------------------------------------------------------------------

pub async fn set_memory(pool: &SqlitePool, project_id: &str, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO project_memory (project_id, key, value, revision, updated_at) \
         VALUES (?1,?2,?3,1,?4) \
         ON CONFLICT(project_id, key) DO UPDATE SET value=excluded.value, \
         revision=project_memory.revision+1, updated_at=excluded.updated_at",
    )
    .bind(project_id)
    .bind(key)
    .bind(value)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_memory(pool: &SqlitePool, project_id: &str, key: &str) -> Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT value FROM project_memory WHERE project_id = ?1 AND key = ?2")
            .bind(project_id)
            .bind(key)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

// ---------------------------------------------------------------------------
// glossary_term
// ---------------------------------------------------------------------------

pub async fn upsert_glossary_term(pool: &SqlitePool, t: &GlossaryTerm) -> Result<()> {
    sqlx::query(
        "INSERT INTO glossary_term (id, project_id, source_lang, target_lang, source, target, \
         note, kind, origin, revision, status) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) \
         ON CONFLICT(project_id, source_lang, target_lang, source) DO UPDATE SET \
         target=excluded.target, note=excluded.note, kind=excluded.kind, origin=excluded.origin, \
         revision=glossary_term.revision+1, status=excluded.status",
    )
    .bind(t.id.as_str())
    .bind(t.project_id.as_str())
    .bind(t.source_lang.as_deref())
    .bind(t.target_lang.as_deref())
    .bind(t.source.as_str())
    .bind(t.target.as_str())
    .bind(t.note.as_deref())
    .bind(t.kind.as_str())
    .bind(t.origin.as_str())
    .bind(t.revision)
    .bind(t.status.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_glossary_terms(pool: &SqlitePool, project_id: &str) -> Result<Vec<GlossaryTerm>> {
    let rows = sqlx::query_as::<_, GlossaryTerm>(
        "SELECT * FROM glossary_term WHERE project_id = ?1 ORDER BY source",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn delete_glossary_term(pool: &SqlitePool, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM glossary_term WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

// ---------------------------------------------------------------------------
// document / chapter / block / chunk
// ---------------------------------------------------------------------------

pub async fn insert_document(pool: &SqlitePool, d: &Document) -> Result<()> {
    sqlx::query(
        "INSERT INTO document (id, project_id, markdown_path, front_matter_json, extractor, \
         extractor_version, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
    )
    .bind(d.id.as_str())
    .bind(d.project_id.as_str())
    .bind(d.markdown_path.as_str())
    .bind(d.front_matter_json.as_str())
    .bind(d.extractor.as_str())
    .bind(d.extractor_version.as_str())
    .bind(d.created_at.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_chapter(pool: &SqlitePool, c: &Chapter) -> Result<()> {
    sqlx::query(
        "INSERT INTO chapter (id, document_id, order_index, title, level, block_first, block_last, \
         summary, summary_model, summary_hash, status) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
    )
    .bind(c.id.as_str())
    .bind(c.document_id.as_str())
    .bind(c.order_index)
    .bind(c.title.as_str())
    .bind(c.level)
    .bind(c.block_first)
    .bind(c.block_last)
    .bind(c.summary.as_deref())
    .bind(c.summary_model.as_deref())
    .bind(c.summary_hash.as_deref())
    .bind(c.status.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_block(pool: &SqlitePool, b: &Block) -> Result<()> {
    sqlx::query(
        "INSERT INTO block (id, document_id, chapter_id, order_index, kind, level, source_md, \
         source_text, translatable, attrs_json, content_hash) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) \
         ON CONFLICT(id) DO UPDATE SET chapter_id=excluded.chapter_id, order_index=excluded.order_index, \
         kind=excluded.kind, level=excluded.level, source_md=excluded.source_md, \
         source_text=excluded.source_text, translatable=excluded.translatable, \
         attrs_json=excluded.attrs_json, content_hash=excluded.content_hash",
    )
    .bind(b.id.as_str())
    .bind(b.document_id.as_str())
    .bind(b.chapter_id.as_deref())
    .bind(b.order_index)
    .bind(b.kind.as_str())
    .bind(b.level)
    .bind(b.source_md.as_str())
    .bind(b.source_text.as_str())
    .bind(b.translatable)
    .bind(b.attrs_json.as_str())
    .bind(b.content_hash.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_chunk(pool: &SqlitePool, c: &Chunk) -> Result<()> {
    sqlx::query(
        "INSERT INTO chunk (id, document_id, chapter_id, order_index, block_ids_json, source_md, \
         token_estimate, context_json, flags_json, status, prompt_hash, model_id, params_json, \
         context_manifest_json, target_md, error, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18) \
         ON CONFLICT(id) DO UPDATE SET block_ids_json=excluded.block_ids_json, source_md=excluded.source_md, \
         token_estimate=excluded.token_estimate, context_json=excluded.context_json, \
         flags_json=excluded.flags_json, updated_at=excluded.updated_at",
    )
    .bind(c.id.as_str())
    .bind(c.document_id.as_str())
    .bind(c.chapter_id.as_deref())
    .bind(c.order_index)
    .bind(c.block_ids_json.as_str())
    .bind(c.source_md.as_str())
    .bind(c.token_estimate)
    .bind(c.context_json.as_str())
    .bind(c.flags_json.as_str())
    .bind(c.status.as_str())
    .bind(c.prompt_hash.as_deref())
    .bind(c.model_id.as_deref())
    .bind(c.params_json.as_deref())
    .bind(c.context_manifest_json.as_deref())
    .bind(c.target_md.as_deref())
    .bind(c.error.as_deref())
    .bind(c.created_at.as_str())
    .bind(c.updated_at.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_chapters(pool: &SqlitePool, document_id: &str) -> Result<Vec<Chapter>> {
    let rows = sqlx::query_as::<_, Chapter>(
        "SELECT * FROM chapter WHERE document_id = ?1 ORDER BY order_index",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_blocks(pool: &SqlitePool, document_id: &str) -> Result<Vec<Block>> {
    let rows = sqlx::query_as::<_, Block>(
        "SELECT * FROM block WHERE document_id = ?1 ORDER BY order_index",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_chunks(pool: &SqlitePool, document_id: &str) -> Result<Vec<Chunk>> {
    let rows = sqlx::query_as::<_, Chunk>(
        "SELECT * FROM chunk WHERE document_id = ?1 ORDER BY order_index",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_chunks_by_project(
    pool: &SqlitePool,
    project_id: &str,
    status: Option<&str>,
) -> Result<Vec<Chunk>> {
    let rows = sqlx::query_as::<_, Chunk>(
        "SELECT c.* FROM chunk c JOIN document d ON d.id = c.document_id \
         WHERE d.project_id = ?1 AND (?2 IS NULL OR c.status = ?2) ORDER BY c.order_index",
    )
    .bind(project_id)
    .bind(status)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_chunk(pool: &SqlitePool, id: &str) -> Result<Option<Chunk>> {
    let row = sqlx::query_as::<_, Chunk>("SELECT * FROM chunk WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn set_chunk_status(pool: &SqlitePool, chunk_id: &str, status: &str) -> Result<()> {
    sqlx::query("UPDATE chunk SET status = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(chunk_id)
        .bind(status)
        .bind(now())
        .execute(pool)
        .await?;
    Ok(())
}

/// Idempotent write of a chunk outcome. Re-running the same job overwrites the
/// same row with the same content, so it never duplicates state.
pub async fn finish_chunk(pool: &SqlitePool, o: &ChunkOutcome) -> Result<()> {
    sqlx::query(
        "UPDATE chunk SET status=?2, prompt_hash=?3, model_id=?4, params_json=?5, \
         context_manifest_json=?6, target_md=?7, error=?8, updated_at=?9 WHERE id=?1",
    )
    .bind(o.chunk_id.as_str())
    .bind(o.status.as_str())
    .bind(o.prompt_hash.as_deref())
    .bind(o.model_id.as_deref())
    .bind(o.params_json.as_deref())
    .bind(o.context_manifest_json.as_deref())
    .bind(o.target_md.as_deref())
    .bind(o.error.as_deref())
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upsert_block_translation(pool: &SqlitePool, t: &BlockTranslation) -> Result<()> {
    sqlx::query(
        "INSERT INTO block_translation (block_id, chunk_id, text_md, placeholders_ok, origin, \
         edited_by_user, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7) \
         ON CONFLICT(block_id, origin) DO UPDATE SET text_md=excluded.text_md, \
         chunk_id=excluded.chunk_id, placeholders_ok=excluded.placeholders_ok, \
         edited_by_user=excluded.edited_by_user, updated_at=excluded.updated_at",
    )
    .bind(t.block_id.as_str())
    .bind(t.chunk_id.as_str())
    .bind(t.text_md.as_str())
    .bind(t.placeholders_ok)
    .bind(t.origin.as_str())
    .bind(t.edited_by_user)
    .bind(t.updated_at.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_block_translations(
    pool: &SqlitePool,
    chunk_id: &str,
) -> Result<Vec<BlockTranslation>> {
    let rows = sqlx::query_as::<_, BlockTranslation>(
        "SELECT * FROM block_translation WHERE chunk_id = ?1",
    )
    .bind(chunk_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn insert_qa_finding(pool: &SqlitePool, f: &QaFinding) -> Result<()> {
    sqlx::query(
        "INSERT INTO qa_finding (id, project_id, chunk_id, block_id, kind, severity, \
         details_json, status, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
    )
    .bind(f.id.as_str())
    .bind(f.project_id.as_str())
    .bind(f.chunk_id.as_deref())
    .bind(f.block_id.as_deref())
    .bind(f.kind.as_str())
    .bind(f.severity.as_str())
    .bind(f.details_json.as_str())
    .bind(f.status.as_str())
    .bind(f.created_at.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_qa_findings(pool: &SqlitePool, project_id: &str) -> Result<Vec<QaFinding>> {
    let rows = sqlx::query_as::<_, QaFinding>(
        "SELECT * FROM qa_finding WHERE project_id = ?1 ORDER BY created_at DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_llm_call(
    pool: &SqlitePool,
    job_id: Option<&str>,
    chunk_id: Option<&str>,
    role: &str,
    endpoint_id: Option<&str>,
    model: &str,
    params_json: &str,
    seed: Option<i64>,
    prompt_hash: &str,
    prompt_text: Option<&str>,
    response_text: Option<&str>,
    finish_reason: Option<&str>,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    latency_ms: Option<i64>,
    attempt: i64,
    error: Option<&str>,
) -> Result<String> {
    let id = new_id();
    sqlx::query(
        "INSERT INTO llm_call (id, job_id, chunk_id, role, endpoint_id, model, params_json, seed, \
         prompt_hash, prompt_text, prompt_compressed, response_text, finish_reason, prompt_tokens, \
         completion_tokens, latency_ms, attempt, error, created_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0,?11,?12,?13,?14,?15,?16,?17,?18)",
    )
    .bind(id.as_str())
    .bind(job_id)
    .bind(chunk_id)
    .bind(role)
    .bind(endpoint_id)
    .bind(model)
    .bind(params_json)
    .bind(seed)
    .bind(prompt_hash)
    .bind(prompt_text)
    .bind(response_text)
    .bind(finish_reason)
    .bind(prompt_tokens)
    .bind(completion_tokens)
    .bind(latency_ms)
    .bind(attempt)
    .bind(error)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// translation_cache / translation_memory
// ---------------------------------------------------------------------------

pub async fn cache_get(
    pool: &SqlitePool,
    prompt_hash: &str,
    model: &str,
    params_hash: &str,
    target_lang: &str,
) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT response_text FROM translation_cache WHERE prompt_hash=?1 AND model=?2 \
         AND params_hash=?3 AND target_lang=?4",
    )
    .bind(prompt_hash)
    .bind(model)
    .bind(params_hash)
    .bind(target_lang)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

pub async fn cache_put(
    pool: &SqlitePool,
    prompt_hash: &str,
    model: &str,
    params_hash: &str,
    target_lang: &str,
    response_text: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO translation_cache (prompt_hash, model, params_hash, target_lang, \
         response_text, created_at) VALUES (?1,?2,?3,?4,?5,?6) \
         ON CONFLICT(prompt_hash, model, params_hash, target_lang) DO UPDATE SET \
         response_text=excluded.response_text, created_at=excluded.created_at",
    )
    .bind(prompt_hash)
    .bind(model)
    .bind(params_hash)
    .bind(target_lang)
    .bind(response_text)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn memory_get(
    pool: &SqlitePool,
    content_hash: &str,
    model: &str,
    target_lang: &str,
) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT text_md FROM translation_memory WHERE content_hash=?1 AND model=?2 AND target_lang=?3",
    )
    .bind(content_hash)
    .bind(model)
    .bind(target_lang)
    .fetch_optional(pool)
    .await?;
    if row.is_some() {
        sqlx::query(
            "UPDATE translation_memory SET hits=hits+1, updated_at=?4 WHERE content_hash=?1 \
             AND model=?2 AND target_lang=?3",
        )
        .bind(content_hash)
        .bind(model)
        .bind(target_lang)
        .bind(now())
        .execute(pool)
        .await?;
    }
    Ok(row.map(|r| r.0))
}

pub async fn memory_put(
    pool: &SqlitePool,
    content_hash: &str,
    model: &str,
    target_lang: &str,
    text_md: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO translation_memory (content_hash, model, target_lang, text_md, hits, updated_at) \
         VALUES (?1,?2,?3,?4,0,?5) \
         ON CONFLICT(content_hash, model, target_lang) DO UPDATE SET text_md=excluded.text_md, \
         updated_at=excluded.updated_at",
    )
    .bind(content_hash)
    .bind(model)
    .bind(target_lang)
    .bind(text_md)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}
