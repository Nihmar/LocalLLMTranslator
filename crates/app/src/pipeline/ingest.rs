//! Ingestion: source document -> Markdown -> blocks -> chunks, persisted.

use serde::Serialize;
use serde_json::Value;

use super::PipelineDeps;
use crate::context::budget::DEFAULT_CHUNK_BUDGET;
use crate::db::models::{Block, Chapter, Document};
use crate::db::{new_id, now};
use crate::error::{AppError, Result};
use crate::util::sha256_hex_str;

#[derive(Debug, Clone, Serialize)]
pub struct IngestOutcome {
    pub document_id: String,
    pub markdown_path: String,
    pub chapters: usize,
    pub blocks: usize,
    pub chunks: usize,
    pub warnings: Vec<String>,
    /// Absolute path of the extracted media directory, if the source had any.
    pub assets_dir: Option<String>,
    /// Media hrefs as they appear in the Markdown (`assets/<name>`).
    pub assets: Vec<String>,
}

/// Run ingestion for a project.
pub async fn run_ingest(
    deps: &PipelineDeps,
    project_id: &str,
    source_path: &str,
    pdf_backend: Option<&str>,
    budget_tokens: Option<usize>,
) -> Result<IngestOutcome> {
    let pool = &deps.pool;
    if crate::db::repo::get_project(pool, project_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound(format!("project {project_id}")));
    }

    let work_dir = deps.work_dir(project_id);
    tokio::fs::create_dir_all(&work_dir).await?;

    // 1. Detect the format and let the sidecar extract a canonical Markdown.
    let detected = deps.sidecar.detect_format(source_path).await?;
    let ingest = deps
        .sidecar
        .ingest(source_path, &work_dir.to_string_lossy(), pdf_backend)
        .await?;

    // Make the extracted media observable: the work dir is stable and per project,
    // so the assets are not persisted in SQLite, but the run log must show them.
    tracing::info!(
        project_id,
        assets = ingest.assets.len(),
        assets_dir = ingest.assets_dir.as_deref().unwrap_or("<none>"),
        asset_paths = ?ingest.assets,
        "ingested document media"
    );

    // 2. Parse the Markdown into stable-id blocks and chapters.
    let parsed = deps.sidecar.parse_document(&ingest.markdown_path).await?;

    // 3. Build chunks (never a fixed character cut: always whole blocks).
    let budget = match budget_tokens {
        Some(budget) => budget,
        None => resolve_ingest_budget(deps).await,
    };
    let built_chunks = deps.sidecar.build_chunks(&parsed.blocks, budget).await?;

    let extractor_version = deps
        .sidecar
        .ping()
        .await
        .map(|p| p.version)
        .unwrap_or_default();

    // 4. Snapshot the prompts used, so the project is reproducible. Written only
    // when missing: the prompts are user data and a re-ingest must not discard an
    // edit made in the project snapshot.
    let prompts_dir = deps.prompts_dir(project_id);
    crate::context::builder::ensure_prompt_files(&prompts_dir).await?;
    // The reconnaissance prompt files (PLAN.md section 9.4) join the snapshot.
    // They are only written when missing, so a user edit survives a re-ingest.
    crate::pipeline::recon::ensure_prompt_files(&prompts_dir).await?;
    crate::pipeline::summarize::ensure_prompt_files(&prompts_dir).await?;
    crate::pipeline::review::ensure_prompt_files(&prompts_dir).await?;

    // 5. Persist document, chapters and blocks in a single transaction.
    let document_id = new_id();
    let front_matter = serde_json::to_string(&ingest.metadata)?;
    let document = Document {
        id: document_id.clone(),
        project_id: project_id.to_string(),
        markdown_path: ingest.markdown_path.clone(),
        front_matter_json: front_matter,
        extractor: detected.format.clone(),
        extractor_version,
        created_at: now(),
    };

    let mut tx = pool.begin().await?;

    // Re-ingesting replaces the previous document tree (cascades to chapters,
    // blocks, chunks and their children).
    sqlx::query("DELETE FROM document WHERE project_id = ?1")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO document (id, project_id, markdown_path, front_matter_json, extractor, \
         extractor_version, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
    )
    .bind(document.id.as_str())
    .bind(document.project_id.as_str())
    .bind(document.markdown_path.as_str())
    .bind(document.front_matter_json.as_str())
    .bind(document.extractor.as_str())
    .bind(document.extractor_version.as_str())
    .bind(document.created_at.as_str())
    .execute(&mut *tx)
    .await?;

    for chapter in &parsed.chapters {
        insert_chapter(&mut tx, chapter, &document_id).await?;
    }
    for block in &parsed.blocks {
        insert_block(&mut tx, block, &document_id).await?;
    }
    for chunk in &built_chunks.chunks {
        insert_chunk(&mut tx, chunk, &document_id).await?;
    }

    // Record book metadata on the project.
    sqlx::query(
        "UPDATE project SET source_format=?2, doc_title=COALESCE(?3, doc_title), \
                 doc_author=COALESCE(?4, doc_author), prompts_snapshot_dir=?5, updated_at=?6 \
                 WHERE id=?1",
    )
    .bind(project_id)
    .bind(detected.format.as_str())
    .bind(meta_str(&ingest.metadata, "title"))
    .bind(meta_str(&ingest.metadata, "author"))
    .bind(prompts_dir.to_string_lossy().to_string())
    .bind(now())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(IngestOutcome {
        document_id,
        markdown_path: ingest.markdown_path,
        chapters: parsed.chapters.len(),
        blocks: parsed.blocks.len(),
        chunks: built_chunks.chunks.len(),
        warnings: ingest.warnings,
        assets_dir: ingest.assets_dir,
        assets: ingest.assets,
    })
}

