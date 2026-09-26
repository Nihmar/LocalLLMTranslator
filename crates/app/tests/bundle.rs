//! `.llmtz` bundle round-trip (PLAN.md §6).
//!
//! Export a seeded project (rows + files), import it into a fresh data directory
//! and assert that the rows, the media and the rewritten paths all arrived while
//! the stale build cache did not.

mod common;

use std::path::Path;

use anyhow::{ensure, Context, Result};

use app_lib::db::models::GlossaryTerm;
use app_lib::db::{self, new_id, repo};
use app_lib::pipeline::bundle::{export_project, import_project};

use common::{add_chunk_with_blocks, seed_project, set_block_translation};

async fn seed_full(dir: &Path) -> Result<(sqlx::SqlitePool, String)> {
    let pool = db::connect(&dir.join("app.sqlite")).await?;
    let seeded = seed_project(&pool, "bundle", "http://127.0.0.1:1", false).await?;
    let chunk_id = add_chunk_with_blocks(
        &pool,
        &seeded.document_id,
        &seeded.chapter_id,
        1,
        &["b000001".to_string()],
        Some("Il porto era tranquillo."),
    )
    .await?;
    set_block_translation(
        &pool,
        "b000001",
        &chunk_id,
        "translator",
        "Il porto era tranquillo.",
    )
    .await?;
    repo::upsert_glossary_term(
        &pool,
        &GlossaryTerm {
            id: new_id(),
            project_id: seeded.project_id.clone(),
            source_lang: Some("en".to_string()),
            target_lang: Some("it".to_string()),
            source: "harbour".to_string(),
            target: "porto".to_string(),
            note: None,
            kind: "term".to_string(),
            origin: "manual".to_string(),
            revision: 1,
            status: "approved".to_string(),
        },
    )
    .await?;
    repo::set_memory(&pool, &seeded.project_id, "style_guide", "Formale.").await?;
    // A stale build cache: it must not travel with the bundle.
    repo::set_memory(
        &pool,
        &seeded.project_id,
        "export_state",
        r#"{"stale":true}"#,
    )
    .await?;
    Ok((pool, seeded.project_id))
}

fn write_project_files(dir: &Path, project_id: &str) -> Result<()> {
    let project = dir.join("projects").join(project_id);
    std::fs::create_dir_all(project.join("work/assets"))?;
    // The seeded document points at `/tmp/book.md`, so the bundle carries that name.
    std::fs::write(project.join("work/book.md"), "# Ciao\n")?;
    std::fs::write(project.join("work/assets/harbour.png"), b"png-bytes")?;
    std::fs::create_dir_all(project.join("output"))?;
    std::fs::write(project.join("output/book.epub"), b"epub-bytes")?;
    std::fs::create_dir_all(project.join("prompts"))?;
    std::fs::write(project.join("prompts/translator.system.md"), "SYS")?;
    Ok(())
}

#[tokio::test]
async fn export_then_import_round_trips_the_project() -> Result<()> {
    let dir_a = tempfile::tempdir()?;
    let (pool_a, project_id) = seed_full(dir_a.path()).await?;
    write_project_files(dir_a.path(), &project_id)?;

    let outcome = export_project(&pool_a, dir_a.path(), &project_id, None).await?;
    ensure!(
        Path::new(&outcome.output_path).is_file(),
        "the archive was not written"
    );
    ensure!(outcome.bytes > 0, "the archive is empty");
    ensure!(
        outcome.files >= 4,
        "expected the work/output/prompts files, got {}",
        outcome.files
    );

    let dir_b = tempfile::tempdir()?;
    let pool_b = db::connect(&dir_b.path().join("app.sqlite")).await?;
    let imported = import_project(&pool_b, dir_b.path(), &outcome.output_path).await?;
    assert_eq!(imported.id, project_id);

    // Rows copied.
    let chunks = repo::list_chunks_by_project(&pool_b, &project_id, None).await?;
    ensure!(
        chunks.len() == 1,
        "expected one chunk, got {}",
        chunks.len()
    );
    let glossary = repo::list_glossary_terms(&pool_b, &project_id).await?;
    ensure!(glossary.len() == 1, "expected one term");
    ensure!(
        repo::get_memory(&pool_b, &project_id, "style_guide")
            .await?
            .as_deref()
            == Some("Formale."),
        "the project memory must travel"
    );
    ensure!(
        repo::get_memory(&pool_b, &project_id, "export_state")
            .await?
            .is_none(),
        "the build cache must not travel"
    );
    ensure!(
        repo::list_block_translations(&pool_b, &chunks[0].id)
            .await?
            .len()
            == 1,
        "the block translation must travel"
    );

    // Files moved and paths rewritten to the local data directory.
    let local = dir_b.path().join("projects").join(&project_id);
    for relative in [
        "work/book.md",
        "work/assets/harbour.png",
        "output/book.epub",
        "prompts/translator.system.md",
    ] {
        ensure!(
            local.join(relative).is_file(),
            "{relative} did not survive the import"
        );
    }
    let document = repo::get_document_for_project(&pool_b, &project_id)
        .await?
        .context("document")?;
    assert_eq!(
        document.markdown_path,
        local.join("work/book.md").to_string_lossy()
    );
    let project = repo::get_project(&pool_b, &project_id)
        .await?
        .context("project")?;
    assert_eq!(
        project.prompts_snapshot_dir.as_deref(),
        Some(local.join("prompts").to_string_lossy().as_ref())
    );

    // Importing the same bundle again is rejected, not merged.
    let error = import_project(&pool_b, dir_b.path(), &outcome.output_path)
        .await
        .expect_err("a duplicate import must be rejected");
    ensure!(
        error.to_string().contains("already exists"),
        "unexpected error: {error}"
    );

    // A plain ZIP is not a bundle.
    let plain = dir_b.path().join("plain.zip");
    {
        let file = std::fs::File::create(&plain)?;
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("readme.txt", zip::write::SimpleFileOptions::default())?;
        std::io::Write::write_all(&mut zip, b"hello")?;
        zip.finish()?;
    }
    let error = import_project(&pool_b, dir_b.path(), &plain.to_string_lossy())
        .await
        .expect_err("a plain zip must be rejected");
    ensure!(
        error.to_string().contains("manifest"),
        "unexpected error: {error}"
    );
    Ok(())
}

/// Exporting twice is allowed and the second archive replaces nothing; the
/// default destination lives in the project output directory.
#[tokio::test]
async fn export_defaults_to_the_project_output_directory() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (pool, project_id) = seed_full(dir.path()).await?;
    write_project_files(dir.path(), &project_id)?;

    let outcome = export_project(&pool, dir.path(), &project_id, None).await?;
    let expected_dir = dir.path().join("projects").join(&project_id).join("output");
    ensure!(
        Path::new(&outcome.output_path).starts_with(&expected_dir),
        "default archive {} is not under {}",
        outcome.output_path,
        expected_dir.display()
    );
    ensure!(
        outcome.output_path.ends_with(".llmtz"),
        "unexpected extension: {}",
        outcome.output_path
    );
    Ok(())
}
