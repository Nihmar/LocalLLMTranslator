//! Pipeline: ingestion, translation and export orchestration.

pub mod chat_call;
pub mod export;
pub mod ingest;
pub mod recon;
pub mod summarize;
pub mod translate;

use std::path::PathBuf;

use sqlx::SqlitePool;

use crate::context::budget::{budget_from_n_ctx, DEFAULT_CHUNK_BUDGET};
use crate::db::models::LlmEndpoint;
use crate::llm::{LlamaClient, Props};
use crate::resources::ResourceGovernor;
use crate::sidecar::SidecarClient;

/// Shared dependencies handed to every pipeline stage.
#[derive(Clone)]
pub struct PipelineDeps {
    pub pool: SqlitePool,
    pub sidecar: SidecarClient,
    pub resources: ResourceGovernor,
    pub data_dir: PathBuf,
}

impl PipelineDeps {
    pub fn new(
        pool: SqlitePool,
        sidecar: SidecarClient,
        resources: ResourceGovernor,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            pool,
            sidecar,
            resources,
            data_dir,
        }
    }

    fn project_dir(&self, project_id: &str) -> PathBuf {
        self.data_dir.join("projects").join(project_id)
    }

    /// Working directory handed to the sidecar for one project.
    pub fn work_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("work")
    }

    /// Output directory for Pandoc builds.
    pub fn output_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("output")
    }

    /// Immutable copy of the prompts used for a project.
    pub fn prompts_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("prompts")
    }
}

/// Context budget for a role endpoint, read from `/props` (PLAN.md section 9.1).
///
/// Prefers the props persisted by `endpoint_test`, probes the endpoint once when
/// they are missing, and falls back to [`DEFAULT_CHUNK_BUDGET`] when neither
/// works — an unreachable server must never block the pipeline.
pub async fn resolve_endpoint_budget(endpoint: &LlmEndpoint) -> usize {
    if let Some(n_ctx) = persisted_n_ctx(endpoint) {
        return budget_from_n_ctx(n_ctx);
    }
    if let Ok(client) = LlamaClient::new(&endpoint.base_url) {
        if let Ok(props) = client.props().await {
            if let Some(n_ctx) = props.n_ctx {
                return budget_from_n_ctx(n_ctx);
            }
        }
    }
    DEFAULT_CHUNK_BUDGET
}

fn persisted_n_ctx(endpoint: &LlmEndpoint) -> Option<u32> {
    endpoint
        .props_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Props>(json).ok())
        .and_then(|props| props.n_ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn endpoint(base_url: &str, props_json: Option<&str>) -> LlmEndpoint {
        LlmEndpoint {
            id: "e1".to_string(),
            name: "e".to_string(),
            base_url: base_url.to_string(),
            api_key_ref: None,
            max_concurrency: None,
            notes: None,
            last_health_at: None,
            last_health_ok: None,
            props_json: props_json.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn budget_prefers_the_persisted_props() {
        let endpoint = endpoint("http://127.0.0.1:1", Some(r#"{"n_ctx":8192}"#));
        assert_eq!(
            resolve_endpoint_budget(&endpoint).await,
            budget_from_n_ctx(8192)
        );
    }

    #[tokio::test]
    async fn budget_probes_the_endpoint_when_props_are_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"n_ctx":4096}"#))
            .mount(&server)
            .await;
        let endpoint = endpoint(&server.uri(), None);
        assert_eq!(
            resolve_endpoint_budget(&endpoint).await,
            budget_from_n_ctx(4096)
        );
    }

    #[tokio::test]
    async fn budget_falls_back_to_the_default_when_the_server_is_unreachable() {
        let endpoint = endpoint("http://127.0.0.1:1", None);
        assert_eq!(
            resolve_endpoint_budget(&endpoint).await,
            DEFAULT_CHUNK_BUDGET
        );
    }
}
