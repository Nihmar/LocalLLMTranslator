import { useState } from "react";
import { EmptyState } from "../../components/EmptyState";
import { FormField } from "../../components/FormField";
import { seriesCreate } from "../../lib/ipc";
import type { Series } from "../../lib/types";
import type { Run } from "./shared";

/** The list of series and the form that creates one. */
export function SeriesSidebar({
  series,
  loading,
  selectedId,
  onSelect,
  busy,
  run,
  onCreated,
}: {
  series: readonly Series[];
  loading: boolean;
  selectedId: string | null;
  onSelect: (id: string) => void;
  busy: boolean;
  run: Run;
  /** Reloads the list and selects the new series. */
  onCreated: (id: string) => Promise<void>;
}) {
  const [newName, setNewName] = useState("");
  const [newSourceLang, setNewSourceLang] = useState("");
  const [newTargetLang, setNewTargetLang] = useState("");

  async function handleCreate(): Promise<void> {
    await run("create", async () => {
      const created = await seriesCreate({
        name: newName.trim(),
        source_lang: newSourceLang.trim().length > 0 ? newSourceLang.trim() : null,
        target_lang: newTargetLang.trim().length > 0 ? newTargetLang.trim() : null,
      });
      setNewName("");
      setNewSourceLang("");
      setNewTargetLang("");
      await onCreated(created.id);
      return `Serie «${created.name}» creata.`;
    });
  }

  return (
    <div className="section-stack min-w-0">
      <div className="panel">
        <div className="panel-head">
          <span className="panel-title">Serie</span>
          <span className="mono-chip">{series.length}</span>
        </div>
        {loading ? (
          <div className="panel-pad">
            <EmptyState tone="loading" compact title="Lettura delle serie…" />
          </div>
        ) : series.length === 0 ? (
          <div className="panel-pad">
            <p className="field-hint">Nessuna serie: creane una qui sotto.</p>
          </div>
        ) : (
          <ul className="section-stack p-2">
            {series.map((entry) => (
              <li key={entry.id}>
                <button
                  type="button"
                  className="nav-item w-full"
                  data-selected={entry.id === selectedId}
                  aria-current={entry.id === selectedId ? "true" : undefined}
                  onClick={() => {
                    onSelect(entry.id);
                  }}
                >
                  <span className="min-w-0">
                    <span className="block truncate text-[0.82rem] font-medium">{entry.name}</span>
                    <span className="block truncate text-[0.68rem] text-faint">
                      {entry.source_lang ?? "?"} → {entry.target_lang ?? "?"}
                    </span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="panel">
        <div className="panel-head">
          <span className="panel-title">Nuova serie</span>
        </div>
        <div className="panel-pad section-stack">
          <FormField label="Nome" htmlFor="series-name" required>
            <input
              id="series-name"
              className="input"
              value={newName}
              onChange={(event) => {
                setNewName(event.target.value);
              }}
              placeholder="La saga del Porto"
            />
          </FormField>
          <div className="grid grid-cols-2 gap-2">
            <FormField label="Lingua di partenza" htmlFor="series-source">
              <input
                id="series-source"
                className="input"
                value={newSourceLang}
                onChange={(event) => {
                  setNewSourceLang(event.target.value);
                }}
                placeholder="en"
              />
            </FormField>
            <FormField label="Lingua di arrivo" htmlFor="series-target">
              <input
                id="series-target"
                className="input"
                value={newTargetLang}
                onChange={(event) => {
                  setNewTargetLang(event.target.value);
                }}
                placeholder="it"
              />
            </FormField>
          </div>
          <button
            type="button"
            className="btn btn-primary"
            disabled={busy || newName.trim().length === 0}
            onClick={() => void handleCreate()}
          >
            Crea serie
          </button>
          <p className="field-hint">
            La coppia linguistica è quella che tutti i libri della serie devono condividere.
          </p>
        </div>
      </div>
    </div>
  );
}
