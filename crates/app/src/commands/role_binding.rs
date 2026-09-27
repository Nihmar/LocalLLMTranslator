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

#[tauri::command]
pub async fn role_binding_list(state: State<'_, AppState>) -> Result<Vec<RoleBinding>> {
    repo::list_role_bindings(&state.pool).await
}

#[tauri::command]
pub async fn role_binding_set(
    state: State<'_, AppState>,
    req: RoleBindingSet,
) -> Result<RoleBinding> {
    let params_json = match &req.params {
        Value::Null => "{}".to_string(),
        other => serde_json::to_string(other)?,
    };
    let binding = RoleBinding {
        id: req.id.unwrap_or_else(new_id),
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
#[tauri::command]
pub async fn role_binding_delete(state: State<'_, AppState>, id: String) -> Result<Ack> {
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
