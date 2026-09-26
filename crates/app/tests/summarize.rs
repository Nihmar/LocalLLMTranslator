//! Rolling chapter memory integration test (PLAN.md section 8).
//!
//! The enqueue decision (`maybe_enqueue_summaries`) and the job itself
//! (`run_summarize`) run against a real SQLite database and a wiremock
//! `llama-server`, with chunks inserted directly: the summarizer consumes
//! `chunk.target_md`, so the translation half of the pipeline is not needed to
//! test it.

mod common;

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use app_lib::db::models::{GlossaryTerm, Job};
use app_lib::db::{new_id, repo};
use app_lib::error::Result as AppResult;
use app_lib::events::{EventEmitter, NullEmitter};
use app_lib::pipeline::summarize::{self, SummarizePayload};
use app_lib::pipeline::PipelineDeps;
use app_lib::scheduler::WorkerPool;

use common::{add_chunk, deps_for, seed_project, wait_for_job};

const SUMMARY_TEXT: &str = "The keeper wakes before dawn and watches the light turn.";

fn summary_json() -> String {
    serde_json::json!({
        "summary": SUMMARY_TEXT,
        "new_terms": [
            { "source": "keeper", "target": "guardiano", "kind": "term", "note": "recurring role" },
            { "source": "Harbour Light", "target": "Harbour Light", "kind": "do_not_translate", "note": "lighthouse" },
        ],
        "style_notes": ["Long, flowing sentences."],
    })
    .to_string()
}

fn sse_body() -> String {
    let content = serde_json::json!({ "choices": [{ "delta": { "content": summary_json() } }] });
    let finish = serde_json::json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5 },
    });
    format!("data: {content}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

async fn mock_summary_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body()),
        )
        .mount(&server)
        .await;
    server
}

struct SummaryDispatcher {
    deps: PipelineDeps,
}

#[async_trait]
impl app_lib::scheduler::JobDispatcher for SummaryDispatcher {
    async fn dispatch(&self, job: &Job) -> AppResult<()> {
        assert_eq!(job.kind, summarize::JOB_KIND);
        let payload: SummarizePayload = serde_json::from_str(&job.payload_json)?;
        summarize::run_summarize(&self.deps, Some(&job.id), &job.project_id, &payload).await?;
        Ok(())
    }
}

fn pool_for(deps: &PipelineDeps) -> WorkerPool {
    let emitter: Arc<dyn EventEmitter> = Arc::new(NullEmitter);
    WorkerPool::new(
        deps.pool.clone(),
        Arc::new(SummaryDispatcher { deps: deps.clone() }),
        emitter,
        1,
        1,
    )
}

#[tokio::test]
async fn rolling_summary_lands_in_project_memory_and_candidates() -> Result<()> {
    let server = mock_summary_server().await;
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let seeded = seed_project(&pool, "rolling", &server.uri(), true).await?;

    // Five completed chunks (the rolling cadence) and one still pending.
    for order in 1..=5 {
        add_chunk(
            &pool,
            &seeded.document_id,
            &seeded.chapter_id,
            order,
            Some(&format!("Translated paragraph {order}.")),
        )
        .await?;
    }
    add_chunk(&pool, &seeded.document_id, &seeded.chapter_id, 6, None).await?;

    let job_id = summarize::maybe_enqueue_summaries(&pool, &seeded.project_id, &seeded.chapter_id)
        .await?
        .expect("a rolling summary is due after five chunks");
    // An equivalent pending job suppresses a second enqueue.
    assert_eq!(
        summarize::maybe_enqueue_summaries(&pool, &seeded.project_id, &seeded.chapter_id).await?,
        None
    );

    let worker = pool_for(&deps);
    worker.start();
    let state = wait_for_job(&pool, &job_id, &worker).await?;
    worker.cancel();
    assert_eq!(state, "done");

    assert_eq!(
        repo::get_memory(&pool, &seeded.project_id, summarize::ROLLING_SUMMARY_KEY)
            .await?
            .as_deref(),
        Some(SUMMARY_TEXT)
    );
    // A rolling run does not close the chapter.
    let chapter = repo::get_chapter(&pool, &seeded.chapter_id)
        .await?
        .expect("chapter");
    assert!(chapter.summary.is_none());

    let terms = repo::list_glossary_terms(&pool, &seeded.project_id).await?;
    assert_eq!(terms.len(), 2);
    assert!(terms
        .iter()
        .all(|term| term.status == "candidate" && term.origin == "proposed"));
    assert!(terms
        .iter()
        .any(|term| term.source == "keeper" && term.target == "guardiano" && term.kind == "term"));

    let snapshot = app_lib::pipeline::recon::snapshot(&pool, &seeded.project_id).await?;
    assert_eq!(snapshot.style_notes, vec!["Long, flowing sentences."]);
    Ok(())
}

