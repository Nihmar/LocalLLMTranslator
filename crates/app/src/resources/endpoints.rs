//! Per-endpoint LLM concurrency (PLAN.md §10).
//!
//! The worker pool claims a job only when the role's endpoint has a free slot.
//! The limit is `min(llm_endpoint.max_concurrency, /props.total_slots)`; when the
//! endpoint reports neither it runs serially, without error. Jobs that never
//! call an LLM (ingest, export, QA scan) belong to [`LOCAL_ROLE`] and use the
//! global cap.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::repo;
use crate::llm::{LlamaClient, Props};

/// The pseudo-role of jobs that never call an LLM endpoint (sidecar, pandoc).
pub const LOCAL_ROLE: &str = "local";

/// Roles the pool may schedule, in a stable order for the UI and the tests.
pub const ROLES: [&str; 5] = [
    LOCAL_ROLE,
    "editor",
    "orchestrator",
    "proofreader",
    "translator",
];

/// The roles that need an endpoint.
pub const LLM_ROLES: [&str; 4] = ["editor", "orchestrator", "proofreader", "translator"];

/// Role a job kind needs. Unknown kinds are local work.
pub fn role_for_kind(kind: &str) -> &'static str {
    match kind {
        "translate_chunk" => "translator",
        "edit_chunk" => "editor",
        "proofread_chunk" => "proofreader",
        "book_recon" | "summarize" => "orchestrator",
        _ => LOCAL_ROLE,
    }
}

/// Job kinds a role runs, in claim order.
pub fn kinds_for_role(role: &str) -> &'static [&'static str] {
    match role {
        "translator" => &["translate_chunk"],
        "editor" => &["edit_chunk"],
        "proofreader" => &["proofread_chunk"],
        "orchestrator" => &["book_recon", "summarize"],
        _ => &["ingest", "export_unit", "qa_scan"],
    }
}

/// Limit and reason for one endpoint, as resolved at boot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EndpointPlanEntry {
    pub role: String,
    pub endpoint_id: Option<String>,
    pub limit: usize,
    pub reason: String,
}

/// `min(max_concurrency, total_slots)`; serial (1) when neither is reported.
pub fn plan_limit(max_concurrency: Option<i64>, total_slots: Option<u32>) -> (usize, String) {
    let max_concurrency = max_concurrency
        .filter(|value| *value > 0)
        .map(|value| value as usize);
    let total_slots = total_slots
        .filter(|value| *value > 0)
        .map(|value| value as usize);
    match (max_concurrency, total_slots) {
        (None, None) => (1, "serial: the endpoint reports no slots".to_string()),
        (Some(limit), None) => (limit, format!("capped by max_concurrency = {limit}")),
        (None, Some(limit)) => (limit, format!("capped by /props total_slots = {limit}")),
        (Some(max_concurrency), Some(total_slots)) => {
            let limit = max_concurrency.min(total_slots);
            (
                limit,
                format!("min(max_concurrency = {max_concurrency}, total_slots = {total_slots})"),
            )
        }
    }
}

/// Live usage of one role, reported on `metrics_get` / `metrics://tick`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EndpointUsage {
    pub role: String,
    pub endpoint_id: Option<String>,
    pub limit: usize,
    pub in_flight: usize,
    pub reason: String,
}

struct LimitState {
    endpoint_id: Option<String>,
    limit: usize,
    in_flight: usize,
    reason: String,
}

/// Live per-role capacity. Unbound LLM roles are absent (their jobs would fail
/// anyway); [`LOCAL_ROLE`] is always present.
pub struct EndpointLimits {
    state: Mutex<HashMap<String, LimitState>>,
}

