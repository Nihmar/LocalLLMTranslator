import { StatusBadge } from "../../components/StatusBadge";
import { basename, countLabel, formatDuration, formatNumber } from "../../lib/format";
import type { ExportOutcome } from "../../lib/types";
import { parentDirectory } from "./shared";

/** The build outcome summary, with the actions that reveal the produced file. */
export function ExportResultPanel({
  result,
  openError,
  onOpen,
}: {
  result: ExportOutcome;
  openError: string | null;
  onOpen: (path: string) => void;
}) {
  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Risultato</span>
        <span className="flex items-center gap-2">
          <StatusBadge status={result.from_cache ? "cached" : "done"} />
          <span className="mono-chip">{formatDuration(result.duration_ms)}</span>
        </span>
      </div>

      <div className="panel-pad section-stack">
        <div className={result.from_cache ? "banner" : "banner banner-ok"} role="status">
          <span aria-hidden="true">{result.from_cache ? "↺" : "✓"}</span>
          <span>
            {result.from_cache
              ? "Nessuna modifica: build saltata, output riutilizzato"
              : "File generato"}
            {` (${countLabel(result.units, "unità", "unità")}): `}
            <span className="font-mono text-ink">{result.output_path}</span>
          </span>
        </div>

        {!result.from_cache ? (
          <p className="field-hint">
            Capitoli ricostruiti: {formatNumber(result.changed_units.length)} · riutilizzati:{" "}
            {formatNumber(result.reused_units)}
          </p>
        ) : null}

        {openError !== null ? (
          <div className="banner banner-error" role="alert">
            <span aria-hidden="true">⚠</span>
            <span>{openError}</span>
          </div>
        ) : null}

        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => {
              onOpen(result.output_path);
            }}
          >
            Apri output
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              onOpen(parentDirectory(result.output_path));
            }}
          >
            Apri cartella
          </button>
          <span className="mono-chip" title={result.output_path}>
            {basename(result.output_path)}
          </span>
        </div>

        <div>
          <div className="stat-label mb-1">Log di Pandoc</div>
          <pre className="max-h-64 overflow-auto rounded-md border border-line bg-canvas p-3 font-mono text-[0.7rem] whitespace-pre-wrap text-ink-soft">
            {result.log.length === 0 ? "— nessun output —" : result.log}
          </pre>
        </div>
      </div>
    </div>
  );
}
