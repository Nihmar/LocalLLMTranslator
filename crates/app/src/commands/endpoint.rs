//! `endpoint_*` commands.

use serde::{Deserialize, Serialize};
use tauri::State;

use super::Ack;
use crate::db::models::LlmEndpoint;
use crate::db::{new_id, repo};
use crate::error::{AppError, Result};
use crate::llm::{probe, EndpointHealth, LlamaClient, ModelInfo, Props};
use crate::AppState;

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
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

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct EndpointTestResult {
    pub health: EndpointHealth,
    pub props: Option<Props>,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct EndpointModelsRequest {
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[tauri::command(rename = "endpoint_list", rename_all = "snake_case")]
pub async fn endpoint_list_command(state: State<'_, AppState>) -> Result<Vec<LlmEndpoint>> {
    endpoint_list(&state).await
}

pub async fn endpoint_list(state: &AppState) -> Result<Vec<LlmEndpoint>> {
    repo::list_endpoints(&state.pool).await
}

#[tauri::command(rename = "endpoint_upsert", rename_all = "snake_case")]
pub async fn endpoint_upsert_command(
    state: State<'_, AppState>,
    req: EndpointUpsert,
) -> Result<LlmEndpoint> {
    endpoint_upsert(&state, req).await
}

pub async fn endpoint_upsert(state: &AppState, req: EndpointUpsert) -> Result<LlmEndpoint> {
    // The edit form only sends the editable fields, so the health snapshot persisted by
    // `endpoint_test` must be carried over explicitly: the upsert would otherwise reset
    // `props_json` (the endpoint's `n_ctx`/`total_slots`) and `last_health_*` to NULL.
    let existing = match &req.id {
        Some(id) => repo::get_endpoint(&state.pool, id).await?,
        None => None,
    };
    let endpoint = merge_endpoint(existing.as_ref(), req);
    repo::upsert_endpoint(&state.pool, &endpoint).await?;
    Ok(endpoint)
}

/// Build the row an endpoint upsert writes, carrying the health state the request does
/// not carry over from the stored row.
fn merge_endpoint(existing: Option<&LlmEndpoint>, req: EndpointUpsert) -> LlmEndpoint {
    LlmEndpoint {
        id: req.id.unwrap_or_else(new_id),
        name: req.name,
        base_url: req.base_url.trim_end_matches('/').to_string(),
        api_key_ref: req.api_key_ref,
        max_concurrency: req.max_concurrency,
        notes: req.notes,
        last_health_at: existing.and_then(|row| row.last_health_at.clone()),
        last_health_ok: existing.and_then(|row| row.last_health_ok),
        props_json: existing.and_then(|row| row.props_json.clone()),
    }
}

#[tauri::command(rename = "endpoint_delete", rename_all = "snake_case")]
pub async fn endpoint_delete_command(state: State<'_, AppState>, id: String) -> Result<Ack> {
    endpoint_delete(&state, id).await
}

pub async fn endpoint_delete(state: &AppState, id: String) -> Result<Ack> {
    repo::delete_endpoint(&state.pool, &id).await?;
    Ok(Ack::done())
}

#[tauri::command(rename = "endpoint_test", rename_all = "snake_case")]
pub async fn endpoint_test_command(
    state: State<'_, AppState>,
    id: String,
) -> Result<EndpointTestResult> {
    endpoint_test(&state, id).await
}

pub async fn endpoint_test(state: &AppState, id: String) -> Result<EndpointTestResult> {
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

#[tauri::command(rename = "endpoint_models", rename_all = "snake_case")]
pub async fn endpoint_models_command(
    state: State<'_, AppState>,
    req: EndpointModelsRequest,
) -> Result<Vec<ModelInfo>> {
    endpoint_models(&state, req).await
}

pub async fn endpoint_models(
    state: &AppState,
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

#[cfg(test)]
mod tests {
    use super::{merge_endpoint, EndpointUpsert};
    use crate::db::models::LlmEndpoint;

    fn stored() -> LlmEndpoint {
        LlmEndpoint {
            id: "e1".to_string(),
            name: "old".to_string(),
            base_url: "http://127.0.0.1:8080".to_string(),
            api_key_ref: None,
            max_concurrency: None,
            notes: None,
            last_health_at: Some("2026-01-01T00:00:00.000Z".to_string()),
            last_health_ok: Some(true),
            props_json: Some(r#"{"n_ctx":32768,"total_slots":4}"#.to_string()),
        }
    }

    fn edit() -> EndpointUpsert {
        EndpointUpsert {
            id: Some("e1".to_string()),
            name: "renamed".to_string(),
            base_url: "http://127.0.0.1:8080/".to_string(),
            api_key_ref: None,
            max_concurrency: Some(2),
            notes: Some("note".to_string()),
        }
    }

    #[test]
    fn editing_an_endpoint_keeps_its_health_snapshot() {
        let merged = merge_endpoint(Some(&stored()), edit());
        assert_eq!(merged.name, "renamed");
        assert_eq!(merged.base_url, "http://127.0.0.1:8080", "trailing slash");
        assert_eq!(merged.max_concurrency, Some(2));
        // The probe result survives the edit.
        assert_eq!(
            merged.last_health_at.as_deref(),
            Some("2026-01-01T00:00:00.000Z")
        );
        assert_eq!(merged.last_health_ok, Some(true));
        assert_eq!(
            merged.props_json.as_deref(),
            Some(r#"{"n_ctx":32768,"total_slots":4}"#)
        );
    }

    #[test]
    fn a_new_endpoint_has_no_health_snapshot() {
        let mut fresh = edit();
        fresh.id = None;
        let merged = merge_endpoint(None, fresh);
        assert!(merged.last_health_at.is_none());
        assert!(merged.last_health_ok.is_none());
        assert!(merged.props_json.is_none());
        assert!(!merged.id.is_empty());
    }
}
