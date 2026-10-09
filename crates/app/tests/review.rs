//! Review pipeline integration test (PLAN.md §11.4).
//!
//! The editor and proofreader passes run against a wiremock `llama-server`; the
//! accept path rewrites a block translation and recomposes the chunk without the
//! sidecar (the placeholder guard skips itself when the sidecar is not running).

mod common;

use anyhow::Result;
use serde_json::Value;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use app_lib::db::models::Suggestion;
use app_lib::db::{new_id, now, repo};
use app_lib::pipeline::{qa, review};

use common::{add_chunk_with_blocks, bind_role, deps_for, seed_project, set_block_translation};

const EDITOR_ANSWER: &str = r#"{"verdict":"needs_fix","issues":[{"block_index":0,"severity":"major","kind":"meaning","quote":"vecchio","suggested":"nuovo","reason":"the adjective is wrong"}]}"#;

fn sse_body(content: &str) -> String {
    sse_body_with(content, "stop")
}

fn sse_body_with(content: &str, finish_reason: &str) -> String {
    let chunk = serde_json::json!({ "choices": [{ "delta": { "content": content } }] });
    let finish = serde_json::json!({
        "choices": [{ "delta": {}, "finish_reason": finish_reason }],
        "usage": { "prompt_tokens": 5, "completion_tokens": 5 },
    });
    format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

fn sse_response(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

/// A reasoning model that spends its whole budget thinking: the stream carries
/// `reasoning_content`, stops on `length` and never emits an answer.
fn sse_reasoning_only(reasoning: &str) -> String {
    let thinking =
        serde_json::json!({ "choices": [{ "delta": { "reasoning_content": reasoning } }] });
    let finish = serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "length" }] });
    format!("data: {thinking}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

async fn mock_answer(content: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(sse_body(content)))
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

/// A proposal waiting for a decision, built by hand so a test can decide it.
fn pending_suggestion(
    chunk_id: &str,
    block_id: &str,
    pass: &str,
    quote: Option<&str>,
    proposed: &str,
) -> Suggestion {
    Suggestion {
        id: new_id(),
        chunk_id: chunk_id.to_string(),
        pass: pass.to_string(),
        block_id: Some(block_id.to_string()),
        field: Some("text".to_string()),
        original: Some("Il vecchio porto".to_string()),
        proposed: Some(proposed.to_string()),
        reason: Some("the adjective is wrong".to_string()),
        severity: Some("major".to_string()),
        quote: quote.map(str::to_string),
        status: "pending".to_string(),
        created_at: now(),
        decided_at: None,
    }
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
    assert!(accepted.decided_at.is_some(), "the decision is timestamped");
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

/// The proofreader answers span-level issues; a no-op one (the replacement equals the
/// quote) is dropped instead of reaching the review.
const PROOFREADER_ANSWER: &str = r#"{"issues":[{"block_index":0,"severity":"minor","kind":"punctuation","quote":"porto","suggested":"porto.","reason":"the sentence needs a full stop"},{"block_index":1,"severity":"minor","kind":"grammar","quote":"La nave","suggested":"La nave","reason":"nothing to change"}]}"#;

#[tokio::test]
async fn proofreader_pass_suggests_span_corrections() -> Result<()> {
    let server = mock_answer(PROOFREADER_ANSWER).await;
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let fixture = seed_reviewable(&pool, &server.uri()).await?;
    bind_role(&pool, "review", "proofreader", &server.uri()).await?;

    let created = review::run_proofread_chunk(&deps, None, &fixture.chunk_id).await?;
    assert_eq!(created, 1, "the no-op issue is not stored");

    let suggestions = repo::list_suggestions(&pool, &fixture.project_id, None, None, None).await?;
    assert_eq!(suggestions.len(), 1);
    let suggestion = &suggestions[0];
    assert_eq!(suggestion.pass, review::PROOFREAD_ROLE);
    assert_eq!(suggestion.block_id.as_deref(), Some("b000001"));
    assert_eq!(suggestion.original.as_deref(), Some("Il vecchio porto"));
    assert_eq!(suggestion.quote.as_deref(), Some("porto"));
    assert_eq!(suggestion.proposed.as_deref(), Some("porto."));
    assert_eq!(suggestion.severity.as_deref(), Some("minor"));
    assert_eq!(
        suggestion.reason.as_deref(),
        Some("the sentence needs a full stop")
    );

    // The request used the structured path and numbered the blocks.
    let requests = server.received_requests().await.expect("requests");
    let body: Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        "proofreader"
    );
    let user = body["messages"][1]["content"].as_str().unwrap_or_default();
    assert!(user.contains("[0] Il vecchio porto"));

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
    assert!(rejected.decided_at.is_some(), "the decision is timestamped");
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

/// The decisions of a project read back as its correction history: accepted and
/// rejected proposals, newest decision first, with the explanation still attached.
#[tokio::test]
async fn decided_suggestions_read_back_as_the_project_history() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let fixture = seed_reviewable(&pool, "http://127.0.0.1:1").await?;

    let accepted = pending_suggestion(
        &fixture.chunk_id,
        "b000001",
        "editor",
        Some("vecchio"),
        "nuovo",
    );
    let rejected = pending_suggestion(
        &fixture.chunk_id,
        "b000002",
        "editor",
        Some("nave"),
        "barca",
    );
    let pending = pending_suggestion(
        &fixture.chunk_id,
        "b000001",
        "proofreader",
        None,
        "Il vecchio porto.",
    );
    for suggestion in [&accepted, &rejected, &pending] {
        repo::insert_suggestion(&pool, suggestion).await?;
    }

    review::accept_suggestion(&deps, &accepted.id).await?;
    review::reject_suggestion(&pool, &rejected.id).await?;

    // Both decisions land in the same millisecond, so pin the times the order reads.
    for (id, decided_at) in [
        (&accepted.id, "2026-01-01T10:00:00.000Z"),
        (&rejected.id, "2026-01-02T10:00:00.000Z"),
    ] {
        sqlx::query("UPDATE suggestion SET decided_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(decided_at)
            .execute(&pool)
            .await?;
    }

    let history =
        repo::list_decided_suggestions(&pool, &fixture.project_id, None, None, None, 100).await?;
    assert_eq!(history.len(), 2, "an undecided proposal is not history");
    assert_eq!(history[0].id, rejected.id, "newest decision first");
    assert_eq!(history[1].id, accepted.id);
    assert_eq!(history[1].reason.as_deref(), Some("the adjective is wrong"));

    let accepted_only = repo::list_decided_suggestions(
        &pool,
        &fixture.project_id,
        None,
        None,
        Some("accepted"),
        100,
    )
    .await?;
    assert_eq!(accepted_only.len(), 1);
    assert_eq!(accepted_only[0].id, accepted.id);

    let newest =
        repo::list_decided_suggestions(&pool, &fixture.project_id, None, None, None, 1).await?;
    assert_eq!(newest.len(), 1, "the limit keeps the newest decisions");
    assert_eq!(newest[0].id, rejected.id);
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

/// A reasoning model that thought its way through the whole budget answers no JSON:
/// the pass retries once, and the retry asks for a larger budget than the default.
#[tokio::test]
async fn editor_retries_an_answer_without_json_and_asks_for_more_room() -> Result<()> {
    let server = MockServer::start().await;
    // First attempt: the 1500 token default, spent on the thinking.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"max_tokens\":1500"))
        .respond_with(sse_response(sse_reasoning_only("weighing the passage")))
        .mount(&server)
        .await;
    // The retry carries the instruction, so the two mocks never overlap.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("contained no JSON object"))
        .respond_with(sse_response(sse_body_with(EDITOR_ANSWER, "stop")))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let fixture = seed_reviewable(&pool, &server.uri()).await?;
    bind_role(&pool, "review", "editor", &server.uri()).await?;

    let created = review::run_edit_chunk(&deps, None, &fixture.chunk_id).await?;
    assert_eq!(created, 1, "the retried answer is used");

    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2);
    let retry: Value = serde_json::from_slice(&requests[1].body)?;
    assert_eq!(retry["max_tokens"], 3000);
    let user = retry["messages"][1]["content"].as_str().unwrap_or_default();
    assert!(user.contains("contained no JSON object"));

    // Both calls stay in the audit trail, the thinking included.
    let calls: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT reasoning_text, response_text FROM llm_call WHERE chunk_id = ?1 \
         ORDER BY created_at",
    )
    .bind(&fixture.chunk_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0.as_deref(), Some("weighing the passage"));
    assert_eq!(calls[0].1.as_deref(), Some(""));
    Ok(())
}

/// When even the retry comes back without an answer, the error says why: the stop
/// reason, the sizes and the budget, so the cause is visible without the text.
#[tokio::test]
async fn editor_reports_why_the_answer_was_empty() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(sse_reasoning_only("weighing the passage")))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let fixture = seed_reviewable(&pool, &server.uri()).await?;
    bind_role(&pool, "review", "editor", &server.uri()).await?;

    let error = review::run_edit_chunk(&deps, None, &fixture.chunk_id)
        .await
        .expect_err("an answer with no JSON object fails the pass");
    let message = error.to_string();
    assert!(
        message.contains("the review answer contains no JSON object"),
        "{message}"
    );
    assert!(message.contains("finish_reason=length"), "{message}");
    assert!(message.contains("reasoning=20 chars"), "{message}");
    assert!(message.contains("max_tokens=3000"), "{message}");
    assert!(message.contains("retried once"), "{message}");
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
