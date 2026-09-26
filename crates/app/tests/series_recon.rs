//! Series reconnaissance integration test (PLAN.md §9.5, S5).
//!
//! `run_series_recon` runs against a real SQLite database and a wiremock `llama-server`.
//! The assertions pin the candidate principle: the model output lands under
//! `series_memory['series_profile']` and touches neither the series style guide/synopsis nor
//! any translation prompt until the user copies it.

mod common;

use anyhow::Result;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use app_lib::db::models::{Series, SeriesGlossaryTerm};
use app_lib::db::{new_id, now, repo};
use app_lib::pipeline::series_recon::{self, MEMORY_KEY};

use common::{deps_for, seed_project};

/// The deterministic candidate the mock model returns, shaped like
/// `prompts/series_recon.schema.json`.
const PROFILE_JSON: &str = r#"{"synopsis":"A saga about the harbour light.","style_notes":["Keep the period register."],"characters":[{"source":"Keeper","target":"Custode","note":"protagonist"},{"source":"Harbour Light","target":"Luce del Porto","note":"the lighthouse"}]}"#;

fn sse_body() -> String {
    let content = json!({ "choices": [{ "delta": { "content": PROFILE_JSON } }] });
    let finish = json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5 },
    });
    format!("data: {content}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

async fn seed_series(pool: &sqlx::SqlitePool, project_id: &str) -> Result<Series> {
    let timestamp = now();
    let series = Series {
        id: new_id(),
        name: "The Saga".to_string(),
        source_lang: Some("English".to_string()),
        target_lang: Some("Italian".to_string()),
        settings_json: "{}".to_string(),
        created_at: timestamp.clone(),
        updated_at: timestamp,
    };
    repo::upsert_series(pool, &series).await?;
    repo::set_project_series(pool, project_id, Some(&series.id), Some(1)).await?;
    repo::upsert_series_term(
        pool,
        &SeriesGlossaryTerm {
            id: new_id(),
            series_id: series.id.clone(),
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
    Ok(series)
}

#[tokio::test]
async fn series_recon_produces_a_candidate_without_touching_the_canon() -> Result<()> {
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
    let project_id = seed_project(&pool, "series", &server.uri(), true)
        .await?
        .project_id;
    let series = seed_series(&pool, &project_id).await?;

    // The confirmed book profile is the evidence.
    repo::set_memory(&pool, &project_id, "synopsis", "A keeper guards the light.").await?;
    repo::set_memory(&pool, &project_id, "style_guide", "Formal register.").await?;

    let outcome = series_recon::run_series_recon(&deps, None, &series.id).await?;
    assert_eq!(outcome.model, "fake-model");
    assert_eq!(outcome.books, 1);
    assert_eq!(outcome.characters, 2);

    let stored = repo::get_series_memory(&pool, &series.id, MEMORY_KEY)
        .await?
        .expect("candidate profile");
    let profile: serde_json::Value = serde_json::from_str(&stored)?;
    assert_eq!(profile["synopsis"], "A saga about the harbour light.");
    assert_eq!(profile["characters"][0]["source"], "Keeper");
    assert_eq!(profile["provenance"]["books"][0], "project-series");

    // The candidate is not the canon: the injected memory is still empty.
    assert!(repo::get_series_memory(&pool, &series.id, "synopsis")
        .await?
        .is_none());
    assert!(repo::get_series_memory(&pool, &series.id, "style_guide")
        .await?
        .is_none());

    // A second run replaces the candidate, it does not pile up.
    series_recon::run_series_recon(&deps, None, &series.id).await?;
    assert_eq!(
        repo::list_series_memory(&pool, &series.id)
            .await?
            .into_iter()
            .filter(|row| row.key == MEMORY_KEY)
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn series_recon_needs_confirmed_book_evidence() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (pool, deps) = deps_for(dir.path()).await?;
    // No endpoint is needed: the run fails before the model call.
    let project_id = seed_project(&pool, "series", "http://127.0.0.1:1", true)
        .await?
        .project_id;
    let series = seed_series(&pool, &project_id).await?;

    let error = series_recon::run_series_recon(&deps, None, &series.id)
        .await
        .expect_err("an empty evidence set must be rejected");
    assert!(
        error.to_string().contains("confirmed book profile"),
        "{error}"
    );
    Ok(())
}
