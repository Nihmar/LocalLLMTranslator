//! VRAM detection and the pure parallel-degree computation.
//!
//! Detection never fails: it returns `Option<VramInfo>` and walks the fallback
//! chain `sysfs -> rocm-smi -> nvidia-smi`. The machine this project targets is
//! AMD, so the sysfs path is tried first.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// A VRAM reading, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct VramInfo {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl VramInfo {
    pub fn free_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.used_bytes)
    }
}

/// Default estimated VRAM cost of one resident generation slot (2 GiB), used
/// when the caller has no better estimate.
pub const DEFAULT_COST_PER_SLOT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Detect VRAM by trying sysfs, then `rocm-smi`, then `nvidia-smi`.
pub fn detect() -> Option<VramInfo> {
    sysfs_scan(Path::new("/sys/class/drm"))
        .or_else(rocm_smi)
        .or_else(nvidia_smi)
}

/// Read `/sys/class/drm/card*/device/mem_info_vram_{used,total}`.
///
/// `root` is injectable so the logic is testable without a GPU.
pub fn sysfs_scan(root: &Path) -> Option<VramInfo> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut best: Option<VramInfo> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `card1` yes, connector nodes like `card1-DP-1` no.
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device = entry.path().join("device");
        let used = read_u64(&device.join("mem_info_vram_used"));
        let total = read_u64(&device.join("mem_info_vram_total"));
        if let (Some(used_bytes), Some(total_bytes)) = (used, total) {
            let info = VramInfo {
                used_bytes,
                total_bytes,
            };
            // Prefer the device with the largest VRAM.
            if best.is_none_or(|b| info.total_bytes > b.total_bytes) {
                best = Some(info);
            }
        }
    }
    best
}

fn read_u64(path: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    text.trim().parse::<u64>().ok()
}

/// Run `rocm-smi --showmeminfo vram --json`.
pub fn rocm_smi() -> Option<VramInfo> {
    let out = std::process::Command::new("rocm-smi")
        .args(["--showmeminfo", "vram", "--json"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    rocm_smi_parse(&String::from_utf8_lossy(&out.stdout))
}

/// Parse the JSON produced by `rocm-smi --showmeminfo vram --json`. Field names
/// vary between ROCm versions, so we search recursively for the two byte counts.
pub fn rocm_smi_parse(json: &str) -> Option<VramInfo> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut total = None;
    let mut used = None;
    collect_vram(&value, &mut total, &mut used);
    match (used, total) {
        (Some(used_bytes), Some(total_bytes)) => Some(VramInfo {
            used_bytes,
            total_bytes,
        }),
        _ => None,
    }
}

fn collect_vram(value: &serde_json::Value, total: &mut Option<u64>, used: &mut Option<u64>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                if let Some(n) = value_to_u64(val) {
                    if key.contains("Total Used Memory") {
                        used.get_or_insert(n);
                    } else if key.contains("Total Memory") {
                        total.get_or_insert(n);
                    }
                }
                collect_vram(val, total, used);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_vram(item, total, used);
            }
        }
        _ => {}
    }
}

