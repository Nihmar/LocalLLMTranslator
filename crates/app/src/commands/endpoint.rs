//! `endpoint_*` commands.

use serde::{Deserialize, Serialize};
use tauri::State;

use super::Ack;
use crate::db::models::LlmEndpoint;
use crate::db::{new_id, repo};
use crate::error::{AppError, Result};
use crate::llm::{probe, EndpointHealth, LlamaClient, ModelInfo, Props};
use crate::AppState;

#[derive(Debug, Clone, Deserialize)]
pub struct EndpointUpsert {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key_ref: Option<String>,
    #[serde(default)]
    pub max_concurrency: Option<i64>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EndpointTestResult {
    pub health: EndpointHealth,
    pub props: Option<Props>,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EndpointModelsRequest {
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn endpoint_list(state: State<'_, AppState>) -> Result<Vec<LlmEndpoint>> {
    repo::list_endpoints(&state.pool).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn endpoint_upsert(
    state: State<'_, AppState>,
    req: EndpointUpsert,
) -> Result<LlmEndpoint> {
    let endpoint = LlmEndpoint {
        id: req.id.unwrap_or_else(new_id),
        name: req.name,
        base_url: req.base_url.trim_end_matches('/').to_string(),
        api_key_ref: req.api_key_ref,
        max_concurrency: req.max_concurrency,
        notes: req.notes,
        last_health_at: None,
        last_health_ok: None,
        props_json: None,
    };
    repo::upsert_endpoint(&state.pool, &endpoint).await?;
    Ok(endpoint)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn endpoint_delete(state: State<'_, AppState>, id: String) -> Result<Ack> {
    repo::delete_endpoint(&state.pool, &id).await?;
    Ok(Ack::done())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn endpoint_test(state: State<'_, AppState>, id: String) -> Result<EndpointTestResult> {
    let endpoint = repo::get_endpoint(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("endpoint {id}")))?;
    let client = LlamaClient::new(&endpoint.base_url)?;

    let health = probe(&client).await;
    let (props, models) = if health.ok {
        let props = client.props().await.ok();
        let models = client.models().await.unwrap_or_default();
        (props, models)
    } else {
        (None, Vec::new())
    };

    // Persist the probe result so the UI can show freshness.
    let mut updated = endpoint;
    updated.last_health_at = Some(health.checked_at.clone());
    updated.last_health_ok = Some(health.ok);
    updated.props_json = props.as_ref().and_then(|p| serde_json::to_string(p).ok());
    let _ = repo::upsert_endpoint(&state.pool, &updated).await;

    Ok(EndpointTestResult {
        health,
        props,
        models,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn endpoint_models(
    state: State<'_, AppState>,
    req: EndpointModelsRequest,
) -> Result<Vec<ModelInfo>> {
    let base_url = match (req.base_url, req.endpoint_id) {
        (Some(url), _) => url,
        (None, Some(id)) => {
            repo::get_endpoint(&state.pool, &id)
                .await?
                .ok_or_else(|| AppError::NotFound(format!("endpoint {id}")))?
                .base_url
        }
        (None, None) => return Err(AppError::Invalid("base_url or endpoint_id required".into())),
    };
    let client = LlamaClient::new(base_url)?;
    client.models().await
}
