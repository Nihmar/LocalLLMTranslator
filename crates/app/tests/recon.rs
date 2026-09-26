//! Book reconnaissance integration test (PLAN.md section 9.4).
//!
//! `run_recon` and `confirm` run against a real SQLite database and a wiremock
//! `llama-server`; the assertions end where the plan says they must: the
//! confirmed values are exactly what the context builder injects into every
//! translation prompt.
//!
//! wiremock rather than `tools/fake_llama_server.py` on purpose: the fake
//! server's `book_profile` branch has its own pytest suite, while this test
//! pins the control plane side (schema request, SSE parse, clamping,
//! persistence, glossary reuse) without a child process.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use app_lib::context::budget::HeuristicCounter;
use app_lib::context::builder::{ContextBuilder, ContextInputs};
use app_lib::db::models::{Block, Chapter, Document, Job, LlmEndpoint, Project, RoleBinding};
use app_lib::db::{self, new_id, now, repo};
use app_lib::error::{AppError, Result as AppResult};
use app_lib::events::{sidecar_event_sink, EventEmitter, NullEmitter};
use app_lib::pipeline::recon::{self, ConfirmRequest, ConfirmedTerm, MEMORY_KEY, META_KEY};
use app_lib::pipeline::PipelineDeps;
use app_lib::resources::ResourceGovernor;
use app_lib::scheduler::queue::{self, NewJob};
use app_lib::scheduler::{JobDispatcher, WorkerPool};
use app_lib::sidecar::{SidecarClient, SpawnSpec, Supervisor};

/// The deterministic candidate the mock model returns, shaped like
/// `prompts/analyze_book.schema.json`.
const PROFILE_JSON: &str = r#"{"source_language":"English","genre":"literary fiction","audience":"adult readers","era":"late nineteenth century","narrative_voice":"third-person limited","register":"formal, slightly archaic","style_notes":["Prefer period vocabulary."],"themes":["loyalty"],"synopsis":"A harbour keeper guards his light.","proper_nouns":[{"source":"Harbour Light","kind":"do_not_translate","note":"lighthouse"},{"source":"Elena","kind":"proper_noun","note":"the keeper's daughter"}],"field_basis":{"genre":"from_text","synopsis":"from_text"}}"#;

