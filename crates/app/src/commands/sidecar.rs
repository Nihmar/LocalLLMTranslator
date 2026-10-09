//! `sidecar_status` command.

use tauri::State;

use crate::error::Result;
use crate::sidecar::SidecarStatus;
use crate::AppState;

/// Report the sidecar state, starting the process first when it is not running.
///
/// The UI calls this on mount and from the "Verifica sidecar" control, so it must
/// actively ensure the process exists: a passive read would leave the banner stuck
/// on "Sidecar non pronto" until an unrelated job happened to use the sidecar. A
/// failed spawn is not returned as an error: the status carries the reason and the
/// banner renders it, keeping the command contract (a `SidecarStatus` result).
#[tauri::command(rename_all = "snake_case")]
pub async fn sidecar_status(state: State<'_, AppState>) -> Result<SidecarStatus> {
    if let Err(error) = state.supervisor.ensure_running().await {
        tracing::warn!(%error, "sidecar status check could not start the sidecar");
    }
    Ok(state.supervisor.status())
}
