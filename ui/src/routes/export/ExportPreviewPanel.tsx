import { useEffect, useState } from "react";
import { formatNumber } from "../../lib/format";
import type { ExportPreview } from "../../lib/types";

/** The composed Markdown of the selected unit, with the translation counters. */
export function ExportPreviewPanel({
  preview,
  error,
  onClose,
}: {
  preview: ExportPreview | null;
  error: string | null;
  onClose: () => void;
}) {
  const [unitIndex, setUnitIndex] = useState(0);
  const selected = preview?.units[unitIndex] ?? null;

  // A new preview starts from its first unit.
  useEffect(() => {
    setUnitIndex(0);
  }, [preview]);

  if (preview === null) {
    return error === null ? null : (
      <div className="banner banner-error" role="alert">
        <span aria-hidden="true">⚠</span>
        <span>{error}</span>
      </div>
    );
  }

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Anteprima</span>
        <span className="flex items-center gap-2">
          <select
            className="select"
            style={{ width: "auto", maxWidth: "20rem" }}
            value={String(unitIndex)}
            onChange={(event) => {
              setUnitIndex(Number(event.target.value));
            }}
          >
            {preview.units.map((unit, index) => (
              <option key={unit.key} value={String(index)}>
                {unit.title}
              </option>
            ))}
          </select>
          <span className="mono-chip">
            {formatNumber(preview.total_chunks - preview.untranslated_chunks)}/
            {formatNumber(preview.total_chunks)} tradotti
          </span>
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => {
              onClose();
            }}
          >
            Chiudi
          </button>
        </span>
      </div>
      <div className="panel-pad section-stack">
        {error !== null ? (
          <div className="banner banner-error" role="alert">
            <span aria-hidden="true">⚠</span>
            <span>{error}</span>
          </div>
        ) : null}
        {selected !== null ? (
          <>
            <div className="grid grid-cols-3 gap-2">
              <div className="stat-tile">
                <div className="stat-label">Unità</div>
                <div className="truncate text-xs text-ink-soft">{selected.title}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Chunk</div>
                <div className="stat-value">{formatNumber(selected.chunks)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Da tradurre</div>
                <div className="stat-value">{formatNumber(selected.untranslated)}</div>
              </div>
            </div>
            <pre className="max-h-96 overflow-auto rounded-md border border-line bg-canvas p-3 font-mono text-[0.7rem] whitespace-pre-wrap text-ink-soft">
              {selected.markdown}
            </pre>
            <details>
              <summary className="cursor-pointer text-xs text-muted">metadata.yaml</summary>
              <pre className="mt-2 max-h-48 overflow-auto rounded-md border border-line bg-canvas p-3 font-mono text-[0.7rem] whitespace-pre-wrap text-ink-soft">
                {preview.metadata_yaml}
              </pre>
            </details>
          </>
        ) : null}
      </div>
    </div>
  );
}
