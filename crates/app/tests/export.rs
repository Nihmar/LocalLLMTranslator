//! Export guard: a book is never exported half in the source language by accident.
//!
//! The refusal happens before the sidecar and Pandoc are involved, so the test needs
//! neither.

mod common;

use anyhow::Result;

use app_lib::error::AppError;
use app_lib::pipeline::export::{run_export, ExportRequest};

use common::{add_chunk, deps_for, seed_project};

#[tokio::test]
async fn untranslated_chunks_block_the_export_until_the_user_confirms() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let seeded = seed_project(&pool, "mixed", "http://127.0.0.1:9", false).await?;
    add_chunk(
        &pool,
        &seeded.document_id,
        &seeded.chapter_id,
        1,
        Some("Translated."),
    )
    .await?;
    add_chunk(&pool, &seeded.document_id, &seeded.chapter_id, 2, None).await?;

    let request = ExportRequest {
        project_id: seeded.project_id.clone(),
        output_format: "epub".to_string(),
        output_path: None,
        template: None,
        css: None,
        toc: true,
        chapter_id: None,
        force: false,
        allow_untranslated: false,
    };
    match run_export(&deps, &request).await {
        Err(AppError::Invalid(message)) => {
            assert!(
                message.contains("1 of 2 chunks"),
                "unexpected message: {message}"
            );
        }
        other => panic!("a half-translated export must be refused, got {other:?}"),
    }
    Ok(())
}
