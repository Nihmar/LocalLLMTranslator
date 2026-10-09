import { formatNumber } from "../../lib/format";
import type { ExportBuildRecord } from "../../lib/types";
import { timestampLabel } from "./shared";

/** The recent builds of the project, newest first. */
export function ExportHistoryPanel({ history }: { history: readonly ExportBuildRecord[] }) {
  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Cronologia build</span>
        <span className="mono-chip">{formatNumber(history.length)}</span>
      </div>
      {history.length === 0 ? (
        <div className="panel-pad">
          <p className="field-hint">Nessuna build registrata per questo progetto.</p>
        </div>
      ) : (
        <div className="table-scroll" style={{ maxHeight: "18rem" }}>
          <table className="data-table">
            <thead>
              <tr>
                <th>Quando</th>
                <th>Formato</th>
                <th>Ambito</th>
                <th className="num">Unità</th>
                <th>Esito</th>
              </tr>
            </thead>
            <tbody>
              {history.map((record) => (
                <tr key={record.id}>
                  <td className="font-mono text-[0.7rem] text-muted">
                    {timestampLabel(record.built_at)}
                  </td>
                  <td>
                    <span className="mono-chip">{record.output_format}</span>
                  </td>
                  <td className="text-xs text-ink-soft">
                    {record.chapter_id === null ? "libro" : record.chapter_id}
                  </td>
                  <td className="num font-mono text-[0.72rem] text-muted">
                    {formatNumber(record.units)}
                  </td>
                  <td>
                    {record.from_cache ? (
                      <span className="badge badge-neutral">saltata</span>
                    ) : (
                      <span className="badge badge-success">
                        {formatNumber(record.changed_units.length)} modificate
                      </span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
