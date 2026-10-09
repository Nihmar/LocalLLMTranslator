import { formatNumber } from "../../lib/format";
import { PASSES } from "./shared";

/** Title, counters and the controls that enqueue a new review run. */
export function ReviewHeader({
  pendingCount,
  importantCount,
  pass,
  onPass,
  withQa,
  onWithQa,
  starting,
  onStart,
}: {
  pendingCount: number;
  importantCount: number;
  pass: string;
  onPass: (value: string) => void;
  withQa: boolean;
  onWithQa: (value: boolean) => void;
  starting: boolean;
  onStart: () => void;
}) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-3">
      <div>
        <h1 className="font-serif text-2xl font-medium text-ink">Rivedi</h1>
        <p className="text-sm text-muted">
          {formatNumber(pendingCount)} proposte da decidere, di cui{" "}
          {formatNumber(importantCount)} importanti.
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <select
          className="select"
          style={{ width: "auto" }}
          aria-label="Passaggi da eseguire"
          value={pass}
          onChange={(event) => {
            onPass(event.target.value);
          }}
        >
          {PASSES.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
        <label className="flex items-center gap-1.5 text-sm text-ink-soft">
          <input
            type="checkbox"
            checked={withQa}
            onChange={(event) => {
              onWithQa(event.target.checked);
            }}
          />
          Rilancia QA
        </label>
        <button type="button" className="btn btn-primary" disabled={starting} onClick={onStart}>
          {starting ? <span className="spinner" aria-hidden="true" /> : null}
          Avvia revisione
        </button>
      </div>
    </div>
  );
}
