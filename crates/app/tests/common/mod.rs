//! Helpers shared by the integration tests (`tests/recon.rs`,
//! `tests/summarize.rs`): a real SQLite database, a sidecar supervisor that is
//! never started (these tests do not call the sidecar) and a seeded
//! project/document/chapter tree.

#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use sqlx::SqlitePool;

use app_lib::db::models::{
    Block, BlockTranslation, Chapter, Chunk, Document, LlmEndpoint, Project, RoleBinding,
};
use app_lib::db::{self, new_id, now, repo};
use app_lib::events::{sidecar_event_sink, EventEmitter, NullEmitter};
use app_lib::pipeline::PipelineDeps;
use app_lib::resources::ResourceGovernor;
use app_lib::scheduler::queue;
use app_lib::scheduler::WorkerPool;
use app_lib::sidecar::{SidecarClient, SpawnSpec, Supervisor};

/// Ids created by [`seed_project`].
pub struct Seeded {
    pub project_id: String,
    pub document_id: String,
    pub chapter_id: String,
}

/// A supervisor that is never started: these tests do not touch the sidecar.
pub fn unused_sidecar() -> SidecarClient {
    let emitter: Arc<dyn EventEmitter> = Arc::new(NullEmitter);
    let sink = sidecar_event_sink(emitter.clone());
    let supervisor = Supervisor::new(SpawnSpec::new("/bin/true", vec![]), sink, emitter, None);
    SidecarClient::new(supervisor)
}

/// Connect a fresh database inside `dir` and build the pipeline dependencies.
pub async fn deps_for(dir: &Path) -> Result<(SqlitePool, PipelineDeps)> {
    let pool = db::connect(&dir.join("app.sqlite")).await?;
    let deps = PipelineDeps::new(
        pool.clone(),
        unused_sidecar(),
        ResourceGovernor::default(),
        dir.to_path_buf(),
    );
    Ok((pool, deps))
}

/// Insert a project, a document, one chapter with three paragraphs and, when
/// asked, an endpoint bound to the `orchestrator` role.
pub async fn seed_project(
    pool: &SqlitePool,
    name: &str,
    base_url: &str,
    bind_orchestrator: bool,
) -> Result<Seeded> {
    let project_id = new_id();
    let timestamp = now();
    repo::insert_project(
        pool,
        &Project {
            id: project_id.clone(),
            name: format!("project-{name}"),
            source_path: "book.epub".to_string(),
            source_hash: "fixture-hash".to_string(),
            source_format: "epub".to_string(),
            source_lang: None,
            target_lang: "Italian".to_string(),
            doc_title: Some("The Lantern Keeper".to_string()),
            doc_author: Some("Fixture Author".to_string()),
            prompts_snapshot_dir: None,
            settings_json: "{}".to_string(),
            created_at: timestamp.clone(),
            updated_at: timestamp.clone(),
        },
    )
    .await?;

    let document_id = new_id();
    repo::insert_document(
        pool,
        &Document {
            id: document_id.clone(),
            project_id: project_id.clone(),
            markdown_path: "/tmp/book.md".to_string(),
            front_matter_json: r#"{"title":"The Lantern Keeper","author":"Fixture Author"}"#
                .to_string(),
            extractor: "epub".to_string(),
            extractor_version: "0".to_string(),
            created_at: timestamp.clone(),
        },
    )
    .await?;

    let chapter_id = new_id();
    repo::insert_chapter(
        pool,
        &Chapter {
            id: chapter_id.clone(),
            document_id: document_id.clone(),
            order_index: 1,
            title: "Chapter One".to_string(),
            level: 1,
            block_first: 1,
            block_last: 3,
            summary: None,
            summary_model: None,
            summary_hash: None,
            status: "pending".to_string(),
        },
    )
    .await?;

    for (index, text) in [
        "The harbour was quiet that morning.",
        "The keeper watched the light turn.",
        "Morning came later that year.",
    ]
    .iter()
    .enumerate()
    {
        repo::insert_block(
            pool,
            &Block {
                id: format!("b{:06}", index + 1),
                document_id: document_id.clone(),
                chapter_id: Some(chapter_id.clone()),
                order_index: index as i64 + 1,
                kind: "para".to_string(),
                level: 0,
                source_md: (*text).to_string(),
                source_text: (*text).to_string(),
                translatable: true,
                attrs_json: "{}".to_string(),
                content_hash: format!("h{index}"),
            },
        )
        .await?;
    }

    if bind_orchestrator {
        bind_role(pool, name, "orchestrator", base_url).await?;
    }

    Ok(Seeded {
        project_id,
        document_id,
        chapter_id,
    })
}

