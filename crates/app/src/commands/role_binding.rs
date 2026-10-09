//! `role_binding_*` commands.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use super::Ack;
use crate::db::models::RoleBinding;
use crate::db::{new_id, repo};
use crate::error::Result;
use crate::AppState;

#[derive(Debug, Clone, Deserialize)]
pub struct RoleBindingSet {
    #[serde(default)]
    pub id: Option<String>,
    pub role: String,
    pub endpoint_id: String,
    pub model: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub priority: Option<i64>,
}

#[tauri::command(rename = "role_binding_list", rename_all = "snake_case")]
pub async fn role_binding_list_command(state: State<'_, AppState>) -> Result<Vec<RoleBinding>> {
    role_binding_list(&state).await
}

pub async fn role_binding_list(state: &AppState) -> Result<Vec<RoleBinding>> {
    repo::list_role_bindings(&state.pool).await
}

#[tauri::command(rename = "role_binding_set", rename_all = "snake_case")]
pub async fn role_binding_set_command(
    state: State<'_, AppState>,
    req: RoleBindingSet,
) -> Result<RoleBinding> {
    role_binding_set(&state, req).await
}

pub async fn role_binding_set(state: &AppState, req: RoleBindingSet) -> Result<RoleBinding> {
    let params_json = match &req.params {
        Value::Null => "{}".to_string(),
        other => serde_json::to_string(other)?,
    };
    // Without an explicit id, re-assigning a role on the same endpoint updates that binding
    // instead of adding a second row for the same pair: the UI treats (role, endpoint) as one
    // assignment, and a twin would silently shadow it in `role_binding_for` by priority.
    let id = match req.id {
        Some(id) => Some(id),
        None => repo::find_role_binding(&state.pool, &req.role, &req.endpoint_id)
            .await?
            .map(|existing| existing.id),
    };
    let binding = RoleBinding {
        id: id.unwrap_or_else(new_id),
        endpoint_id: req.endpoint_id,
        role: req.role,
        model: req.model,
        params_json,
        priority: req.priority.unwrap_or(0),
    };
    repo::upsert_role_binding(&state.pool, &binding).await?;
    Ok(binding)
}

/// Remove a role assignment.
///
/// The counterpart of `role_binding_set`: without it an assignment made once could never be
/// undone through the UI, and the row kept deciding which endpoint a role used.
#[tauri::command(rename = "role_binding_delete", rename_all = "snake_case")]
pub async fn role_binding_delete_command(state: State<'_, AppState>, id: String) -> Result<Ack> {
    role_binding_delete(&state, id).await
}

pub async fn role_binding_delete(state: &AppState, id: String) -> Result<Ack> {
    repo::delete_role_binding(&state.pool, &id).await?;
    Ok(Ack::done())
}

/// Convenience payload for the UI: which roles are currently bound.
#[derive(Debug, Clone, Serialize)]
pub struct RoleBindingSummary {
    pub role: String,
    pub model: String,
    pub endpoint_id: String,
}
