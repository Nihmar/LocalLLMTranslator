import { formatNumber } from "../../lib/format";
import type { Chapter } from "../../lib/types";

/** The chapters that become export units, in document order. */
export function ExportUnitsPanel({ chapters }: { chapters: readonly Chapter[] }) {
  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Unità da esportare</span>
        <span className="mono-chip">{formatNumber(chapters.length)} capitoli</span>
      </div>

      <div className="table-scroll" style={{ maxHeight: "22rem" }}>
        <table className="data-table">
          <thead>
            <tr>
              <th style={{ width: "4.5rem" }} className="num">
                Ordine
              </th>
              <th style={{ minWidth: "14rem" }}>Capitolo</th>
              <th style={{ width: "5rem" }}>Livello</th>
              <th style={{ width: "9rem" }} className="num">
                Blocchi
              </th>
            </tr>
          </thead>
          <tbody>
            {chapters.map((chapter) => (
              <tr key={chapter.id}>
                <td className="num">{formatNumber(chapter.order_index)}</td>
                <td>
                  <span className="block truncate text-ink-soft" title={chapter.title}>
                    {chapter.title}
                  </span>
                  <span className="font-mono text-[0.68rem] text-faint">{chapter.id}</span>
                </td>
                <td>
                  <span className="mono-chip">H{formatNumber(chapter.level)}</span>
                </td>
                <td className="num font-mono text-[0.72rem] text-muted">
                  {`${formatNumber(chapter.block_first)} – ${formatNumber(chapter.block_last)}`}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="panel-pad">
        <p className="text-[0.72rem] text-faint">
          La build impagina tutte le unità in un solo file; scegli un capitolo per generarne una
          versione autonoma. I capitoli senza modifiche vengono riutilizzati: il build salta Pandoc
          quando nulla è cambiato.
        </p>
      </div>
    </div>
  );
}
