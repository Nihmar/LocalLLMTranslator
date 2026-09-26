//! Review pipeline integration test (PLAN.md §11.4).
//!
//! The editor and proofreader passes run against a wiremock `llama-server`; the
//! accept path rewrites a block translation and recomposes the chunk without the
//! sidecar (the placeholder guard skips itself when the sidecar is not running).

mod common;

use anyhow::Result;
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use app_lib::db::repo;
use app_lib::pipeline::{qa, review};

use common::{add_chunk_with_blocks, bind_role, deps_for, seed_project, set_block_translation};

const EDITOR_ANSWER: &str = r#"{"verdict":"needs_fix","issues":[{"block_index":0,"severity":"major","kind":"meaning","quote":"vecchio","suggested":"nuovo","reason":"the adjective is wrong"}]}"#;

fn sse_body(content: &str) -> String {
    let chunk = serde_json::json!({ "choices": [{ "delta": { "content": content } }] });
    let finish = serde_json::json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 5, "completion_tokens": 5 },
    });
    format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

async fn mock_answer(content: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(content)),
        )
        .mount(&server)
        .await;
    server
}

struct Fixture {
    project_id: String,
    chunk_id: String,
}

/// One chunk with two translated blocks.
async fn seed_reviewable(pool: &sqlx::SqlitePool, base_url: &str) -> Result<Fixture> {
    let seeded = seed_project(pool, "review", base_url, false).await?;
    let chunk_id = add_chunk_with_blocks(
        pool,
        &seeded.document_id,
        &seeded.chapter_id,
        1,
        &["b000001".to_string(), "b000002".to_string()],
        Some("Il vecchio porto\n\nLa nave"),
    )
    .await?;
    set_block_translation(pool, "b000001", &chunk_id, "translator", "Il vecchio porto").await?;
    set_block_translation(pool, "b000002", &chunk_id, "translator", "La nave").await?;
    Ok(Fixture {
        project_id: seeded.project_id,
        chunk_id,
    })
}

#[tokio::test]
async fn editor_pass_creates_a_suggestion_and_accept_rewrites_the_block() -> Result<()> {
    let server = mock_answer(EDITOR_ANSWER).await;
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let fixture = seed_reviewable(&pool, &server.uri()).await?;
    bind_role(&pool, "review", "editor", &server.uri()).await?;

    let created = review::run_edit_chunk(&deps, None, &fixture.chunk_id).await?;
    assert_eq!(created, 1);

    let suggestions = repo::list_suggestions(&pool, &fixture.project_id, None, None, None).await?;
    assert_eq!(suggestions.len(), 1);
    let suggestion = &suggestions[0];
    assert_eq!(suggestion.pass, review::EDIT_ROLE);
    assert_eq!(suggestion.status, "pending");
    assert_eq!(suggestion.block_id.as_deref(), Some("b000001"));
    assert_eq!(suggestion.quote.as_deref(), Some("vecchio"));
    assert_eq!(suggestion.proposed.as_deref(), Some("nuovo"));
    assert_eq!(suggestion.severity.as_deref(), Some("major"));
    assert_eq!(suggestion.original.as_deref(), Some("Il vecchio porto"));

    // The request used the structured path and numbered the blocks.
    let requests = server.received_requests().await.expect("requests");
    let body: Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(body["response_format"]["json_schema"]["name"], "editor");
    let user = body["messages"][1]["content"].as_str().unwrap_or_default();
    assert!(user.contains("[0]"));
    assert!(user.contains("[1]"));

    // Accept: the block is rewritten with the editor origin and the chunk is
    // recomposed, so an export right after sees the fix.
    let accepted = review::accept_suggestion(&deps, &suggestion.id).await?;
    assert_eq!(accepted.status, "accepted");
    let translations = repo::list_block_translations(&pool, &fixture.chunk_id).await?;
    let edited = translations
        .iter()
        .find(|row| row.origin == review::EDIT_ROLE)
        .expect("the editor row");
    assert_eq!(edited.text_md, "Il nuovo porto");
    let chunk = repo::get_chunk(&pool, &fixture.chunk_id)
        .await?
        .expect("chunk");
    assert_eq!(
        chunk.target_md.as_deref(),
        Some("Il nuovo porto\n\nLa nave")
    );
    Ok(())
}

