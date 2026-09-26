//! Endpoint health probing.

use serde::{Deserialize, Serialize};

use super::client::LlamaClient;
use crate::db::now;

/// Result of a single `GET /health` probe, timestamped for persistence in
/// `llm_endpoint.last_health_at` / `last_health_ok`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointHealth {
    pub ok: bool,
    pub status: Option<String>,
    pub code: u16,
    pub checked_at: String,
}

/// Probe an endpoint. Network failures are reported as `ok = false` rather than
/// propagated, so a dead endpoint never aborts a command.
pub async fn probe(client: &LlamaClient) -> EndpointHealth {
    match client.health().await {
        Ok(h) => EndpointHealth {
            ok: h.ok,
            status: h.status,
            code: h.code,
            checked_at: now(),
        },
        Err(e) => EndpointHealth {
            ok: false,
            status: Some(e.to_string()),
            code: 0,
            checked_at: now(),
        },
    }
}
