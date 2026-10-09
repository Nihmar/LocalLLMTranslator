import { formatNumber } from "../../lib/format";
import type { ExportProgressEvent } from "../../lib/types";
import { progressLabel } from "./shared";

/** What stands between "Genera output" and the result: refusals, progress and warnings. */
export function BuildProgressPanel({
  warning,
  buildError,
  building,
  progress,
  hasResult,
  onExportAnyway,
  onDismissWarning,
}: {
  /** Set when a build would emit untranslated chunks in the source language. */
  warning: { missing: number; total: number } | null;
  buildError: string | null;
  building: boolean;
  progress: ExportProgressEvent | null;
  /** A result panel already describes the last build: the bare progress line is redundant. */
  hasResult: boolean;
  onExportAnyway: () => void;
  onDismissWarning: () => void;
}) {
  return (
    <>
      {warning !== null ? (
        <div className="banner banner-warn" role="alert">
          <span aria-hidden="true">⚠</span>
          <div className="flex flex-col gap-2">
            <span>
              {formatNumber(warning.missing)} parti su {formatNumber(warning.total)} non sono
              tradotte: nel file uscirebbero nella lingua originale.
            </span>
            <div className="flex flex-wrap gap-2">
              <button type="button" className="btn" onClick={onExportAnyway}>
                Esporta comunque
              </button>
              <button type="button" className="btn" onClick={onDismissWarning}>
                Annulla
              </button>
            </div>
          </div>
        </div>
      ) : null}

      {buildError !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{buildError}</span>
        </div>
      ) : null}

      {building ? (
        <div className="panel panel-pad">
          <p className="flex items-center gap-2 text-xs text-muted">
            <span className="spinner" aria-hidden="true" />
            {progress === null ? "Avvio della build…" : progressLabel(progress)}
          </p>
        </div>
      ) : progress !== null && !hasResult ? (
        <div className="panel panel-pad">
          <p className="text-xs text-muted">{progressLabel(progress)}</p>
        </div>
      ) : null}
    </>
  );
}