/// Run `nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader,nounits`.
pub fn nvidia_smi() -> Option<VramInfo> {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    nvidia_smi_parse(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `memory.used,memory.total` in MiB (the `nounits` format).
pub fn nvidia_smi_parse(stdout: &str) -> Option<VramInfo> {
    let line = stdout.lines().find(|l| l.contains(','))?;
    let mut parts = line.split(',').map(str::trim);
    let used_mib = parts.next()?.parse::<u64>().ok()?;
    let total_mib = parts.next()?.parse::<u64>().ok()?;
    const MIB: u64 = 1024 * 1024;
    Some(VramInfo {
        used_bytes: used_mib * MIB,
        total_bytes: total_mib * MIB,
    })
}

fn value_to_u64(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

/// Reason the parallel degree was capped; surfaced to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ParallelReason {
    /// Full parallelism is available.
    Ok,
    /// VRAM is unknown -> serial degradation.
    VramUnknown,
    /// Not enough free VRAM for a second slot -> serial degradation.
    InsufficientHeadroom,
    /// The endpoint exposes a single slot, or the user limited concurrency.
    SlotLimited,
}

impl ParallelReason {
    pub fn is_serial(self) -> bool {
        !matches!(self, ParallelReason::Ok)
    }
}

/// Pure computation of the maximum useful parallelism for one endpoint.
///
/// `max_parallel = min(free slots, floor(free VRAM / cost per slot), user limit)`,
/// and it degrades to `1` (serial, without error) whenever information is missing
/// or insufficient. This is the injectable function the tests exercise.
pub fn max_parallel(
    vram: Option<VramInfo>,
    free_slots: Option<usize>,
    cost_per_slot: u64,
    user_limit: Option<usize>,
) -> usize {
    let Some(vram) = vram else {
        return 1;
    };
    if cost_per_slot == 0 {
        return 1;
    }
    let headroom_slots = (vram.free_bytes() / cost_per_slot) as usize;
    if headroom_slots < 2 {
        // Zero or a single slot of headroom: run serially.
        return 1;
    }
    // A missing or zero slot count means we cannot safely run in parallel.
    let slot_bound = free_slots.filter(|s| *s > 0).unwrap_or(1);
    let user = user_limit.filter(|u| *u > 0).unwrap_or(usize::MAX);
    headroom_slots.min(slot_bound).min(user).max(1)
}

/// Same as [`max_parallel`] but also reports why the degree was capped.
pub fn max_parallel_with_reason(
    vram: Option<VramInfo>,
    free_slots: Option<usize>,
    cost_per_slot: u64,
    user_limit: Option<usize>,
) -> (usize, ParallelReason) {
    let degree = max_parallel(vram, free_slots, cost_per_slot, user_limit);
    let reason = if degree > 1 {
        ParallelReason::Ok
    } else if vram.is_none() {
        ParallelReason::VramUnknown
    } else if cost_per_slot == 0
        || (vram.map(|v| v.free_bytes()).unwrap_or(0) / cost_per_slot.max(1)) < 2
    {
        ParallelReason::InsufficientHeadroom
    } else {
        ParallelReason::SlotLimited
    };
    (degree, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn serial_when_vram_unknown() {
        assert_eq!(max_parallel(None, Some(4), GIB, None), 1);
        assert_eq!(
            max_parallel_with_reason(None, Some(4), GIB, None).1,
            ParallelReason::VramUnknown
        );
    }

    #[test]
    fn serial_when_headroom_insufficient() {
        let vram = VramInfo {
            used_bytes: 15 * GIB,
            total_bytes: 16 * GIB,
        };
        // Only ~1 GiB free, cost 2 GiB/slot -> cannot fit a second slot.
        assert_eq!(max_parallel(Some(vram), Some(4), 2 * GIB, None), 1);
        assert_eq!(
            max_parallel_with_reason(Some(vram), Some(4), 2 * GIB, None).1,
            ParallelReason::InsufficientHeadroom
        );
    }

    #[test]
    fn serial_when_endpoint_has_one_slot() {
        let vram = VramInfo {
            used_bytes: 0,
            total_bytes: 32 * GIB,
        };
        assert_eq!(max_parallel(Some(vram), Some(1), 2 * GIB, None), 1);
        assert_eq!(
            max_parallel_with_reason(Some(vram), Some(1), 2 * GIB, None).1,
            ParallelReason::SlotLimited
        );
    }

    #[test]
    fn serial_when_user_limit_is_one() {
        let vram = VramInfo {
            used_bytes: 0,
            total_bytes: 64 * GIB,
        };
        assert_eq!(max_parallel(Some(vram), Some(4), GIB, Some(1)), 1);
    }

    #[test]
    fn full_parallelism_when_everything_allows() {
        let vram = VramInfo {
            used_bytes: 4 * GIB,
            total_bytes: 32 * GIB,
        };
        // 28 GiB free / 2 GiB per slot = 14 headroom slots, capped by 4 free slots.
        assert_eq!(max_parallel(Some(vram), Some(4), 2 * GIB, None), 4);
        assert_eq!(
            max_parallel_with_reason(Some(vram), Some(4), 2 * GIB, None).1,
            ParallelReason::Ok
        );
    }

    #[test]
    fn sysfs_scan_reads_card_nodes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let device = dir.path().join("card1").join("device");
        fs::create_dir_all(&device).expect("mkdir");
        fs::write(device.join("mem_info_vram_used"), "1073741824\n").expect("write");
        fs::write(device.join("mem_info_vram_total"), "17179869184\n").expect("write");
        // A connector node that must be ignored.
        fs::create_dir_all(dir.path().join("card1-DP-1")).expect("mkdir");
        let info = sysfs_scan(dir.path()).expect("some");
        assert_eq!(info.used_bytes, GIB);
        assert_eq!(info.total_bytes, 16 * GIB);
        assert_eq!(info.free_bytes(), 15 * GIB);
    }

    #[test]
    fn rocm_smi_json_is_parsed_leniently() {
        let json = r#"{"card0":{"VRAM Total Memory (B)":"17179869184","VRAM Total Used Memory (B)":"2147483648","Other":"x"}}"#;
        let info = rocm_smi_parse(json).expect("some");
        assert_eq!(info.total_bytes, 16 * GIB);
        assert_eq!(info.used_bytes, 2 * GIB);
    }

    #[test]
    fn nvidia_smi_csv_is_parsed() {
        let out = "2048, 16384\n";
        let info = nvidia_smi_parse(out).expect("some");
        assert_eq!(info.used_bytes, 2048 * 1024 * 1024);
        assert_eq!(info.total_bytes, 16384 * 1024 * 1024);
    }
}
