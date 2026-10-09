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
         target_lang, doc_title, doc_author, series_id, series_order, prompts_snapshot_dir, \
         settings_json, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
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
    .bind(p.series_id.as_deref())
    .bind(p.series_order)
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

/// Delete a project and everything that hangs off it.
///
/// `document` (and through it chapters, blocks, chunks, suggestions) cascades, but
/// `qa_finding`, `glossary_term`, `project_memory` and `job` carry `project_id` without
/// a foreign key, and `llm_call` only points at jobs and chunks: those are deleted
/// explicitly, before the cascade removes the chunks `llm_call` is matched on. The
/// model calls hold prompts and answers, i.e. the text of the book being deleted.
pub async fn delete_project(pool: &SqlitePool, id: &str) -> Result<u64> {
    let mut tx = pool.begin().await?;
    for statement in [
        "DELETE FROM llm_call WHERE job_id IN (SELECT id FROM job WHERE project_id = ?1) \
         OR chunk_id IN (SELECT c.id FROM chunk c JOIN document d ON d.id = c.document_id \
         WHERE d.project_id = ?1)",
        "DELETE FROM qa_finding WHERE project_id = ?1",
        "DELETE FROM glossary_term WHERE project_id = ?1",
        "DELETE FROM project_memory WHERE project_id = ?1",
        "DELETE FROM job WHERE project_id = ?1",
    ] {
        sqlx::query(statement).bind(id).execute(&mut *tx).await?;
    }
    let res = sqlx::query("DELETE FROM project WHERE id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(res.rows_affected())
}

/// Delete the rows that point at a project's chunks but have no foreign key, before the
/// document tree is replaced by a re-ingest.
///
/// `llm_call` and `qa_finding` reference chunks/projects with plain `TEXT` columns, so
/// `DELETE FROM document` (which cascades to chapters/blocks/chunks) does not touch them.
/// Without this, a re-import leaves stale QA findings and model calls — including the book
/// text they carry — behind. `delete_project` handles the same rows for a deletion.
pub async fn delete_document_dependents(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM llm_call WHERE job_id IN (SELECT id FROM job WHERE project_id = ?1) \
         OR chunk_id IN (SELECT c.id FROM chunk c JOIN document d ON d.id = c.document_id \
         WHERE d.project_id = ?1)",
    )
    .bind(project_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM qa_finding WHERE project_id = ?1")
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
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

/// The binding of one role on one endpoint, used to update that row instead of adding a twin.
pub async fn find_role_binding(
    pool: &SqlitePool,
    role: &str,
    endpoint_id: &str,
) -> Result<Option<RoleBinding>> {
    let row = sqlx::query_as::<_, RoleBinding>(
        "SELECT * FROM role_binding WHERE role = ?1 AND endpoint_id = ?2 \
         ORDER BY priority DESC LIMIT 1",
    )
    .bind(role)
    .bind(endpoint_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Remove one binding. Returns the number of rows deleted, so a caller can tell whether the
/// assignment existed at all.
pub async fn delete_role_binding(pool: &SqlitePool, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM role_binding WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
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

pub async fn get_glossary_term(pool: &SqlitePool, id: &str) -> Result<Option<GlossaryTerm>> {
    let row = sqlx::query_as::<_, GlossaryTerm>("SELECT * FROM glossary_term WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// The term matching `source` case-insensitively, if any. The unique key is
/// case-sensitive, so the merge rule needs this lookup instead of relying on it.
pub async fn get_glossary_term_by_source(
    pool: &SqlitePool,
    project_id: &str,
    source: &str,
) -> Result<Option<GlossaryTerm>> {
    let row = sqlx::query_as::<_, GlossaryTerm>(
        "SELECT * FROM glossary_term WHERE project_id = ?1 AND lower(source) = lower(?2) LIMIT 1",
    )
    .bind(project_id)
    .bind(source)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Optimistic update of a term: only writes when `revision` still matches.
/// Returns `false` when another writer changed (or removed) the row.
pub async fn update_glossary_term_checked(
    pool: &SqlitePool,
    term: &GlossaryTerm,
    expected_revision: i64,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE glossary_term SET target = ?2, note = ?3, kind = ?4, status = ?5, \
         origin = 'manual', revision = revision + 1 WHERE id = ?1 AND revision = ?6",
    )
    .bind(term.id.as_str())
    .bind(term.target.as_str())
    .bind(term.note.as_deref())
    .bind(term.kind.as_str())
    .bind(term.status.as_str())
    .bind(expected_revision)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn set_glossary_status(pool: &SqlitePool, id: &str, status: &str) -> Result<()> {
    sqlx::query("UPDATE glossary_term SET status = ?2 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

/// Whether an open `glossary_conflict` finding already records this source term,
/// so re-running an agent does not pile up duplicate findings.
pub async fn has_open_glossary_conflict(
    pool: &SqlitePool,
    project_id: &str,
    source: &str,
) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM qa_finding WHERE project_id = ?1 AND kind = 'glossary_conflict' \
         AND status = 'open' AND json_extract(details_json, '$.source') = ?2",
    )
    .bind(project_id)
    .bind(source)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

pub async fn delete_glossary_term(pool: &SqlitePool, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM glossary_term WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

// ---------------------------------------------------------------------------
// series (PLAN.md §9.5)
// ---------------------------------------------------------------------------

pub async fn upsert_series(pool: &SqlitePool, s: &Series) -> Result<()> {
    sqlx::query(
        "INSERT INTO series (id, name, source_lang, target_lang, settings_json, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7) \
         ON CONFLICT(id) DO UPDATE SET name=excluded.name, source_lang=excluded.source_lang, \
         target_lang=excluded.target_lang, settings_json=excluded.settings_json, \
         updated_at=excluded.updated_at",
    )
    .bind(s.id.as_str())
    .bind(s.name.as_str())
    .bind(s.source_lang.as_deref())
    .bind(s.target_lang.as_deref())
    .bind(s.settings_json.as_str())
    .bind(s.created_at.as_str())
    .bind(s.updated_at.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_series(pool: &SqlitePool) -> Result<Vec<Series>> {
    let rows = sqlx::query_as::<_, Series>("SELECT * FROM series ORDER BY name")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn get_series(pool: &SqlitePool, id: &str) -> Result<Option<Series>> {
    let row = sqlx::query_as::<_, Series>("SELECT * FROM series WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn delete_series(pool: &SqlitePool, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM series WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Place a project in a series (or remove it with `series_id = None`).
pub async fn set_project_series(
    pool: &SqlitePool,
    project_id: &str,
    series_id: Option<&str>,
    series_order: Option<i64>,
) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE project SET series_id = ?2, series_order = ?3, updated_at = ?4 WHERE id = ?1",
    )
    .bind(project_id)
    .bind(series_id)
    .bind(series_order)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

pub async fn list_projects_for_series(pool: &SqlitePool, series_id: &str) -> Result<Vec<Project>> {
    let rows = sqlx::query_as::<_, Project>(
        "SELECT * FROM project WHERE series_id = ?1 ORDER BY series_order, created_at",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn upsert_series_term(pool: &SqlitePool, t: &SeriesGlossaryTerm) -> Result<()> {
    sqlx::query(
        "INSERT INTO series_glossary_term (id, series_id, source_lang, target_lang, source, target, \
         note, kind, origin, revision, status) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) \
         ON CONFLICT(series_id, source_lang, target_lang, source) DO UPDATE SET \
         target=excluded.target, note=excluded.note, kind=excluded.kind, origin=excluded.origin, \
         revision=series_glossary_term.revision+1, status=excluded.status",
    )
    .bind(t.id.as_str())
    .bind(t.series_id.as_str())
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

pub async fn list_series_terms(
    pool: &SqlitePool,
    series_id: &str,
) -> Result<Vec<SeriesGlossaryTerm>> {
    let rows = sqlx::query_as::<_, SeriesGlossaryTerm>(
        "SELECT * FROM series_glossary_term WHERE series_id = ?1 ORDER BY source",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_series_term(pool: &SqlitePool, id: &str) -> Result<Option<SeriesGlossaryTerm>> {
    let row =
        sqlx::query_as::<_, SeriesGlossaryTerm>("SELECT * FROM series_glossary_term WHERE id = ?1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

/// The series term matching `source` case-insensitively, if any.
pub async fn get_series_term_by_source(
    pool: &SqlitePool,
    series_id: &str,
    source: &str,
) -> Result<Option<SeriesGlossaryTerm>> {
    let row = sqlx::query_as::<_, SeriesGlossaryTerm>(
        "SELECT * FROM series_glossary_term WHERE series_id = ?1 AND lower(source) = lower(?2) LIMIT 1",
    )
    .bind(series_id)
    .bind(source)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Optimistic update of a series term: only writes when `revision` still matches.
pub async fn update_series_term_checked(
    pool: &SqlitePool,
    term: &SeriesGlossaryTerm,
    expected_revision: i64,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE series_glossary_term SET target = ?2, note = ?3, kind = ?4, status = ?5, \
         origin = 'manual', revision = revision + 1 WHERE id = ?1 AND revision = ?6",
    )
    .bind(term.id.as_str())
    .bind(term.target.as_str())
    .bind(term.note.as_deref())
    .bind(term.kind.as_str())
    .bind(term.status.as_str())
    .bind(expected_revision)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn set_series_term_status(pool: &SqlitePool, id: &str, status: &str) -> Result<()> {
    sqlx::query("UPDATE series_glossary_term SET status = ?2 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_series_term(pool: &SqlitePool, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM series_glossary_term WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn upsert_series_variant(pool: &SqlitePool, v: &SeriesGlossaryVariant) -> Result<()> {
    sqlx::query(
        "INSERT INTO series_glossary_variant (id, term_id, text) VALUES (?1,?2,?3) \
         ON CONFLICT(term_id, text) DO NOTHING",
    )
    .bind(v.id.as_str())
    .bind(v.term_id.as_str())
    .bind(v.text.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_series_variant(pool: &SqlitePool, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM series_glossary_variant WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn list_series_variants(
    pool: &SqlitePool,
    term_id: &str,
) -> Result<Vec<SeriesGlossaryVariant>> {
    let rows = sqlx::query_as::<_, SeriesGlossaryVariant>(
        "SELECT * FROM series_glossary_variant WHERE term_id = ?1 ORDER BY text",
    )
    .bind(term_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Every variant of a series, so the effective glossary can be resolved in one query.
pub async fn list_variants_for_series(
    pool: &SqlitePool,
    series_id: &str,
) -> Result<Vec<SeriesGlossaryVariant>> {
    let rows = sqlx::query_as::<_, SeriesGlossaryVariant>(
        "SELECT v.* FROM series_glossary_variant v \
         JOIN series_glossary_term t ON t.id = v.term_id WHERE t.series_id = ?1 ORDER BY v.text",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn set_series_memory(
    pool: &SqlitePool,
    series_id: &str,
    key: &str,
    value: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO series_memory (series_id, key, value, revision, updated_at) \
         VALUES (?1,?2,?3,1,?4) \
         ON CONFLICT(series_id, key) DO UPDATE SET value=excluded.value, \
         revision=series_memory.revision+1, updated_at=excluded.updated_at",
    )
    .bind(series_id)
    .bind(key)
    .bind(value)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_series_memory(
    pool: &SqlitePool,
    series_id: &str,
    key: &str,
) -> Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT value FROM series_memory WHERE series_id = ?1 AND key = ?2")
            .bind(series_id)
            .bind(key)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

/// Remove a series memory value. Returns the number of rows deleted.
pub async fn delete_series_memory(pool: &SqlitePool, series_id: &str, key: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM series_memory WHERE series_id = ?1 AND key = ?2")
        .bind(series_id)
        .bind(key)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn list_series_memory(pool: &SqlitePool, series_id: &str) -> Result<Vec<SeriesMemory>> {
    let rows = sqlx::query_as::<_, SeriesMemory>(
        "SELECT * FROM series_memory WHERE series_id = ?1 ORDER BY key",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
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

/// The document extracted from `project_id`'s source, if it has been ingested.
pub async fn get_document_for_project(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Option<Document>> {
    let row = sqlx::query_as::<_, Document>(
        "SELECT id, project_id, markdown_path, front_matter_json, extractor, extractor_version, \
         created_at FROM document WHERE project_id = ?1 LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
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

pub async fn get_chapter(pool: &SqlitePool, id: &str) -> Result<Option<Chapter>> {
    let row = sqlx::query_as::<_, Chapter>("SELECT * FROM chapter WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// Store the chapter summary produced by a final `summarize` run and mark the
/// chapter as summarized.
pub async fn update_chapter_summary(
    pool: &SqlitePool,
    chapter_id: &str,
    summary: &str,
    model: &str,
    summary_hash: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE chapter SET summary = ?2, summary_model = ?3, summary_hash = ?4, status = 'done' \
         WHERE id = ?1",
    )
    .bind(chapter_id)
    .bind(summary)
    .bind(model)
    .bind(summary_hash)
    .execute(pool)
    .await?;
    Ok(())
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

pub async fn get_block(pool: &SqlitePool, id: &str) -> Result<Option<Block>> {
    let row = sqlx::query_as::<_, Block>("SELECT * FROM block WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
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

/// Return one chunk left `running` to `pending`, after the job that was working on it was
/// interrupted.
///
/// Only a `running` chunk is touched: a chunk whose job had already written its translation keeps
/// its outcome, and a chunk that never started stays `pending`. Without this, an interrupted
/// chunk stayed `running` forever and `translation_start` — which only enqueues `pending`,
/// `failed` and `needs_review` — would never pick it up again.
pub async fn reset_chunk_to_pending(pool: &SqlitePool, chunk_id: &str) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE chunk SET status='pending', updated_at=?2 WHERE id=?1 AND status='running'",
    )
    .bind(chunk_id)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Return chunks left `running` to `pending`, optionally scoped to one project.
///
/// A chunk is `running` only while its `translate_chunk` job executes, so a
/// chunk still in that state at boot (or after a cancel) belongs to an
/// interrupted job. Resetting it makes it eligible for `translation_start`
/// again. `project_id = None` covers every document (boot recovery); `Some`
/// restricts the reset to that project, so one project's cancel does not disturb
/// another project's in-flight chunks. Returns the number of chunks reset.
pub async fn reset_running_chunks(pool: &SqlitePool, project_id: Option<&str>) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE chunk SET status='pending', updated_at=?1 WHERE status='running' \
         AND (?2 IS NULL OR document_id IN (SELECT id FROM document WHERE project_id = ?2))",
    )
    .bind(now())
    .bind(project_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
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

/// Store the recomposed markdown of a chunk after a review accepted a change.
pub async fn update_chunk_target(pool: &SqlitePool, chunk_id: &str, target_md: &str) -> Result<()> {
    sqlx::query("UPDATE chunk SET target_md = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(chunk_id)
        .bind(target_md)
        .bind(now())
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// suggestion
// ---------------------------------------------------------------------------

/// A project's suggestions, newest first; every filter is optional.
pub async fn list_suggestions(
    pool: &SqlitePool,
    project_id: &str,
    chunk_id: Option<&str>,
    pass: Option<&str>,
    status: Option<&str>,
) -> Result<Vec<Suggestion>> {
    let rows = sqlx::query_as::<_, Suggestion>(
        "SELECT s.* FROM suggestion s \
         JOIN chunk c ON c.id = s.chunk_id \
         JOIN document d ON d.id = c.document_id \
         WHERE d.project_id = ?1 \
         AND (?2 IS NULL OR s.chunk_id = ?2) \
         AND (?3 IS NULL OR s.pass = ?3) \
         AND (?4 IS NULL OR s.status = ?4) \
         ORDER BY s.created_at DESC, s.id",
    )
    .bind(project_id)
    .bind(chunk_id)
    .bind(pass)
    .bind(status)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// The project's decisions, newest first: only rows that carry a `decided_at`.
/// Pending and superseded proposals were never decided, so they are not history.
pub async fn list_decided_suggestions(
    pool: &SqlitePool,
    project_id: &str,
    chunk_id: Option<&str>,
    pass: Option<&str>,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<Suggestion>> {
    let rows = sqlx::query_as::<_, Suggestion>(
        "SELECT s.* FROM suggestion s \
         JOIN chunk c ON c.id = s.chunk_id \
         JOIN document d ON d.id = c.document_id \
         WHERE d.project_id = ?1 \
         AND (?2 IS NULL OR s.chunk_id = ?2) \
         AND (?3 IS NULL OR s.pass = ?3) \
         AND (?4 IS NULL OR s.status = ?4) \
         AND s.decided_at IS NOT NULL \
         ORDER BY s.decided_at DESC, s.id \
         LIMIT ?5",
    )
    .bind(project_id)
    .bind(chunk_id)
    .bind(pass)
    .bind(status)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_suggestion(pool: &SqlitePool, id: &str) -> Result<Option<Suggestion>> {
    let row = sqlx::query_as::<_, Suggestion>("SELECT * FROM suggestion WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn insert_suggestion(pool: &SqlitePool, s: &Suggestion) -> Result<()> {
    sqlx::query(
        "INSERT INTO suggestion (id, chunk_id, pass, block_id, field, original, proposed, \
         reason, severity, quote, status, created_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
    )
    .bind(s.id.as_str())
    .bind(s.chunk_id.as_str())
    .bind(s.pass.as_str())
    .bind(s.block_id.as_deref())
    .bind(s.field.as_deref())
    .bind(s.original.as_deref())
    .bind(s.proposed.as_deref())
    .bind(s.reason.as_deref())
    .bind(s.severity.as_deref())
    .bind(s.quote.as_deref())
    .bind(s.status.as_str())
    .bind(s.created_at.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a decision on a suggestion. `decided_at` is stamped here and never
/// cleared, which is what turns the retained row into a correction history entry.
pub async fn set_suggestion_status(pool: &SqlitePool, id: &str, status: &str) -> Result<()> {
    sqlx::query("UPDATE suggestion SET status = ?2, decided_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .bind(now())
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark the still-pending suggestions of a pass as superseded, so a re-run
/// replaces the old proposals instead of piling up.
pub async fn supersede_suggestions(pool: &SqlitePool, chunk_id: &str, pass: &str) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE suggestion SET status = 'superseded' WHERE chunk_id = ?1 AND pass = ?2 \
         AND status = 'pending'",
    )
    .bind(chunk_id)
    .bind(pass)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Sibling proposals were computed against the text that just changed, so
/// accepting one supersedes the other pending ones for the same block.
pub async fn supersede_suggestions_for_block(
    pool: &SqlitePool,
    chunk_id: &str,
    pass: &str,
    block_id: &str,
) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE suggestion SET status = 'superseded' WHERE chunk_id = ?1 AND pass = ?2 \
         AND block_id = ?3 AND status = 'pending'",
    )
    .bind(chunk_id)
    .bind(pass)
    .bind(block_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
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

/// Findings of one project, each filter optional.
pub async fn list_qa_findings_filtered(
    pool: &SqlitePool,
    project_id: &str,
    kind: Option<&str>,
    severity: Option<&str>,
    chunk_id: Option<&str>,
    status: Option<&str>,
) -> Result<Vec<QaFinding>> {
    let rows = sqlx::query_as::<_, QaFinding>(
        "SELECT * FROM qa_finding WHERE project_id = ?1 \
         AND (?2 IS NULL OR kind = ?2) \
         AND (?3 IS NULL OR severity = ?3) \
         AND (?4 IS NULL OR chunk_id = ?4) \
         AND (?5 IS NULL OR status = ?5) \
         ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'major' THEN 1 ELSE 2 END, kind, created_at",
    )
    .bind(project_id)
    .bind(kind)
    .bind(severity)
    .bind(chunk_id)
    .bind(status)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Set a finding's status (`open`, `resolved`, `ignored`). Returns the rows affected, so a
/// caller can tell a real transition from a no-op.
pub async fn set_qa_finding_status(pool: &SqlitePool, id: &str, status: &str) -> Result<u64> {
    let res = sqlx::query("UPDATE qa_finding SET status = ?2 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Replace a chunk's findings on a re-scan.
pub async fn delete_qa_findings_for_chunk(pool: &SqlitePool, chunk_id: &str) -> Result<u64> {
    let result = sqlx::query("DELETE FROM qa_finding WHERE chunk_id = ?1")
        .bind(chunk_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
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
    reasoning_text: Option<&str>,
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
         prompt_hash, prompt_text, prompt_compressed, response_text, reasoning_text, finish_reason, \
         prompt_tokens, completion_tokens, latency_ms, attempt, error, created_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
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
    .bind(reasoning_text)
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
    glossary_hash: &str,
) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT text_md FROM translation_memory WHERE content_hash=?1 AND model=?2 AND target_lang=?3 \
         AND glossary_hash=?4",
    )
    .bind(content_hash)
    .bind(model)
    .bind(target_lang)
    .bind(glossary_hash)
    .fetch_optional(pool)
    .await?;
    if row.is_some() {
        sqlx::query(
            "UPDATE translation_memory SET hits=hits+1, updated_at=?5 WHERE content_hash=?1 \
             AND model=?2 AND target_lang=?3 AND glossary_hash=?4",
        )
        .bind(content_hash)
        .bind(model)
        .bind(target_lang)
        .bind(glossary_hash)
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
    glossary_hash: &str,
    text_md: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO translation_memory (content_hash, model, target_lang, glossary_hash, text_md, hits, updated_at) \
         VALUES (?1,?2,?3,?4,?5,0,?6) \
         ON CONFLICT(content_hash, model, target_lang, glossary_hash) DO UPDATE SET \
         text_md=excluded.text_md, updated_at=excluded.updated_at",
    )
    .bind(content_hash)
    .bind(model)
    .bind(target_lang)
    .bind(glossary_hash)
    .bind(text_md)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// app_state (persisted application flags)
// ---------------------------------------------------------------------------

/// `app_state` key for the worker-pool pause flag; value `"1"` means paused.
pub const KEY_WORKER_PAUSED: &str = "worker_paused";

/// Upsert an application-level key/value pair.
pub async fn set_app_state(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO app_state (key, value, updated_at) VALUES (?1,?2,?3) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Read an application-level value, or `None` when it was never set.
pub async fn get_app_state(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM app_state WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

/// Remove an application-level value. Returns the number of rows deleted.
pub async fn delete_app_state(pool: &SqlitePool, key: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM app_state WHERE key = ?1")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connect_memory;

    /// Minimal project/document with two translatable blocks and one chunk.
    async fn seed(pool: &SqlitePool) {
        let ts = now();
        sqlx::query(
            "INSERT INTO project (id, name, source_path, source_hash, source_format, target_lang, \
             settings_json, created_at, updated_at) VALUES ('p','p','/x','h','epub','it','{}',?1,?1)",
        )
        .bind(&ts)
        .execute(pool)
        .await
        .expect("project");
        sqlx::query(
            "INSERT INTO document (id, project_id, markdown_path, front_matter_json, extractor, \
             extractor_version, created_at) VALUES ('d','p','/m','{}','epub','0',?1)",
        )
        .bind(&ts)
        .execute(pool)
        .await
        .expect("document");
        for (id, translatable) in [("b1", 1), ("b2", 1)] {
            sqlx::query(
                "INSERT INTO block (id, document_id, order_index, kind, level, source_md, \
                 source_text, translatable, attrs_json, content_hash) \
                 VALUES (?1,'d',0,'para',0,'a','a',?2,'{}','h')",
            )
            .bind(id)
            .bind(translatable)
            .execute(pool)
            .await
            .expect("block");
        }
        sqlx::query(
            "INSERT INTO chunk (id, document_id, order_index, block_ids_json, source_md, \
             token_estimate, context_json, flags_json, status, created_at, updated_at) \
             VALUES ('c1','d',0,'[\"b1\",\"b2\"]','a',1,'{}','[]','running',?1,?1)",
        )
        .bind(&ts)
        .execute(pool)
        .await
        .expect("chunk");
    }

    #[tokio::test]
    async fn delete_project_leaves_no_orphan_rows() {
        let pool = connect_memory().await.expect("pool");
        seed(&pool).await;
        let ts = now();
        for statement in [
            "INSERT INTO job (id, project_id, kind, payload_json, created_at) \
             VALUES ('j1','p','translate_chunk','{}',?1)",
            "INSERT INTO llm_call (id, job_id, role, model, params_json, prompt_hash, created_at) \
             VALUES ('l1','j1','translator','m','{}','h',?1)",
            "INSERT INTO llm_call (id, chunk_id, role, model, params_json, prompt_hash, created_at) \
             VALUES ('l2','c1','editor','m','{}','h',?1)",
            "INSERT INTO qa_finding (id, project_id, kind, severity, details_json, created_at) \
             VALUES ('q1','p','untranslated','major','{}',?1)",
            "INSERT INTO glossary_term (id, project_id, source, target) VALUES ('g1','p','a','b')",
            "INSERT INTO project_memory (project_id, key, value, updated_at) \
             VALUES ('p','synopsis','x',?1)",
        ] {
            sqlx::query(statement)
                .bind(&ts)
                .execute(&pool)
                .await
                .expect(statement);
        }

        assert_eq!(delete_project(&pool, "p").await.expect("delete"), 1);

        for count in [
            "SELECT COUNT(*) FROM project",
            "SELECT COUNT(*) FROM document",
            "SELECT COUNT(*) FROM chunk",
            "SELECT COUNT(*) FROM job",
            "SELECT COUNT(*) FROM llm_call",
            "SELECT COUNT(*) FROM qa_finding",
            "SELECT COUNT(*) FROM glossary_term",
            "SELECT COUNT(*) FROM project_memory",
        ] {
            let left: i64 = sqlx::query_scalar(count)
                .fetch_one(&pool)
                .await
                .expect(count);
            assert_eq!(left, 0, "{count}: rows of the deleted project are left");
        }
    }

    #[tokio::test]
    async fn delete_document_dependents_removes_orphan_calls_and_findings() {
        let pool = connect_memory().await.expect("pool");
        seed(&pool).await;
        let ts = now();
        for statement in [
            "INSERT INTO job (id, project_id, kind, payload_json, created_at) \
             VALUES ('j1','p','translate_chunk','{}',?1)",
            "INSERT INTO llm_call (id, job_id, role, model, params_json, prompt_hash, created_at) \
             VALUES ('l1','j1','translator','m','{}','h',?1)",
            "INSERT INTO llm_call (id, chunk_id, role, model, params_json, prompt_hash, created_at) \
             VALUES ('l2','c1','editor','m','{}','h',?1)",
            "INSERT INTO qa_finding (id, project_id, kind, severity, details_json, created_at) \
             VALUES ('q1','p','untranslated','major','{}',?1)",
        ] {
            sqlx::query(statement)
                .bind(&ts)
                .execute(&pool)
                .await
                .expect(statement);
        }

        // A re-ingest deletes the document tree; the dependents must go first.
        let mut tx = pool.begin().await.expect("tx");
        delete_document_dependents(&mut tx, "p")
            .await
            .expect("clean");
        sqlx::query("DELETE FROM document WHERE project_id = ?1")
            .bind("p")
            .execute(&mut *tx)
            .await
            .expect("document");
        tx.commit().await.expect("commit");

        for count in [
            "SELECT COUNT(*) FROM llm_call",
            "SELECT COUNT(*) FROM qa_finding",
            "SELECT COUNT(*) FROM chunk",
        ] {
            let left: i64 = sqlx::query_scalar(count)
                .fetch_one(&pool)
                .await
                .expect(count);
            assert_eq!(left, 0, "{count}: rows of the replaced document are left");
        }
        // The project itself and the job row survive (the job is the running re-ingest).
        let projects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project")
            .fetch_one(&pool)
            .await
            .expect("projects");
        assert_eq!(projects, 1);
    }

    #[tokio::test]
    async fn delete_role_binding_removes_one_assignment() {
        let pool = connect_memory().await.expect("pool");
        for id in ["e1", "e2"] {
            upsert_endpoint(
                &pool,
                &LlmEndpoint {
                    id: id.to_string(),
                    name: id.to_string(),
                    base_url: "http://127.0.0.1:8080".to_string(),
                    api_key_ref: None,
                    max_concurrency: None,
                    notes: None,
                    last_health_at: None,
                    last_health_ok: None,
                    props_json: None,
                },
            )
            .await
            .expect("endpoint");
        }
        let translator = RoleBinding {
            id: "b1".to_string(),
            endpoint_id: "e1".to_string(),
            role: "translator".to_string(),
            model: "m1".to_string(),
            params_json: "{}".to_string(),
            priority: 10,
        };
        let editor = RoleBinding {
            id: "b2".to_string(),
            endpoint_id: "e2".to_string(),
            role: "editor".to_string(),
            model: "m2".to_string(),
            params_json: "{}".to_string(),
            priority: 0,
        };
        upsert_role_binding(&pool, &translator).await.expect("b1");
        upsert_role_binding(&pool, &editor).await.expect("b2");

        assert_eq!(delete_role_binding(&pool, "b1").await.expect("delete"), 1);
        // Deleting what is not there is not an error: the UI may race with another window.
        assert_eq!(delete_role_binding(&pool, "b1").await.expect("delete"), 0);

        let left = list_role_bindings(&pool).await.expect("list");
        assert_eq!(left.len(), 1, "only the removed row is gone");
        assert_eq!(left[0].id, "b2");
        assert!(role_binding_for(&pool, "translator")
            .await
            .expect("lookup")
            .is_none());
    }

    #[tokio::test]
    async fn find_role_binding_matches_the_pair_not_the_model() {
        let pool = connect_memory().await.expect("pool");
        upsert_endpoint(
            &pool,
            &LlmEndpoint {
                id: "e1".to_string(),
                name: "e1".to_string(),
                base_url: "http://127.0.0.1:8080".to_string(),
                api_key_ref: None,
                max_concurrency: None,
                notes: None,
                last_health_at: None,
                last_health_ok: None,
                props_json: None,
            },
        )
        .await
        .expect("endpoint");
        upsert_role_binding(
            &pool,
            &RoleBinding {
                id: "b1".to_string(),
                endpoint_id: "e1".to_string(),
                role: "translator".to_string(),
                model: "first-model".to_string(),
                params_json: "{}".to_string(),
                priority: 0,
            },
        )
        .await
        .expect("b1");

        let found = find_role_binding(&pool, "translator", "e1")
            .await
            .expect("find")
            .expect("some");
        assert_eq!(found.id, "b1");
        assert!(find_role_binding(&pool, "editor", "e1")
            .await
            .expect("find")
            .is_none());
        assert!(find_role_binding(&pool, "translator", "other")
            .await
            .expect("find")
            .is_none());
    }

    #[tokio::test]
    async fn reset_chunk_to_pending_only_touches_a_running_chunk() {
        let pool = connect_memory().await.expect("pool");
        seed(&pool).await;

        assert_eq!(reset_chunk_to_pending(&pool, "c1").await.expect("reset"), 1);
        assert_eq!(
            get_chunk(&pool, "c1")
                .await
                .expect("get")
                .expect("some")
                .status,
            "pending"
        );
        // Idempotent: a chunk already returned to the queue is left alone.
        assert_eq!(reset_chunk_to_pending(&pool, "c1").await.expect("reset"), 0);

        // A chunk whose job wrote its translation keeps it.
        sqlx::query(
            "INSERT INTO chunk (id, document_id, order_index, block_ids_json, source_md, \
             token_estimate, context_json, flags_json, status, created_at, updated_at) \
             VALUES ('c2','d',1,'[]','a',1,'{}','[]','done',?1,?1)",
        )
        .bind(now())
        .execute(&pool)
        .await
        .expect("chunk 2");
        assert_eq!(reset_chunk_to_pending(&pool, "c2").await.expect("reset"), 0);
        assert_eq!(
            get_chunk(&pool, "c2")
                .await
                .expect("get")
                .expect("some")
                .status,
            "done"
        );
    }

    #[tokio::test]
    async fn reset_running_chunks_only_touches_running_chunks() {
        let pool = connect_memory().await.expect("pool");
        seed(&pool).await;
        sqlx::query(
            "INSERT INTO chunk (id, document_id, order_index, block_ids_json, source_md, \
             token_estimate, context_json, flags_json, status, created_at, updated_at) \
             VALUES ('c2','d',1,'[]','a',1,'{}','[]','done',?1,?1)",
        )
        .bind(now())
        .execute(&pool)
        .await
        .expect("chunk 2");

        assert_eq!(reset_running_chunks(&pool, None).await.expect("reset"), 1);
        assert_eq!(
            get_chunk(&pool, "c1")
                .await
                .expect("get")
                .expect("some")
                .status,
            "pending"
        );
        assert_eq!(
            get_chunk(&pool, "c2")
                .await
                .expect("get")
                .expect("some")
                .status,
            "done"
        );
    }

    #[tokio::test]
    async fn reset_running_chunks_can_be_scoped_to_one_project() {
        let pool = connect_memory().await.expect("pool");
        seed(&pool).await;
        // A second project with its own running chunk.
        let ts = now();
        sqlx::query(
            "INSERT INTO project (id, name, source_path, source_hash, source_format, target_lang, \
             settings_json, created_at, updated_at) VALUES ('p2','p2','/x','h','epub','it','{}',?1,?1)",
        )
        .bind(&ts)
        .execute(&pool)
        .await
        .expect("project 2");
        sqlx::query(
            "INSERT INTO document (id, project_id, markdown_path, front_matter_json, extractor, \
             extractor_version, created_at) VALUES ('d2','p2','/m','{}','epub','0',?1)",
        )
        .bind(&ts)
        .execute(&pool)
        .await
        .expect("document 2");
        sqlx::query(
            "INSERT INTO chunk (id, document_id, order_index, block_ids_json, source_md, \
             token_estimate, context_json, flags_json, status, created_at, updated_at) \
             VALUES ('c2','d2',0,'[]','a',1,'{}','[]','running',?1,?1)",
        )
        .bind(&ts)
        .execute(&pool)
        .await
        .expect("chunk 2");

        // A scoped reset leaves the other project's running chunk alone.
        assert_eq!(
            reset_running_chunks(&pool, Some("p")).await.expect("reset"),
            1
        );
        assert_eq!(
            get_chunk(&pool, "c1")
                .await
                .expect("get")
                .expect("some")
                .status,
            "pending"
        );
        assert_eq!(
            get_chunk(&pool, "c2")
                .await
                .expect("get")
                .expect("some")
                .status,
            "running"
        );

        // The unscoped reset catches everything left over (boot recovery).
        assert_eq!(reset_running_chunks(&pool, None).await.expect("reset"), 1);
        assert_eq!(
            get_chunk(&pool, "c2")
                .await
                .expect("get")
                .expect("some")
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn a_glossary_upsert_on_the_unique_key_keeps_the_existing_id_and_bumps_revision() {
        // Why `glossary_upsert` re-reads the row it wrote: the ON CONFLICT target is the
        // unique key, so a second write with a different id updates the *first* row and
        // leaves the constructed value's id and revision stale.
        let pool = connect_memory().await.expect("pool");
        seed(&pool).await;
        let first = GlossaryTerm {
            id: "t1".to_string(),
            project_id: "p".to_string(),
            source_lang: Some("en".to_string()),
            target_lang: Some("it".to_string()),
            source: "keeper".to_string(),
            target: "guardiano".to_string(),
            note: None,
            kind: "term".to_string(),
            origin: "manual".to_string(),
            revision: 1,
            status: "approved".to_string(),
        };
        upsert_glossary_term(&pool, &first).await.expect("first");
        let second = GlossaryTerm {
            id: "t2".to_string(),
            target: "custode".to_string(),
            ..first
        };
        upsert_glossary_term(&pool, &second).await.expect("second");

        let stored = get_glossary_term(&pool, "t1")
            .await
            .expect("get")
            .expect("row");
        assert_eq!(stored.target, "custode");
        assert_eq!(stored.revision, 2, "the existing row was updated in place");
        assert!(get_glossary_term(&pool, "t2").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn translation_memory_is_scoped_to_the_effective_glossary() {
        let pool = connect_memory().await.expect("pool");
        memory_put(&pool, "h1", "m", "it", "glossary-a", "Ciao")
            .await
            .expect("put");
        assert_eq!(
            memory_get(&pool, "h1", "m", "it", "glossary-a")
                .await
                .expect("get")
                .as_deref(),
            Some("Ciao")
        );
        // A different canon must not reuse the old rendering.
        assert!(memory_get(&pool, "h1", "m", "it", "glossary-b")
            .await
            .expect("get")
            .is_none());

        memory_put(&pool, "h1", "m", "it", "glossary-b", "Salve")
            .await
            .expect("put");
        assert_eq!(
            memory_get(&pool, "h1", "m", "it", "glossary-b")
                .await
                .expect("get")
                .as_deref(),
            Some("Salve")
        );
        assert_eq!(
            memory_get(&pool, "h1", "m", "it", "glossary-a")
                .await
                .expect("get")
                .as_deref(),
            Some("Ciao"),
            "the two canons coexist"
        );
    }

    #[tokio::test]
    async fn qa_findings_can_be_filtered_by_status_and_closed() {
        let pool = connect_memory().await.expect("pool");
        seed(&pool).await;
        let id = new_id();
        insert_qa_finding(
            &pool,
            &QaFinding {
                id: id.clone(),
                project_id: "p".to_string(),
                chunk_id: None,
                block_id: None,
                kind: "glossary_conflict".to_string(),
                severity: "minor".to_string(),
                details_json: "{}".to_string(),
                status: "open".to_string(),
                created_at: now(),
            },
        )
        .await
        .expect("finding");

        let open = list_qa_findings_filtered(
            &pool,
            "p",
            Some("glossary_conflict"),
            None,
            None,
            Some("open"),
        )
        .await
        .expect("open");
        assert_eq!(open.len(), 1);
        assert!(
            list_qa_findings_filtered(&pool, "p", None, None, None, Some("resolved"))
                .await
                .expect("resolved")
                .is_empty()
        );

        assert_eq!(
            set_qa_finding_status(&pool, &id, "resolved")
                .await
                .expect("close"),
            1
        );
        assert_eq!(
            list_qa_findings_filtered(&pool, "p", None, None, None, Some("resolved"))
                .await
                .expect("resolved")
                .len(),
            1
        );
        // Reopening works too, and an unknown id is a no-op.
        assert_eq!(
            set_qa_finding_status(&pool, &id, "open")
                .await
                .expect("reopen"),
            1
        );
        assert_eq!(
            set_qa_finding_status(&pool, "missing", "resolved")
                .await
                .expect("missing"),
            0
        );
    }

    #[tokio::test]
    async fn app_state_round_trips_upserts_and_deletes() {
        let pool = connect_memory().await.expect("pool");
        assert!(get_app_state(&pool, KEY_WORKER_PAUSED)
            .await
            .expect("get")
            .is_none());

        set_app_state(&pool, KEY_WORKER_PAUSED, "1")
            .await
            .expect("set");
        assert_eq!(
            get_app_state(&pool, KEY_WORKER_PAUSED)
                .await
                .expect("get")
                .as_deref(),
            Some("1")
        );

        // Upsert overwrites the previous value.
        set_app_state(&pool, KEY_WORKER_PAUSED, "0")
            .await
            .expect("set");
        assert_eq!(
            get_app_state(&pool, KEY_WORKER_PAUSED)
                .await
                .expect("get")
                .as_deref(),
            Some("0")
        );

        assert_eq!(
            delete_app_state(&pool, KEY_WORKER_PAUSED)
                .await
                .expect("delete"),
            1
        );
        assert!(get_app_state(&pool, KEY_WORKER_PAUSED)
            .await
            .expect("get")
            .is_none());
    }
}
