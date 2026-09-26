//! Resource governor: read-only VRAM probing and slot-cap computation.
//!
//! The governor never mutates anything and never errors: it degrades to serial
//! execution with an explicit reason, reported through the `metrics://tick`
//! event.

pub mod endpoints;
pub mod vram;

pub use endpoints::{
    kinds_for_role, plan_limit, probe_endpoint_limits, role_for_kind, EndpointLimits,
    EndpointPermit, EndpointPlanEntry, EndpointUsage, LLM_ROLES, LOCAL_ROLE, ROLES,
};
pub use vram::{
    detect, max_parallel, max_parallel_with_reason, ParallelReason, VramInfo,
    DEFAULT_COST_PER_SLOT_BYTES,
};

use serde::Serialize;

/// A point-in-time view of the machine resources relevant to scheduling.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceSnapshot {
    pub vram: Option<VramInfo>,
    pub free_bytes: Option<u64>,
    /// `min(free slots, headroom estimate, user limit)`, `1` when degraded.
    pub suggested_parallel: usize,
    pub reason: ParallelReason,
}

/// Holds the operator-tunable knobs of the governor.
#[derive(Debug, Clone)]
pub struct ResourceGovernor {
    pub user_limit: Option<usize>,
    pub cost_per_slot: u64,
}

impl Default for ResourceGovernor {
    fn default() -> Self {
        Self {
            user_limit: None,
            cost_per_slot: DEFAULT_COST_PER_SLOT_BYTES,
        }
    }
}

impl ResourceGovernor {
    pub fn new(user_limit: Option<usize>, cost_per_slot: u64) -> Self {
        Self {
            user_limit,
            cost_per_slot,
        }
    }

    pub fn snapshot(&self, vram: Option<VramInfo>, free_slots: Option<usize>) -> ResourceSnapshot {
        let (suggested_parallel, reason) =
            max_parallel_with_reason(vram, free_slots, self.cost_per_slot, self.user_limit);
        ResourceSnapshot {
            free_bytes: vram.map(|v| v.free_bytes()),
            vram,
            suggested_parallel,
            reason,
        }
    }

    /// Probe the machine and compute a snapshot for the given endpoint.
    pub fn detect_snapshot(&self, free_slots: Option<usize>) -> ResourceSnapshot {
        self.snapshot(detect(), free_slots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn governor_reports_serial_reason_when_vram_missing() {
        let gov = ResourceGovernor::default();
        let snap = gov.snapshot(None, Some(4));
        assert_eq!(snap.suggested_parallel, 1);
        assert_eq!(snap.reason, ParallelReason::VramUnknown);
        assert!(snap.reason.is_serial());
    }
}