#[tokio::test]
async fn proofreader_pass_suggests_corrected_blocks() -> Result<()> {
    let server = mock_answer("Il vecchio porto.\n\n<!-- block -->\n\nLa nave").await;
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let fixture = seed_reviewable(&pool, &server.uri()).await?;
    bind_role(&pool, "review", "proofreader", &server.uri()).await?;

    let created = review::run_proofread_chunk(&deps, None, &fixture.chunk_id).await?;
    assert_eq!(created, 1);

    let suggestions = repo::list_suggestions(&pool, &fixture.project_id, None, None, None).await?;
    assert_eq!(suggestions.len(), 1);
    let suggestion = &suggestions[0];
    assert_eq!(suggestion.pass, review::PROOFREAD_ROLE);
    assert_eq!(suggestion.block_id.as_deref(), Some("b000001"));
    assert_eq!(suggestion.original.as_deref(), Some("Il vecchio porto"));
    assert_eq!(suggestion.proposed.as_deref(), Some("Il vecchio porto."));

    review::accept_suggestion(&deps, &suggestion.id).await?;
    let translations = repo::list_block_translations(&pool, &fixture.chunk_id).await?;
    let polished = translations
        .iter()
        .find(|row| row.origin == review::PROOFREAD_ROLE)
        .expect("the proofreader row");
    assert_eq!(polished.text_md, "Il vecchio porto.");
    Ok(())
}

#[tokio::test]
async fn reject_marks_the_suggestion_without_touching_the_text() -> Result<()> {
    let server = mock_answer(EDITOR_ANSWER).await;
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let fixture = seed_reviewable(&pool, &server.uri()).await?;
    bind_role(&pool, "review", "editor", &server.uri()).await?;
    review::run_edit_chunk(&deps, None, &fixture.chunk_id).await?;

    let suggestion = repo::list_suggestions(&pool, &fixture.project_id, None, None, None)
        .await?
        .into_iter()
        .next()
        .expect("a suggestion");
    let rejected = review::reject_suggestion(&pool, &suggestion.id).await?;
    assert_eq!(rejected.status, "rejected");
    let chunk = repo::get_chunk(&pool, &fixture.chunk_id)
        .await?
        .expect("chunk");
    assert_eq!(
        chunk.target_md.as_deref(),
        Some("Il vecchio porto\n\nLa nave")
    );

    // Rejecting twice is idempotent.
    let again = review::reject_suggestion(&pool, &suggestion.id).await?;
    assert_eq!(again.status, "rejected");
    Ok(())
}

#[tokio::test]
async fn review_start_enqueues_each_pass_once() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let fixture = seed_reviewable(&pool, "http://127.0.0.1:1").await?;
    let _ = &deps;

    let first =
        review::enqueue_review_jobs(&pool, &fixture.project_id, None, None, "both", true).await?;
    assert_eq!(first.len(), 3, "editor + proofreader + qa_scan");

    let second =
        review::enqueue_review_jobs(&pool, &fixture.project_id, None, None, "both", true).await?;
    assert!(second.is_empty(), "pending jobs must suppress duplicates");

    let kinds: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COUNT(*) FROM job WHERE project_id = ?1 GROUP BY kind ORDER BY kind",
    )
    .bind(&fixture.project_id)
    .fetch_all(&pool)
    .await?;
    let counts: std::collections::BTreeMap<String, i64> = kinds.into_iter().collect();
    assert_eq!(counts.get(review::EDIT_JOB), Some(&1));
    assert_eq!(counts.get(review::PROOFREAD_JOB), Some(&1));
    assert_eq!(counts.get(qa::JOB_KIND), Some(&1));
    Ok(())
}

/// A pending chunk is not reviewable: no translation, no pass.
#[tokio::test]
async fn review_start_skips_untranslated_chunks() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (pool, _deps) = deps_for(dir.path()).await?;
    let seeded = seed_project(&pool, "pending", "http://127.0.0.1:1", false).await?;
    common::add_chunk(&pool, &seeded.document_id, &seeded.chapter_id, 1, None).await?;

    let jobs =
        review::enqueue_review_jobs(&pool, &seeded.project_id, None, None, "editor", false).await?;
    assert!(jobs.is_empty());
    Ok(())
}
