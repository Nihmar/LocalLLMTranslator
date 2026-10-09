import { TERM_KINDS, type TermForm } from "./shared";

/** The proper nouns the reconnaissance proposed: accept, retype, translate or annotate. */
export function ProposedNounsTable({
  terms,
  onTermChange,
}: {
  terms: readonly TermForm[];
  onTermChange: (index: number, patch: Partial<TermForm>) => void;
}) {
  if (terms.length === 0) {
    return null;
  }
  return (
    <div>
      <div className="field-label">Nomi propri proposti</div>
      <div className="table-scroll">
        <table className="data-table">
          <thead>
            <tr>
              <th style={{ width: "4rem" }}>Usa</th>
              <th>Nome</th>
              <th>Tipo</th>
              <th>Traduzione</th>
              <th>Nota</th>
            </tr>
          </thead>
          <tbody>
            {terms.map((term, index) => (
              <tr key={term.source}>
                <td>
                  <input
                    type="checkbox"
                    checked={term.accepted}
                    onChange={(event) => {
                      onTermChange(index, { accepted: event.target.checked });
                    }}
                  />
                </td>
                <td>
                  <span className="mono-chip">{term.source}</span>
                </td>
                <td>
                  <select
                    className="select"
                    value={term.kind}
                    onChange={(event) => {
                      onTermChange(index, { kind: event.target.value });
                    }}
                  >
                    {TERM_KINDS.map((kind) => (
                      <option key={kind.value} value={kind.value}>
                        {kind.label}
                      </option>
                    ))}
                  </select>
                </td>
                <td>
                  <input
                    className="input"
                    value={term.target}
                    onChange={(event) => {
                      onTermChange(index, { target: event.target.value });
                    }}
                  />
                </td>
                <td>
                  <input
                    className="input"
                    value={term.note}
                    onChange={(event) => {
                      onTermChange(index, { note: event.target.value });
                    }}
                  />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
