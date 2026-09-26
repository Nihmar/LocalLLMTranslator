import { formatBytes, formatNumber, formatPercent, percent } from "../lib/format";
import type { Metrics, ParallelReason } from "../lib/types";
import { StatusBadge } from "./StatusBadge";

/**
 * VRAM + queue gauge driven by `metrics_get` / `metrics://tick` (`PLAN.md` §10).
 *
 * The backend reports a resource snapshot, not a scheduling decision: `suggested_parallel` is
 * computed by the ResourceGovernor and `reason` says why it was capped. The degraded case is
 * rendered *explicitly*, with the reported reason, and never as a silent slowdown.
 */

export interface ResourceGaugeProps {
  metrics: Metrics | null;
  loading?: boolean | undefined;
  compact?: boolean | undefined;
}

/** Italian explanation of each `ParallelReason` (`resources::vram::ParallelReason`). */
const REASON_HINT: Readonly<Record<ParallelReason, string>> = {
  ok: "",
  vram_unknown: "VRAM non rilevata: nessuna GPU leggibile, il limite è dato dai soli slot.",
  insufficient_headroom:
    "VRAM libera insufficiente per un secondo slot: i chunk procedono uno alla volta.",
  slot_limited:
    "L'endpoint espone un solo slot (o il limite utente è 1): un chunk alla volta.",
};

export function ResourceGauge({ metrics, loading = false, compact = false }: ResourceGaugeProps) {
  if (loading || metrics === null) {
    return (
      <div className="panel panel-pad">
        <div className="panel-title mb-2">Risorse</div>
        <p className="flex items-center gap-2 text-xs text-muted">
          <span className="spinner" aria-hidden="true" />
          Lettura di VRAM e coda…
        </p>
      </div>
    );
  }

  const degraded = metrics.reason !== "ok";
  const vram = metrics.vram;
  const vramKnown = vram !== null && vram.total_bytes > 0;
  const vramRatio = vramKnown ? percent(vram.used_bytes, vram.total_bytes) : 0;
  const parallelLabel =
    metrics.suggested_parallel === 1 ? "1 chunk alla volta" : `${formatNumber(metrics.suggested_parallel)} chunk`;

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Risorse</span>
        <span className="flex items-center gap-2">
          <StatusBadge status={degraded ? "degraded" : "parallel"} />
          {metrics.worker_paused ? <span className="badge badge-warning">In pausa</span> : null}
          <span className="mono-chip">{parallelLabel}</span>
        </span>
      </div>

      <div className={`section-stack ${compact ? "p-2.5" : "p-4"}`}>
        {degraded ? (
          <div className="banner banner-warn" role="status">
            <span aria-hidden="true">⚠</span>
            <span>
              <strong className="font-semibold">Modalità seriale.</strong>{" "}
              {REASON_HINT[metrics.reason]}
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
            {vramKnown
              ? `Libera: ${formatBytes(metrics.free_bytes)}.`
              : REASON_HINT.vram_unknown}
          </p>
        </div>

        {/* Queue */}
        <div className="grid grid-cols-3 gap-2">
          <div className="stat-tile">
            <div className="stat-label">In volo sul sidecar</div>
            <div className="stat-value">{formatNumber(metrics.sidecar_in_flight)}</div>
          </div>
          <div className="stat-tile">
            <div className="stat-label">Parallelismo</div>
            <div className="stat-value">{formatNumber(metrics.suggested_parallel)}</div>
          </div>
          <div className="stat-tile">
            <div className="stat-label">Esecutore</div>
            <div className="stat-value" style={{ fontSize: "0.85rem" }}>
              {metrics.worker_running ? "attivo" : "fermo"}
            </div>
          </div>
        </div>

        <div>
          <div className="stat-label mb-1">Coda per stato</div>
          {metrics.jobs.length === 0 ? (
            <p className="text-[0.72rem] text-faint">Nessun job in coda.</p>
          ) : (
            <ul className="flex flex-wrap gap-1">
              {metrics.jobs.map((job) => (
                <li key={job.state}>
                  <span className="mono-chip">
                    {job.state}: {formatNumber(job.count)}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>

        {/* Per-endpoint LLM capacity: a capped sub-agent must be explained,
            not look like a slow machine. */}
        {metrics.endpoints.length > 0 ? (
          <div>
            <div className="stat-label mb-1">Slot per endpoint (in uso / limite)</div>
            <ul className="space-y-1">
              {metrics.endpoints.map((endpoint) => (
                <li
                  key={endpoint.role}
                  className="flex items-center justify-between gap-2 text-xs"
                  title={endpoint.reason}
                >
                  <span className="flex items-center gap-1">
                    <span className="mono-chip">{endpoint.role}</span>
                    {endpoint.endpoint_id === null ? (
                      <span className="badge badge-neutral">non assegnato</span>
                    ) : null}
                  </span>
                  <span className="font-mono text-muted tabular-nums">
                    {formatNumber(endpoint.in_flight)} / {formatNumber(endpoint.limit)}
                  </span>
                </li>
              ))}
            </ul>
            <p className="field-hint">
              Limite = min (max_concurrency, slot riportati da /props). Passa il mouse su una riga
              per il motivo.
            </p>
          </div>
        ) : null}
      </div>
    </div>
  );
}
