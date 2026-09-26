import { formatBytes, formatNumber, formatPercent, percent } from "../lib/format";
import type { ResourceMetrics } from "../lib/types";
import { StatusBadge } from "./StatusBadge";

/**
 * VRAM + slot gauge driven by `metrics_get` / `metrics://tick` (`PLAN.md` §10).
 *
 * The important behaviour is the degraded case: when the ResourceGovernor cannot schedule more
 * than one chunk at a time it says so *explicitly*, with the reason the backend reported, and
 * never as a silent slowdown.
 */

export interface ResourceGaugeProps {
  resources: ResourceMetrics | null;
  loading?: boolean | undefined;
  compact?: boolean | undefined;
}

const VRAM_SOURCE_LABEL: Readonly<Record<ResourceMetrics["vram"]["source"], string>> = {
  sysfs: "sysfs",
  "rocm-smi": "rocm-smi",
  "nvidia-smi": "nvidia-smi",
  unknown: "non rilevata",
};

export function ResourceGauge({ resources, loading = false, compact = false }: ResourceGaugeProps) {
  if (loading || resources === null) {
    return (
      <div className="panel panel-pad">
        <div className="panel-title mb-2">Risorse</div>
        <p className="flex items-center gap-2 text-xs text-muted">
          <span className="spinner" aria-hidden="true" />
          Lettura di VRAM e slot…
        </p>
      </div>
    );
  }

  const { vram, slots, max_parallel: maxParallel, degraded, degraded_reason: degradedReason } =
    resources;

  const vramKnown = vram.used_bytes !== null && vram.total_bytes !== null && vram.total_bytes > 0;
  const vramRatio = vramKnown ? percent(vram.used_bytes ?? 0, vram.total_bytes ?? 0) : 0;
  const slotsKnown = slots.total_slots !== null && slots.total_slots > 0;
  const freeSlots = slots.free_slots;
  const parallelLabel = maxParallel === 1 ? "1 chunk alla volta" : `${formatNumber(maxParallel)} chunk`;

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Risorse</span>
        <span className="flex items-center gap-2">
          <StatusBadge status={degraded ? "degraded" : "parallel"} />
          <span className="mono-chip">{parallelLabel}</span>
        </span>
      </div>

      <div className={`section-stack ${compact ? "p-2.5" : "p-4"}`}>
        {degraded ? (
          <div className="banner banner-warn" role="status">
            <span aria-hidden="true">⚠</span>
            <span>
              <strong className="font-semibold">Modalità seriale.</strong>{" "}
              {degradedReason !== null && degradedReason.length > 0
                ? degradedReason
                : "Il ResourceGovernor ha ridotto il parallelismo a un solo chunk per volta."}
            </span>
          </div>
        ) : null}

        {/* VRAM */}
        <div>
          <div className="mb-1 flex items-baseline justify-between gap-3">
            <span className="stat-label">VRAM</span>
            <span className="font-mono text-xs text-muted tabular-nums">
              {vramKnown
                ? `${formatBytes(vram.used_bytes)} / ${formatBytes(vram.total_bytes)} · ${formatPercent(vramRatio)}`
                : "non rilevata"}
            </span>
          </div>
          <div
            className="progress-track"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={vramKnown ? Math.round(vramRatio) : undefined}
            aria-label="VRAM utilizzata"
          >
            <div
              className="progress-fill"
              style={{
                width: `${vramKnown ? vramRatio : 0}%`,
                backgroundColor:
                  vramRatio >= 92
                    ? "var(--color-danger)"
                    : vramRatio >= 78
                      ? "var(--color-warn)"
                      : "var(--color-accent)",
              }}
            />
          </div>
          <p className="field-hint">
            Sorgente: {VRAM_SOURCE_LABEL[vram.source]}
            {vramKnown ? null : " — nessuna GPU leggibile, il limite è dato dai soli slot."}
          </p>
        </div>

        {/* Slots */}
        <div className="grid grid-cols-3 gap-2">
          <div className="stat-tile">
            <div className="stat-label">Slot liberi</div>
            <div className="stat-value">
              {slotsKnown ? `${formatNumber(freeSlots ?? 0)} / ${formatNumber(slots.total_slots)}` : "—"}
            </div>
          </div>
          <div className="stat-tile">
            <div className="stat-label">In volo</div>
            <div className="stat-value">{formatNumber(slots.in_flight)}</div>
          </div>
          <div className="stat-tile">
            <div className="stat-label">Parallelismo</div>
            <div className="stat-value">{formatNumber(maxParallel)}</div>
          </div>
        </div>

        {slotsKnown ? null : (
          <p className="field-hint">
            Gli slot non sono dichiarati da nessun endpoint attivo: si usa il limite per endpoint.
          </p>
        )}
      </div>
    </div>
  );
}
