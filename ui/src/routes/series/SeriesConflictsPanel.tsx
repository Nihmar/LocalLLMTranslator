import { StatusBadge } from "../../components/StatusBadge";
import { qaFindingSetStatus, seriesGlossaryUpsert } from "../../lib/ipc";
import type { Project, QaFinding, SeriesDetail } from "../../lib/types";
import { parseConflictDetails, type ConflictDetails, type SeriesPanelProps } from "./shared";

/**
 * The `glossary_conflict` findings of the member books: adopt the book rendering into the
 * canon, keep the canon, or mark the finding resolved.
 */
export function SeriesConflictsPanel({
  detail,
  conflicts,
  showClosed,
  onShowClosed,
  busy,
  run,
  onChanged,
}: SeriesPanelProps & {
  detail: SeriesDetail;
  conflicts: ReadonlyArray<{ project: Project; finding: QaFinding }>;
  showClosed: boolean;
  onShowClosed: (show: boolean) => void;
  onChanged: () => Promise<void>;
}) {
  async function handleSetStatus(finding: QaFinding, status: string): Promise<void> {
    await run(`conflict:${finding.id}`, async () => {
      await qaFindingSetStatus(finding.id, status);
      await onChanged();
      return status === "open"
        ? "Conflitto riaperto."
        : status === "resolved"
          ? "Conflitto segnato come risolto."
          : "Canone mantenuto: il conflitto è chiuso, il libro resta sovrano.";
    });
  }

  /** Adopt the book's rendering into the canon, then close the finding. */
  async function handleAdopt(finding: QaFinding, details: ConflictDetails): Promise<void> {
    if (details.source === null || details.projectTarget === null) {
      return;
    }
    const source = details.source;
    const target = details.projectTarget;
    await run(`conflict-adopt:${finding.id}`, async () => {
      const existing = detail.glossary.find(
        (term) => term.source.trim().toLowerCase() === source.trim().toLowerCase(),
      );
      await seriesGlossaryUpsert({
        id: existing?.id ?? null,
        series_id: detail.series.id,
        source,
        target,
        kind: existing?.kind ?? "term",
        note: existing?.note ?? null,
        status: "approved",
        expected_revision: existing?.revision ?? null,
      });
      await qaFindingSetStatus(finding.id, "resolved");
      await onChanged();
      return `Canone aggiornato: «${source}» ora è «${target}»; il conflitto è risolto.`;
    });
  }

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Conflitti di canone</span>
        <span className="flex items-center gap-2">
          <label className="flex items-center gap-1 text-[0.68rem] text-muted">
            <input
              type="checkbox"
              checked={showClosed}
              onChange={(event) => {
                onShowClosed(event.target.checked);
              }}
            />
            Mostra chiusi
          </label>
          <span className="mono-chip">{conflicts.length}</span>
        </span>
      </div>
      <div className="panel-pad section-stack">
        {conflicts.length === 0 ? (
          <p className="field-hint">
            Nessun conflitto: i libri rendono i termini di serie come il canone.
          </p>
        ) : (
          <ul className="section-stack">
            {conflicts.map(({ project, finding }) => {
              const details = parseConflictDetails(finding.details_json);
              const closed = finding.status !== "open";
              const adoptable =
                details.projectTarget !== null &&
                (details.scope === "series" || details.scope === "series_promote");
              return (
                <li key={finding.id} className="rounded border border-line p-2 text-xs">
                  <span className="flex flex-wrap items-center gap-1">
                    <span className="badge badge-warning">{project.name}</span>
                    {details.source !== null ? (
                      <span className="font-medium text-ink">{details.source}</span>
                    ) : null}
                    <span className="badge badge-neutral">{finding.kind}</span>
                    <StatusBadge status={finding.status} />
                    {details.scope !== null ? (
                      <span className="text-[0.65rem] text-faint">{details.scope}</span>
                    ) : null}
                  </span>
                  {details.seriesTarget !== null || details.projectTarget !== null ? (
                    <p className="mt-1 text-[0.72rem] text-muted">
                      canone: <strong>{details.seriesTarget ?? "—"}</strong> · libro:{" "}
                      <strong>{details.projectTarget ?? "—"}</strong>
                    </p>
                  ) : (
                    <p className="mt-1 font-mono text-[0.65rem] break-all text-muted">
                      {finding.details_json}
                    </p>
                  )}
                  <div className="mt-1 flex flex-wrap items-center gap-1">
                    {!closed ? (
                      <>
                        {adoptable ? (
                          <button
                            type="button"
                            className="btn btn-sm btn-primary"
                            disabled={busy}
                            onClick={() => void handleAdopt(finding, details)}
                          >
                            Adotta «{details.projectTarget}» nel canone
                          </button>
                        ) : null}
                        <button
                          type="button"
                          className="btn btn-sm"
                          disabled={busy}
                          onClick={() => void handleSetStatus(finding, "ignored")}
                        >
                          Mantieni il canone
                        </button>
                        <button
                          type="button"
                          className="btn btn-sm btn-ghost"
                          disabled={busy}
                          onClick={() => void handleSetStatus(finding, "resolved")}
                        >
                          Segna risolto
                        </button>
                      </>
                    ) : (
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={busy}
                        onClick={() => void handleSetStatus(finding, "open")}
                      >
                        Riapri
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