fn sse_body() -> String {
    let content = serde_json::json!({ "choices": [{ "delta": { "content": PROFILE_JSON } }] });
    let finish = serde_json::json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5 },
    });
    format!("data: {content}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

/// A supervisor that is never started: `run_recon` does not touch the sidecar.
fn unused_sidecar() -> SidecarClient {
    let emitter: Arc<dyn EventEmitter> = Arc::new(NullEmitter);
    let sink = sidecar_event_sink(emitter.clone());
    let supervisor = Supervisor::new(SpawnSpec::new("/bin/true", vec![]), sink, emitter, None);
    SidecarClient::new(supervisor)
}

async fn seed_project(
    pool: &SqlitePool,
    base_url: &str,
    bind_orchestrator: bool,
) -> Result<String> {
    let project_id = new_id();
    let timestamp = now();
    repo::insert_project(
        pool,
        &Project {
            id: project_id.clone(),
            name: "recon-project".to_string(),
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
        let endpoint_id = new_id();
        repo::upsert_endpoint(
            pool,
            &LlmEndpoint {
                id: endpoint_id.clone(),
                name: "fake-orchestrator".to_string(),
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
                role: "orchestrator".to_string(),
                model: "fake-model".to_string(),
                params_json: "{}".to_string(),
                priority: 0,
            },
        )
        .await?;
    }

    Ok(project_id)
}

async fn deps_for(dir: &std::path::Path) -> Result<(SqlitePool, PipelineDeps)> {
    let pool = db::connect(&dir.join("app.sqlite")).await?;
    let deps = PipelineDeps::new(
        pool.clone(),
        unused_sidecar(),
        ResourceGovernor::default(),
        dir.to_path_buf(),
    );
    Ok((pool, deps))
}

#[tokio::test]
async fn reconnaissance_produces_a_candidate_the_user_confirms() -> Result<()> {
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

    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let project_id = seed_project(&pool, &server.uri(), true).await?;

    // ---- run -----------------------------------------------------------------
    let outcome = recon::run_recon(&deps, None, &project_id, Some("User-pasted page.")).await?;
    assert_eq!(outcome.model, "fake-model");
    assert_eq!(outcome.proper_nouns, 2);
    // Two paragraphs per chapter, and the fixture has one chapter.
    assert_eq!(outcome.excerpt_blocks, 2);

    let snapshot = recon::snapshot(&pool, &project_id).await?;
    let profile = snapshot.profile.clone().expect("a candidate profile");
    assert_eq!(profile.source_language.value, "English");
    assert_eq!(profile.genre.basis, "from_text");
    assert_eq!(profile.synopsis.basis, "from_text");
    assert_eq!(profile.register.basis, "inferred");
    assert_eq!(
        profile.provenance.pasted_chars,
        "User-pasted page.".chars().count()
    );
    assert!(profile.provenance.metadata);
    assert!(snapshot.orchestrator_bound);
    assert!(snapshot.style_guide.is_empty(), "nothing is confirmed yet");

    // The request really used the structured-output path and carried the local
    // evidence (never a fetched page).
    let requests = server
        .received_requests()
        .await
        .expect("the mock records requests");
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        "book_profile"
    );
    let system = body["messages"][0]["content"].as_str().unwrap_or_default();
    let user = body["messages"][1]["content"].as_str().unwrap_or_default();
    assert!(system.contains("Italian"));
    assert!(user.contains("User-pasted page."));
    assert!(user.contains("The harbour was quiet that morning."));
    assert!(user.contains("Fixture Author"));

    // ---- confirm -------------------------------------------------------------
    let confirmed_fields = vec![
        "source_language".to_string(),
        "genre".to_string(),
        "synopsis".to_string(),
    ];
    let terms: Vec<ConfirmedTerm> = profile
        .proper_nouns
        .iter()
        .map(|term| ConfirmedTerm {
            source: term.source.clone(),
            target: None,
            kind: term.kind.clone(),
            note: Some(term.note.clone()),
        })
        .collect();
    let style_guide = "Formal and slightly archaic; keep the period vocabulary.".to_string();
    let request = ConfirmRequest {
        project_id: project_id.clone(),
        profile: profile.clone(),
        confirmed_fields,
        style_guide: style_guide.clone(),
        proper_nouns: terms,
    };
    let confirmed = recon::confirm(&pool, &request).await?;

    assert_eq!(confirmed.style_guide, style_guide);
    assert_eq!(confirmed.synopsis, profile.synopsis.value);
    assert!(confirmed.book_meta.is_some());
    assert_eq!(confirmed.glossary.len(), 2);
    assert!(confirmed
        .glossary
        .iter()
        .all(|term| term.status == "approved"));
    let do_not_translate = confirmed
        .glossary
        .iter()
        .find(|term| term.kind == "do_not_translate")
        .expect("the do_not_translate branch");
    assert_eq!(do_not_translate.source, "Harbour Light");
    assert_eq!(do_not_translate.target, "Harbour Light");

    let project = repo::get_project(&pool, &project_id)
        .await?
        .expect("project");
    assert_eq!(project.source_lang.as_deref(), Some("English"));

    // ---- what the builder reads ---------------------------------------------
    let builder = ContextBuilder::load(&deps.prompts_dir(&project_id));
    let inputs = ContextInputs {
        source_language: project.source_lang.clone().unwrap_or_default(),
        target_language: project.target_lang.clone(),
        style_guide: confirmed.style_guide.clone(),
        synopsis: confirmed.synopsis.clone(),
        book_title: project.doc_title.clone().unwrap_or_default(),
        book_author: project.doc_author.clone().unwrap_or_default(),
        chapter_title: "Chapter One".to_string(),
        chunk_text: "The harbour was quiet.".to_string(),
        budget_tokens: 1000,
        ..ContextInputs::default()
    };
    let built = builder.build(&inputs, &HeuristicCounter)?;
    assert!(built.system.contains(&style_guide));
    assert!(built.user.contains(&confirmed.synopsis));

    // ---- audit and idempotency ----------------------------------------------
    let calls: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM llm_call WHERE role = 'orchestrator'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(calls, 1);

    let again = recon::confirm(&pool, &request).await?;
    assert_eq!(
        again.glossary.len(),
        2,
        "a second confirmation must update, not duplicate"
    );

    // The candidate key and the confirmed meta key are both present.
    assert!(repo::get_memory(&pool, &project_id, MEMORY_KEY)
        .await?
        .is_some());
    assert!(repo::get_memory(&pool, &project_id, META_KEY)
        .await?
        .is_some());
    Ok(())
}

#[tokio::test]
async fn without_orchestrator_binding_the_step_fails_cleanly() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let project_id = seed_project(&pool, "http://127.0.0.1:1", false).await?;

    let error = recon::run_recon(&deps, None, &project_id, None)
        .await
        .expect_err("no orchestrator binding must fail the run");
    assert!(matches!(error, AppError::Invalid(_)));
    assert!(error.to_string().contains("orchestrator"));

    let snapshot = recon::snapshot(&pool, &project_id).await?;
    assert!(!snapshot.orchestrator_bound);
    assert!(snapshot.profile.is_none());
    Ok(())
}

/// Dispatch `book_recon` exactly the way the production dispatcher does, so the
/// job payload (`pasted_text`) and the job lifecycle are covered too.
#[tokio::test]
async fn the_book_recon_job_reaches_the_pipeline() -> Result<()> {
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

    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    let project_id = seed_project(&pool, &server.uri(), true).await?;

    let job = NewJob::new(
        &project_id,
        recon::JOB_KIND,
        json!({ "pasted_text": "A page the user pasted." }),
    );
    let job_id = queue::enqueue(&pool, &job).await?;

    struct Dispatcher {
        deps: PipelineDeps,
    }

    #[async_trait]
    impl JobDispatcher for Dispatcher {
        async fn dispatch(&self, job: &Job) -> AppResult<()> {
            assert_eq!(job.kind, recon::JOB_KIND);
            let payload: Value = serde_json::from_str(&job.payload_json)?;
            let pasted = payload.get("pasted_text").and_then(Value::as_str);
            recon::run_recon(&self.deps, Some(&job.id), &job.project_id, pasted).await?;
            Ok(())
        }
    }

    let emitter: Arc<dyn EventEmitter> = Arc::new(NullEmitter);
    let worker = WorkerPool::new(
        pool.clone(),
        Arc::new(Dispatcher { deps: deps.clone() }),
        emitter,
        1,
        1,
    );
    worker.start();

    let deadline = Instant::now() + Duration::from_secs(10);
    let state = loop {
        let job = queue::get_job(&pool, &job_id)
            .await?
            .context("the job disappeared")?;
        if matches!(job.state.as_str(), "done" | "failed" | "cancelled") {
            break job.state;
        }
        if Instant::now() >= deadline {
            worker.cancel();
            bail!("the book_recon job did not finish in time");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    worker.cancel();
    assert_eq!(state, "done", "the book_recon job failed");

    let snapshot = recon::snapshot(&pool, &project_id).await?;
    assert!(
        snapshot.profile.is_some(),
        "the job left no candidate profile"
    );
    assert!(snapshot.running_job.is_none());
    Ok(())
}