/// Create an endpoint pointing at `base_url` and bind `role` to it.
pub async fn bind_role(pool: &SqlitePool, name: &str, role: &str, base_url: &str) -> Result<()> {
    let endpoint_id = new_id();
    repo::upsert_endpoint(
        pool,
        &LlmEndpoint {
            id: endpoint_id.clone(),
            name: format!("{name}-{role}"),
            base_url: base_url.to_string(),
            api_key_ref: None,
            max_concurrency: Some(1),
            notes: None,
            last_health_at: None,
            last_health_ok: None,
            props_json: None,
        },
    )
    .await?;
    repo::upsert_role_binding(
        pool,
        &RoleBinding {
            id: new_id(),
            endpoint_id,
            role: role.to_string(),
            model: "fake-model".to_string(),
            params_json: "{}".to_string(),
            priority: 0,
        },
    )
    .await?;
    Ok(())
}

/// Insert one chunk of the chapter; `translated` marks it `done` with a
/// `target_md`, otherwise it stays `pending`.
pub async fn add_chunk(
    pool: &SqlitePool,
    document_id: &str,
    chapter_id: &str,
    order: i64,
    translated: Option<&str>,
) -> Result<String> {
    add_chunk_with_blocks(
        pool,
        document_id,
        chapter_id,
        order,
        &[format!("b{order:06}")],
        translated,
    )
    .await
}

/// Insert a chunk over an explicit list of blocks.
pub async fn add_chunk_with_blocks(
    pool: &SqlitePool,
    document_id: &str,
    chapter_id: &str,
    order: i64,
    block_ids: &[String],
    translated: Option<&str>,
) -> Result<String> {
    let chunk_id = format!("c{order:06}");
    let timestamp = now();
    repo::insert_chunk(
        pool,
        &Chunk {
            id: chunk_id.clone(),
            document_id: document_id.to_string(),
            chapter_id: Some(chapter_id.to_string()),
            order_index: order,
            block_ids_json: serde_json::to_string(block_ids)?,
            source_md: format!("Source paragraph {order}."),
            token_estimate: 20,
            context_json: "{}".to_string(),
            flags_json: "[]".to_string(),
            status: if translated.is_some() {
                "done"
            } else {
                "pending"
            }
            .to_string(),
            prompt_hash: None,
            model_id: None,
            params_json: None,
            context_manifest_json: None,
            target_md: translated.map(str::to_string),
            error: None,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        },
    )
    .await?;
    Ok(chunk_id)
}

/// Store a block translation.
pub async fn set_block_translation(
    pool: &SqlitePool,
    block_id: &str,
    chunk_id: &str,
    origin: &str,
    text: &str,
) -> Result<()> {
    repo::upsert_block_translation(
        pool,
        &BlockTranslation {
            block_id: block_id.to_string(),
            chunk_id: chunk_id.to_string(),
            text_md: text.to_string(),
            placeholders_ok: true,
            origin: origin.to_string(),
            edited_by_user: false,
            updated_at: now(),
        },
    )
    .await?;
    Ok(())
}

/// Mark a chunk `done` with a translation.
pub async fn set_chunk_done(pool: &SqlitePool, chunk_id: &str, target_md: &str) -> Result<()> {
    sqlx::query("UPDATE chunk SET status = 'done', target_md = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(chunk_id)
        .bind(target_md)
        .bind(now())
        .execute(pool)
        .await?;
    Ok(())
}

/// Poll a job until it reaches a terminal state and return that state. The
/// caller owns the worker and must cancel it.
pub async fn wait_for_job(pool: &SqlitePool, job_id: &str, worker: &WorkerPool) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let job = queue::get_job(pool, job_id)
            .await?
            .context("the job disappeared")?;
        if matches!(job.state.as_str(), "done" | "failed" | "cancelled") {
            return Ok(job.state);
        }
        if Instant::now() >= deadline {
            worker.cancel();
            bail!("job {job_id} did not finish in time");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