impl EndpointLimits {
    /// Build from the boot plan plus the global cap that bounds local work. A
    /// role without a binding still gets capacity (bounded by the global cap),
    /// so its jobs are claimed and fail with the dispatcher's clear "no binding"
    /// error instead of sitting in the queue forever.
    pub fn from_plan(entries: &[EndpointPlanEntry], local_limit: usize) -> Arc<Self> {
        let local_limit = local_limit.max(1);
        let mut state = HashMap::new();
        state.insert(
            LOCAL_ROLE.to_string(),
            LimitState {
                endpoint_id: None,
                limit: local_limit,
                in_flight: 0,
                reason: format!("local work, global cap {local_limit}"),
            },
        );
        for role in LLM_ROLES {
            let limit_state = match entries.iter().find(|entry| entry.role == role) {
                Some(entry) => LimitState {
                    endpoint_id: entry.endpoint_id.clone(),
                    limit: entry.limit.max(1),
                    in_flight: 0,
                    reason: entry.reason.clone(),
                },
                None => LimitState {
                    endpoint_id: None,
                    limit: local_limit,
                    in_flight: 0,
                    reason: format!("no endpoint bound; bounded by the global cap {local_limit}"),
                },
            };
            state.insert(role.to_string(), limit_state);
        }
        Arc::new(Self {
            state: Mutex::new(state),
        })
    }

    /// A limiter with only local capacity (pools without endpoints, tests).
    pub fn local_only(local_limit: usize) -> Arc<Self> {
        Self::from_plan(&[], local_limit)
    }

    /// Roles known to the limiter, in the stable [`ROLES`] order.
    pub fn roles(&self) -> Vec<String> {
        let state = self.state.lock();
        ROLES
            .iter()
            .filter(|role| state.contains_key(**role))
            .map(|role| (*role).to_string())
            .collect()
    }

    /// Point-in-time usage, for the metrics payload.
    pub fn usage(&self) -> Vec<EndpointUsage> {
        let state = self.state.lock();
        ROLES
            .iter()
            .filter_map(|role| {
                let limit = state.get(*role)?;
                Some(EndpointUsage {
                    role: (*role).to_string(),
                    endpoint_id: limit.endpoint_id.clone(),
                    limit: limit.limit,
                    in_flight: limit.in_flight,
                    reason: limit.reason.clone(),
                })
            })
            .collect()
    }

    /// Total slots across the bound LLM endpoints (local and unbound roles are
    /// not endpoint capacity).
    pub fn total_slots(&self) -> usize {
        let state = self.state.lock();
        state
            .iter()
            .filter(|(role, limit)| role.as_str() != LOCAL_ROLE && limit.endpoint_id.is_some())
            .map(|(_, limit)| limit.limit)
            .sum()
    }

    pub fn has_capacity(&self, role: &str) -> bool {
        let state = self.state.lock();
        state
            .get(role)
            .is_some_and(|limit| limit.in_flight < limit.limit)
    }

    fn release(&self, role: &str) {
        let mut state = self.state.lock();
        if let Some(limit) = state.get_mut(role) {
            limit.in_flight = limit.in_flight.saturating_sub(1);
        }
    }

    /// Reserve one slot for `role`, or `None` when it is at capacity.
    pub fn try_acquire(self: &Arc<Self>, role: &str) -> Option<EndpointPermit> {
        {
            let mut state = self.state.lock();
            let limit = state.get_mut(role)?;
            if limit.in_flight >= limit.limit {
                return None;
            }
            limit.in_flight += 1;
        }
        Some(EndpointPermit {
            limits: Arc::clone(self),
            role: role.to_string(),
        })
    }
}

/// Releases its slot on drop, so a panicking or cancelled job cannot leak one.
pub struct EndpointPermit {
    limits: Arc<EndpointLimits>,
    role: String,
}

impl Drop for EndpointPermit {
    fn drop(&mut self) {
        self.limits.release(&self.role);
    }
}

