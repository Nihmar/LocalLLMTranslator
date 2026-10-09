import { formatNumber } from "../../lib/format";
import type { PassFilter, SeverityFilter, Tab } from "./shared";

/** Tabs (open / decided / QA) and, for the inbox tabs, the severity and pass filters. */
export function ReviewTabs({
  tab,
  onTab,
  pendingCount,
  decidedCount,
  findingsCount,
  severity,
  onSeverity,
  passFilter,
  onPassFilter,
}: {
  tab: Tab;
  onTab: (tab: Tab) => void;
  pendingCount: number;
  decidedCount: number;
  findingsCount: number;
  severity: SeverityFilter;
  onSeverity: (value: SeverityFilter) => void;
  passFilter: PassFilter;
  onPassFilter: (value: PassFilter) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <div role="tablist" aria-label="Sezione" className="segmented">
        {(
          [
            ["open", `Da decidere · ${formatNumber(pendingCount)}`],
            ["decided", `Decise · ${formatNumber(decidedCount)}`],
            ["qa", `Controlli QA · ${formatNumber(findingsCount)}`],
          ] as const
        ).map(([id, label]) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            className="segmented-item"
            onClick={() => {
              onTab(id);
            }}
          >
            {label}
          </button>
        ))}
      </div>
      {tab === "qa" ? null : (
        <div className="flex flex-wrap gap-2">
          {tab === "open"
            ? (
                [
                  ["important", "Importanti"],
                  ["minor", "Minori"],
                  ["all", "Tutte"],
                ] as const
              ).map(([id, label]) => (
                <button
                  key={id}
                  type="button"
                  className="chip"
                  aria-pressed={severity === id}
                  onClick={() => {
                    onSeverity(id);
                  }}
                >
                  {label}
                </button>
              ))
            : null}
          <select
            className="select"
            style={{ width: "auto" }}
            aria-label="Passaggio"
            value={passFilter}
            onChange={(event) => {
              const value = event.target.value;
              onPassFilter(value === "editor" || value === "proofreader" ? value : "all");
            }}
          >
            <option value="all">Editor e proofreader</option>
            <option value="editor">Solo editor</option>
            <option value="proofreader">Solo proofreader</option>
          </select>
        </div>
      )}
      {tab === "open" ? (
        <span className="ml-auto text-xs text-muted">
          <kbd className="kbd">J</kbd> <kbd className="kbd">K</kbd> scorri ·{" "}
          <kbd className="kbd">A</kbd> accetta · <kbd className="kbd">R</kbd> rifiuta
        </span>
      ) : null}
    </div>
  );
}