#[tokio::test]
async fn final_summary_closes_the_chapter_and_never_demotes_a_term() -> Result<()> {
    let server = mock_summary_server().await;
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let seeded = seed_project(&pool, "final", &server.uri(), true).await?;

    // An approved rendering that the summarizer proposes again: it must survive.
    repo::upsert_glossary_term(
        &pool,
        &GlossaryTerm {
            id: new_id(),
            project_id: seeded.project_id.clone(),
            source_lang: Some("English".to_string()),
            target_lang: Some("Italian".to_string()),
            source: "keeper".to_string(),
            target: "custode".to_string(),
            note: None,
            kind: "term".to_string(),
            origin: "manual".to_string(),
            revision: 1,
            status: "approved".to_string(),
        },
    )
    .await?;

    for order in 1..=3 {
        add_chunk(
            &pool,
            &seeded.document_id,
            &seeded.chapter_id,
            order,
            Some(&format!("Translated paragraph {order}.")),
        )
        .await?;
    }

    let job_id = summarize::maybe_enqueue_summaries(&pool, &seeded.project_id, &seeded.chapter_id)
        .await?
        .expect("the final summary is due when no chunk is unresolved");
    let worker = pool_for(&deps);
    worker.start();
    let state = wait_for_job(&pool, &job_id, &worker).await?;
    worker.cancel();
    assert_eq!(state, "done");

    let chapter = repo::get_chapter(&pool, &seeded.chapter_id)
        .await?
        .expect("chapter");
    assert_eq!(chapter.summary.as_deref(), Some(SUMMARY_TEXT));
    assert_eq!(chapter.summary_model.as_deref(), Some("fake-model"));
    assert!(chapter.summary_hash.is_some_and(|hash| !hash.is_empty()));
    assert_eq!(chapter.status, "done");
    // The chapter summary takes over, so the rolling one is cleared.
    assert_eq!(
        repo::get_memory(&pool, &seeded.project_id, summarize::ROLLING_SUMMARY_KEY)
            .await?
            .as_deref(),
        Some("")
    );

    let terms = repo::list_glossary_terms(&pool, &seeded.project_id).await?;
    let keeper = terms
        .iter()
        .find(|term| term.source == "keeper")
        .expect("keeper");
    assert_eq!(
        keeper.target, "custode",
        "an approved term is never demoted"
    );
    assert_eq!(keeper.status, "approved");
    assert_eq!(
        terms.iter().filter(|term| term.source == "keeper").count(),
        1,
        "the proposal must not duplicate the existing term"
    );
    assert!(terms.iter().any(|term| term.source == "Harbour Light"));
    Ok(())
}

#[tokio::test]
async fn without_orchestrator_binding_no_summary_job_is_enqueued() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (pool, _deps) = deps_for(dir.path()).await?;
    let seeded = seed_project(&pool, "unbound", "http://127.0.0.1:1", false).await?;

    for order in 1..=5 {
        add_chunk(
            &pool,
            &seeded.document_id,
            &seeded.chapter_id,
            order,
            Some("Translated paragraph."),
        )
        .await?;
    }

    assert_eq!(
        summarize::maybe_enqueue_summaries(&pool, &seeded.project_id, &seeded.chapter_id).await?,
        None
    );
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job WHERE kind = ?1")
        .bind(summarize::JOB_KIND)
        .fetch_one(&pool)
        .await?;
    assert_eq!(jobs, 0);
    Ok(())
}

/// The dispatcher parses the payload shape `maybe_enqueue_summaries` writes, so
/// the two cannot drift.
#[tokio::test]
async fn the_payload_round_trips() -> Result<()> {
    let payload = SummarizePayload {
        chapter_id: "c1".to_string(),
        final_run: true,
    };
    let json = serde_json::to_string(&payload)?;
    assert!(json.contains("\"final\":true"));
    let parsed: SummarizePayload = serde_json::from_str(&json)?;
    assert_eq!(parsed.chapter_id, "c1");
    assert!(parsed.final_run);
    let defaulted: SummarizePayload =
        serde_json::from_value(serde_json::json!({"chapter_id":"c1"}))?;
    assert!(!defaulted.final_run);
    Ok(())
}