fn meta_str(metadata: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Budget for the sidecar chunker: the translator endpoint's `/props` when a
/// binding exists, the documented default otherwise — ingestion may well run
/// before any endpoint is configured.
async fn resolve_ingest_budget(deps: &PipelineDeps) -> usize {
    let Ok(Some(binding)) = crate::db::repo::role_binding_for(&deps.pool, "translator").await
    else {
        return DEFAULT_CHUNK_BUDGET;
    };
    let Ok(Some(endpoint)) = crate::db::repo::get_endpoint(&deps.pool, &binding.endpoint_id).await
    else {
        return DEFAULT_CHUNK_BUDGET;
    };
    crate::pipeline::resolve_endpoint_budget(&endpoint).await
}

async fn insert_chapter(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    chapter: &crate::db::models::ChapterIr,
    document_id: &str,
) -> Result<()> {
    let row = Chapter {
        id: chapter.id.clone(),
        document_id: document_id.to_string(),
        order_index: chapter.order,
        title: chapter.title.clone(),
        level: chapter.level,
        block_first: chapter.block_first,
        block_last: chapter.block_last,
        summary: None,
        summary_model: None,
        summary_hash: None,
        status: "pending".to_string(),
    };
    sqlx::query(
        "INSERT INTO chapter (id, document_id, order_index, title, level, block_first, block_last, \
         summary, summary_model, summary_hash, status) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) \
         ON CONFLICT(id) DO UPDATE SET order_index=excluded.order_index, title=excluded.title, \
         level=excluded.level, block_first=excluded.block_first, block_last=excluded.block_last",
    )
    .bind(row.id.as_str())
    .bind(row.document_id.as_str())
    .bind(row.order_index)
    .bind(row.title.as_str())
    .bind(row.level)
    .bind(row.block_first)
    .bind(row.block_last)
    .bind(row.summary.as_deref())
    .bind(row.summary_model.as_deref())
    .bind(row.summary_hash.as_deref())
    .bind(row.status.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_block(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    block: &crate::db::models::BlockIr,
    document_id: &str,
) -> Result<()> {
    let attrs_json = serde_json::to_string(&block.attrs)?;
    let content_hash = if block.content_hash.is_empty() {
        sha256_hex_str(&block.source_text)
    } else {
        block.content_hash.clone()
    };
    let row = Block {
        id: block.id.clone(),
        document_id: document_id.to_string(),
        chapter_id: block.chapter_id.clone(),
        order_index: block.order,
        kind: block.kind.clone(),
        level: block.level,
        source_md: block.source_md.clone(),
        source_text: block.source_text.clone(),
        translatable: block.translatable,
        attrs_json,
        content_hash,
    };
    sqlx::query(
        "INSERT INTO block (id, document_id, chapter_id, order_index, kind, level, source_md, \
         source_text, translatable, attrs_json, content_hash) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) \
         ON CONFLICT(id) DO UPDATE SET chapter_id=excluded.chapter_id, order_index=excluded.order_index, \
         kind=excluded.kind, level=excluded.level, source_md=excluded.source_md, \
         source_text=excluded.source_text, translatable=excluded.translatable, \
         attrs_json=excluded.attrs_json, content_hash=excluded.content_hash",
    )
    .bind(row.id.as_str())
    .bind(row.document_id.as_str())
    .bind(row.chapter_id.as_deref())
    .bind(row.order_index)
    .bind(row.kind.as_str())
    .bind(row.level)
    .bind(row.source_md.as_str())
    .bind(row.source_text.as_str())
    .bind(row.translatable)
    .bind(row.attrs_json.as_str())
    .bind(row.content_hash.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_chunk(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    chunk: &crate::db::models::ChunkIr,
    document_id: &str,
) -> Result<()> {
    let block_ids_json = serde_json::to_string(&chunk.block_ids)?;
    let context_json = serde_json::to_string(&chunk.context_carrier)?;
    let flags_json = serde_json::to_string(&chunk.flags)?;
    let timestamp = now();
    sqlx::query(
        "INSERT INTO chunk (id, document_id, chapter_id, order_index, block_ids_json, source_md, \
         token_estimate, context_json, flags_json, status, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'pending',?10,?10) \
         ON CONFLICT(id) DO UPDATE SET chapter_id=excluded.chapter_id, order_index=excluded.order_index, \
         block_ids_json=excluded.block_ids_json, source_md=excluded.source_md, \
         token_estimate=excluded.token_estimate, context_json=excluded.context_json, \
         flags_json=excluded.flags_json, updated_at=excluded.updated_at",
    )
    .bind(chunk.id.as_str())
    .bind(document_id)
    .bind(chunk.chapter_id.as_deref())
    .bind(chunk.order)
    .bind(block_ids_json.as_str())
    .bind(chunk.source_md.as_str())
    .bind(chunk.token_estimate)
    .bind(context_json.as_str())
    .bind(flags_json.as_str())
    .bind(timestamp.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
