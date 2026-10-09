import { GLOSSARY_KINDS, GLOSSARY_STATUSES, type GlossaryRowForm } from "./shared";

/** The project glossary rows, editable in place, with the two-step persistence of the parent. */
export function GlossaryEditor({
  rows,
  saving,
  onRowChange,
  onRemove,
  onAdd,
  onSave,
}: {
  rows: readonly GlossaryRowForm[];
  saving: boolean;
  onRowChange: (index: number, patch: Partial<GlossaryRowForm>) => void;
  onRemove: (index: number) => void;
  onAdd: () => void;
  onSave: () => void;
}) {
  return (
    <div>
      <div className="field-label">Glossario del progetto ({rows.length})</div>
      {rows.length > 0 ? (
        <div className="table-scroll">
          <table className="data-table">
            <thead>
              <tr>
                <th>Sorgente</th>
                <th>Traduzione</th>
                <th>Tipo</th>
                <th>Stato</th>
                <th>Nota</th>
                <th style={{ width: "3rem" }} />
              </tr>
            </thead>
            <tbody>
              {rows.map((row, index) => (
                <tr key={row.id ?? `new-${index}`}>
                  <td>
                    <input
                      className="input"
                      value={row.source}
                      onChange={(event) => {
                        onRowChange(index, { source: event.target.value });
                      }}
                    />
                  </td>
                  <td>
                    <input
                      className="input"
                      value={row.target}
                      onChange={(event) => {
                        onRowChange(index, { target: event.target.value });
                      }}
                    />
                  </td>
                  <td>
                    <select
                      className="select"
                      value={row.kind}
                      onChange={(event) => {
                        onRowChange(index, { kind: event.target.value });
                      }}
                    >
                      {GLOSSARY_KINDS.map((kind) => (
                        <option key={kind.value} value={kind.value}>
                          {kind.label}
                        </option>
                      ))}
                    </select>
                  </td>
                  <td>
                    <select
                      className="select"
                      value={row.status}
                      onChange={(event) => {
                        onRowChange(index, { status: event.target.value });
                      }}
                    >
                      {GLOSSARY_STATUSES.map((status) => (
                        <option key={status.value} value={status.value}>
                          {status.label}
                        </option>
                      ))}
                    </select>
                  </td>
                  <td>
                    <input
                      className="input"
                      value={row.note}
                      onChange={(event) => {
                        onRowChange(index, { note: event.target.value });
                      }}
                    />
                  </td>
                  <td>
                    <button
                      type="button"
                      className="btn btn-sm btn-ghost"
                      title="Rimuovi il termine"
                      onClick={() => {
                        onRemove(index);
                      }}
                    >
                      ✕
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <p className="field-hint">Nessun termine: aggiungine uno o lancia la ricognizione.</p>
      )}
      <div className="mt-2 flex items-center gap-2">
        <button type="button" className="btn btn-sm" onClick={onAdd}>
          Aggiungi termine
        </button>
        <button type="button" className="btn btn-sm btn-primary" disabled={saving} onClick={onSave}>
          {saving ? <span className="spinner" aria-hidden="true" /> : null}
          Salva glossario
        </button>
        <span className="field-hint">I termini rifiutati non entrano nel prompt.</span>
      </div>
    </div>
  );
}
