import { formatNumber } from "../../lib/format";
import { STATUS_FILTERS } from "./shared";

/** Page header: what the run covers now, and the four queue controls. */
export function TranslateControls({
  projectName,
  chunksTotal,
  visibleCount,
  limit,
  statusFilter,
  onStatusFilter,
  control,
  loading,
  onStart,
  onRetry,
  onPause,
  onCancel,
  onReload,
}: {
  projectName: string;
  chunksTotal: number;
  visibleCount: number;
  limit: number;
  statusFilter: string;
  onStatusFilter: (value: string) => void;
  control: "idle" | "starting" | "pausing" | "cancelling";
  loading: boolean;
  onStart: () => void;
  onRetry: () => void;
  onPause: () => void;
  onCancel: () => void;
  onReload: () => void;
}) {
  const busy = control !== "idle";
  return (
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h1 className="font-serif text-2xl font-medium text-ink">Traduci</h1>
        <p className="mt-0.5 text-xs text-muted">
          Progetto <span className="font-semibold text-ink-soft">{projectName}</span> —{" "}
          {formatNumber(chunksTotal)} chunk nel progetto
          {visibleCount !== chunksTotal
            ? ` · ${formatNumber(visibleCount)} con lo stato scelto`
            : ""}
          {visibleCount > limit ? ` · mostrati ${formatNumber(limit)}` : ""}.
        </p>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-1 text-xs text-muted">
          Stato
          <select
            className="select"
            style={{ width: "auto" }}
            value={statusFilter}
            onChange={(event) => {
              onStatusFilter(event.target.value);
            }}
          >
            {STATUS_FILTERS.map((filter) => (
              <option key={filter.value} value={filter.value}>
                {filter.label}
              </option>
            ))}
          </select>
        </label>

        <button type="button" className="btn btn-primary" disabled={busy} onClick={onStart}>
          {control === "starting" ? <span className="spinner" aria-hidden="true" /> : null}
          Avvia / Riprendi
        </button>

        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={onRetry}
          title="Rimette in coda solo i chunk falliti o da rivedere"
        >
          Riprova falliti
        </button>

        <button type="button" className="btn" disabled={busy} onClick={onPause}>
          {control === "pausing" ? <span className="spinner" aria-hidden="true" /> : null}
          Pausa
        </button>

        <button type="button" className="btn" disabled={busy} onClick={onCancel}>
          {control === "cancelling" ? <span className="spinner" aria-hidden="true" /> : null}
          Annulla
        </button>

        <button
          type="button"
          className="btn"
          disabled={loading}
          onClick={onReload}
          title="Rilegge la tabella dal database"
        >
          Aggiorna
        </button>
      </div>
    </div>
  );
}
