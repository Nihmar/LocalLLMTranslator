//! `sidecar_status` command.

use tauri::State;

use crate::error::Result;
use crate::sidecar::SidecarStatus;
use crate::AppState;

#[tauri::command]
pub async fn sidecar_status(state: State<'_, AppState>) -> Result<SidecarStatus> {
    Ok(state.supervisor.status())
}