/// Resolve the per-role plan for the current bindings: a live `/props` probe
/// first (short timeout through the shared client), the persisted props as
/// fallback. Endpoints that cannot be reached still produce an entry, because
/// `max_concurrency` is db-only information and the limit must not silently
/// become the global cap.
pub async fn probe_endpoint_limits(pool: &SqlitePool) -> Vec<EndpointPlanEntry> {
    let mut entries = Vec::new();
    for role in LLM_ROLES {
        let Ok(Some(binding)) = repo::role_binding_for(pool, role).await else {
            continue;
        };
        let Ok(Some(endpoint)) = repo::get_endpoint(pool, &binding.endpoint_id).await else {
            continue;
        };
        let persisted = endpoint
            .props_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Props>(json).ok())
            .and_then(|props| props.total_slots);
        let live = match LlamaClient::new(&endpoint.base_url) {
            Ok(client) => client
                .props()
                .await
                .ok()
                .and_then(|props| props.total_slots),
            Err(_) => None,
        };
        let (limit, reason) = plan_limit(endpoint.max_concurrency, live.or(persisted));
        entries.push(EndpointPlanEntry {
            role: role.to_string(),
            endpoint_id: Some(endpoint.id),
            limit,
            reason,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_kinds_map_to_their_role() {
        assert_eq!(role_for_kind("translate_chunk"), "translator");
        assert_eq!(role_for_kind("edit_chunk"), "editor");
        assert_eq!(role_for_kind("proofread_chunk"), "proofreader");
        assert_eq!(role_for_kind("book_recon"), "orchestrator");
        assert_eq!(role_for_kind("summarize"), "orchestrator");
        // Sidecar/pandoc work needs no endpoint.
        assert_eq!(role_for_kind("ingest"), LOCAL_ROLE);
        assert_eq!(role_for_kind("export_unit"), LOCAL_ROLE);
        assert_eq!(role_for_kind("qa_scan"), LOCAL_ROLE);
        assert_eq!(role_for_kind("unknown"), LOCAL_ROLE);

        // Every kind a role claims maps back to that role.
        for role in ROLES {
            for kind in kinds_for_role(role) {
                assert_eq!(role_for_kind(kind), role, "{kind} does not map to {role}");
            }
        }
    }

    #[test]
    fn plan_limit_takes_the_minimum_and_degrades_serially() {
        assert_eq!(plan_limit(None, None).0, 1);
        assert!(plan_limit(None, None).1.contains("serial"));
        assert_eq!(plan_limit(Some(4), None).0, 4);
        assert_eq!(plan_limit(None, Some(2)).0, 2);
        assert_eq!(plan_limit(Some(4), Some(2)).0, 2);
        // Non-positive values are treated as missing.
        assert_eq!(plan_limit(Some(0), Some(0)).0, 1);
        assert_eq!(plan_limit(Some(-1), None).0, 1);
    }

    #[test]
    fn limiter_hands_out_at_most_the_limit_and_releases_on_drop() {
        let plan = vec![EndpointPlanEntry {
            role: "translator".to_string(),
            endpoint_id: Some("e1".to_string()),
            limit: 2,
            reason: "test".to_string(),
        }];
        let limits = EndpointLimits::from_plan(&plan, 4);
        assert!(limits.roles().contains(&"translator".to_string()));
        assert_eq!(limits.total_slots(), 2);

        let first = limits.try_acquire("translator").expect("first");
        let second = limits.try_acquire("translator").expect("second");
        assert!(limits.try_acquire("translator").is_none(), "limit reached");
        assert!(!limits.has_capacity("translator"));

        let usage = limits.usage();
        let translator = usage
            .iter()
            .find(|entry| entry.role == "translator")
            .expect("entry");
        assert_eq!(translator.in_flight, 2);
        assert_eq!(translator.limit, 2);
        assert_eq!(translator.endpoint_id.as_deref(), Some("e1"));

        drop(second);
        assert!(limits.has_capacity("translator"));
        drop(first);
        assert_eq!(
            limits
                .usage()
                .iter()
                .find(|entry| entry.role == "translator")
                .map(|entry| entry.in_flight),
            Some(0)
        );
    }

    #[test]
    fn an_unbound_role_runs_under_the_global_cap_and_never_shows_as_slots() {
        let limits = EndpointLimits::local_only(2);
        assert!(limits.roles().contains(&"translator".to_string()));
        assert_eq!(limits.total_slots(), 0, "no endpoint is bound");
        let permit = limits.try_acquire("translator").expect("global cap");
        let translator = limits
            .usage()
            .into_iter()
            .find(|entry| entry.role == "translator")
            .expect("usage");
        assert_eq!(translator.limit, 2);
        assert!(translator.endpoint_id.is_none());
        drop(permit);
    }
}
