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
        "book_recon" | "summarize" | "series_recon" => "orchestrator",
        _ => LOCAL_ROLE,
    }
}

/// Job kinds a role runs, in claim order.
pub fn kinds_for_role(role: &str) -> &'static [&'static str] {
    match role {
        "translator" => &["translate_chunk"],
        "editor" => &["edit_chunk"],
        "proofreader" => &["proofread_chunk"],
        "orchestrator" => &["book_recon", "series_recon", "summarize"],
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
    reason: String,
    /// Key of the shared capacity this role draws from (`endpoint:<id>` for a bound
    /// role, `role:<role>` for local and unbound work).
    capacity_key: String,
}

/// The shared, live counter of one endpoint (or of one unbound role). Two roles bound
/// to the same endpoint draw from the same counter: `llama-server` enforces a slot limit
/// per process, so per-role counters would let the pool exceed it.
#[derive(Debug, Clone, Copy)]
struct Capacity {
    limit: usize,
    in_flight: usize,
}

#[derive(Default)]
struct LimitStateMap {
    roles: HashMap<String, LimitState>,
    capacity: HashMap<String, Capacity>,
}

/// Live per-role capacity. Unbound LLM roles are absent (their jobs would fail
/// anyway); [`LOCAL_ROLE`] is always present.
pub struct EndpointLimits {
    inner: Mutex<LimitStateMap>,
}

impl EndpointLimits {
    /// Build from the boot plan plus the global cap that bounds local work. A
    /// role without a binding still gets capacity (bounded by the global cap),
    /// so its jobs are claimed and fail with the dispatcher's clear "no binding"
    /// error instead of sitting in the queue forever.
    ///
    /// Roles bound to the same endpoint share one capacity, so the endpoint's real
    /// slot limit is never exceeded by scheduling two roles side by side.
    pub fn from_plan(entries: &[EndpointPlanEntry], local_limit: usize) -> Arc<Self> {
        let local_limit = local_limit.max(1);
        let mut map = LimitStateMap::default();

        insert_capacity(&mut map, "role:local", local_limit);
        map.roles.insert(
            LOCAL_ROLE.to_string(),
            LimitState {
                endpoint_id: None,
                reason: format!("local work, global cap {local_limit}"),
                capacity_key: "role:local".to_string(),
            },
        );

        for role in LLM_ROLES {
            let (key, endpoint_id, limit, reason) =
                match entries.iter().find(|entry| entry.role == role) {
                    Some(entry) => {
                        let endpoint_id = entry.endpoint_id.clone();
                        let key = match &endpoint_id {
                            Some(id) => format!("endpoint:{id}"),
                            // A plan entry without an endpoint id is local-like capacity.
                            None => format!("role:{role}"),
                        };
                        (key, endpoint_id, entry.limit.max(1), entry.reason.clone())
                    }
                    None => (
                        format!("role:{role}"),
                        None,
                        local_limit,
                        format!("no endpoint bound; bounded by the global cap {local_limit}"),
                    ),
                };
            insert_capacity(&mut map, &key, limit);
            map.roles.insert(
                role.to_string(),
                LimitState {
                    endpoint_id,
                    reason,
                    capacity_key: key,
                },
            );
        }

        Arc::new(Self {
            inner: Mutex::new(map),
        })
    }

    /// A limiter with only local capacity (pools without endpoints, tests).
    pub fn local_only(local_limit: usize) -> Arc<Self> {
        Self::from_plan(&[], local_limit)
    }

    /// Roles known to the limiter, in the stable [`ROLES`] order.
    pub fn roles(&self) -> Vec<String> {
        let map = self.inner.lock();
        ROLES
            .iter()
            .filter(|role| map.roles.contains_key(**role))
            .map(|role| (*role).to_string())
            .collect()
    }

    /// Point-in-time usage, for the metrics payload. Roles sharing an endpoint report
    /// the endpoint's in-flight count, which is the number that actually caps them.
    pub fn usage(&self) -> Vec<EndpointUsage> {
        let map = self.inner.lock();
        ROLES
            .iter()
            .filter_map(|role| {
                let state = map.roles.get(*role)?;
                let capacity = map.capacity.get(&state.capacity_key)?;
                Some(EndpointUsage {
                    role: (*role).to_string(),
                    endpoint_id: state.endpoint_id.clone(),
                    limit: capacity.limit,
                    in_flight: capacity.in_flight,
                    reason: state.reason.clone(),
                })
            })
            .collect()
    }

    /// Total slots the plan provides, counted once per endpoint: two roles bound to the
    /// same server must not make it look twice as large.
    pub fn total_slots(&self) -> usize {
        let map = self.inner.lock();
        map.capacity
            .iter()
            .filter(|(key, _)| key.starts_with("endpoint:"))
            .map(|(_, capacity)| capacity.limit)
            .sum()
    }

    pub fn has_capacity(&self, role: &str) -> bool {
        let map = self.inner.lock();
        let Some(state) = map.roles.get(role) else {
            return false;
        };
        map.capacity
            .get(&state.capacity_key)
            .is_some_and(|capacity| capacity.in_flight < capacity.limit)
    }

    fn release(&self, capacity_key: &str) {
        let mut map = self.inner.lock();
        if let Some(capacity) = map.capacity.get_mut(capacity_key) {
            capacity.in_flight = capacity.in_flight.saturating_sub(1);
        }
    }

    /// Reserve one slot for `role`, or `None` when the capacity it draws from is at
    /// its limit.
    pub fn try_acquire(self: &Arc<Self>, role: &str) -> Option<EndpointPermit> {
        let capacity_key = {
            let mut map = self.inner.lock();
            let state = map.roles.get(role)?;
            let capacity_key = state.capacity_key.clone();
            let capacity = map.capacity.get_mut(&capacity_key)?;
            if capacity.in_flight >= capacity.limit {
                return None;
            }
            capacity.in_flight += 1;
            capacity_key
        };
        Some(EndpointPermit {
            limits: Arc::clone(self),
            capacity_key,
        })
    }
}

/// Insert the shared capacity for `key`, or tighten it to the smallest limit a role
/// asked for when the same endpoint is bound twice.
fn insert_capacity(map: &mut LimitStateMap, key: &str, limit: usize) {
    let limit = limit.max(1);
    map.capacity
        .entry(key.to_string())
        .and_modify(|capacity| capacity.limit = capacity.limit.min(limit))
        .or_insert(Capacity {
            limit,
            in_flight: 0,
        });
}

/// Distinct endpoint slots the plan provides, counted once per endpoint. Used for the
/// global cap, so a shared endpoint is not counted per role.
pub fn plan_slots(entries: &[EndpointPlanEntry]) -> usize {
    let mut limits: HashMap<&str, usize> = HashMap::new();
    for entry in entries {
        let Some(endpoint_id) = entry.endpoint_id.as_deref() else {
            continue;
        };
        let limit = entry.limit.max(1);
        limits
            .entry(endpoint_id)
            .and_modify(|current| *current = (*current).min(limit))
            .or_insert(limit);
    }
    limits.values().sum()
}

/// Releases its slot on drop, so a panicking or cancelled job cannot leak one.
pub struct EndpointPermit {
    limits: Arc<EndpointLimits>,
    capacity_key: String,
}

impl Drop for EndpointPermit {
    fn drop(&mut self) {
        self.limits.release(&self.capacity_key);
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
        assert_eq!(role_for_kind("series_recon"), "orchestrator");
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
    fn two_roles_on_one_endpoint_share_a_single_budget() {
        let plan = vec![
            EndpointPlanEntry {
                role: "translator".to_string(),
                endpoint_id: Some("e1".to_string()),
                limit: 2,
                reason: "min(...)".to_string(),
            },
            EndpointPlanEntry {
                role: "editor".to_string(),
                endpoint_id: Some("e1".to_string()),
                limit: 2,
                reason: "min(...)".to_string(),
            },
        ];
        let limits = EndpointLimits::from_plan(&plan, 8);

        // The endpoint has two slots, not four: the roles draw from one counter.
        assert_eq!(limits.total_slots(), 2, "a shared endpoint counts once");
        let first = limits.try_acquire("translator").expect("first slot");
        let second = limits.try_acquire("translator").expect("second slot");
        assert!(
            limits.try_acquire("editor").is_none(),
            "the editor must wait while the translator holds both slots"
        );

        drop(first);
        let editor = limits.try_acquire("editor").expect("released slot");
        drop(editor);
        drop(second);
        assert!(limits.has_capacity("translator"));
        assert!(limits.has_capacity("editor"));

        // Both roles report the endpoint's live usage.
        assert!(limits
            .usage()
            .iter()
            .filter(|entry| entry.endpoint_id.as_deref() == Some("e1"))
            .all(|entry| entry.in_flight == 0 && entry.limit == 2));
    }

    #[test]
    fn plan_slots_counts_a_shared_endpoint_once_and_skips_unbound_entries() {
        let plan = vec![
            EndpointPlanEntry {
                role: "translator".to_string(),
                endpoint_id: Some("e1".to_string()),
                limit: 4,
                reason: String::new(),
            },
            EndpointPlanEntry {
                role: "editor".to_string(),
                endpoint_id: Some("e1".to_string()),
                limit: 4,
                reason: String::new(),
            },
            EndpointPlanEntry {
                role: "proofreader".to_string(),
                endpoint_id: Some("e2".to_string()),
                limit: 1,
                reason: String::new(),
            },
            EndpointPlanEntry {
                role: "orchestrator".to_string(),
                endpoint_id: None,
                limit: 3,
                reason: String::new(),
            },
        ];
        assert_eq!(plan_slots(&plan), 5);
        assert_eq!(plan_slots(&[]), 0);
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

    #[tokio::test]
    async fn probe_prefers_live_props_and_takes_the_minimum() {
        use crate::db::models::{LlmEndpoint, RoleBinding};
        use crate::db::repo;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"total_slots":3}"#))
            .mount(&server)
            .await;

        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        repo::upsert_endpoint(
            &pool,
            &LlmEndpoint {
                id: "e1".to_string(),
                name: "fake".to_string(),
                base_url: server.uri(),
                api_key_ref: None,
                max_concurrency: Some(2),
                notes: None,
                last_health_at: None,
                last_health_ok: None,
                // Stale persisted props must lose against the live probe.
                props_json: Some(r#"{"total_slots":9}"#.to_string()),
            },
        )
        .await
        .expect("endpoint");
        repo::upsert_role_binding(
            &pool,
            &RoleBinding {
                id: "b1".to_string(),
                endpoint_id: "e1".to_string(),
                role: "translator".to_string(),
                model: "m".to_string(),
                params_json: "{}".to_string(),
                priority: 0,
            },
        )
        .await
        .expect("binding");

        let plan = probe_endpoint_limits(&pool).await;
        let entry = plan
            .iter()
            .find(|entry| entry.role == "translator")
            .expect("translator plan");
        assert_eq!(entry.limit, 2, "min(live slots 3, max_concurrency 2)");
        assert!(entry.reason.contains("min"), "reason: {}", entry.reason);
        assert!(plan.iter().all(|entry| entry.role != "editor"));
    }

    #[tokio::test]
    async fn probe_falls_back_to_persisted_props_when_the_server_is_down() {
        use crate::db::models::{LlmEndpoint, RoleBinding};
        use crate::db::repo;

        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        repo::upsert_endpoint(
            &pool,
            &LlmEndpoint {
                id: "e1".to_string(),
                name: "fake".to_string(),
                base_url: "http://127.0.0.1:1".to_string(),
                api_key_ref: None,
                max_concurrency: None,
                notes: None,
                last_health_at: None,
                last_health_ok: None,
                props_json: Some(r#"{"total_slots":5}"#.to_string()),
            },
        )
        .await
        .expect("endpoint");
        repo::upsert_role_binding(
            &pool,
            &RoleBinding {
                id: "b1".to_string(),
                endpoint_id: "e1".to_string(),
                role: "editor".to_string(),
                model: "m".to_string(),
                params_json: "{}".to_string(),
                priority: 0,
            },
        )
        .await
        .expect("binding");

        let plan = probe_endpoint_limits(&pool).await;
        let entry = plan
            .iter()
            .find(|entry| entry.role == "editor")
            .expect("editor plan");
        assert_eq!(entry.limit, 5, "persisted /props is the fallback");
        assert!(entry.reason.contains("total_slots"));
    }
}
