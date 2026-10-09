import { useEffect, useState } from "react";
import { FormField } from "../../components/FormField";
import { seriesDelete, seriesUpdate } from "../../lib/ipc";
import type { SeriesDetail } from "../../lib/types";
import type { SeriesPanelProps } from "./shared";

/** Name, language pair and memory of a series, plus its deletion. */
export function SeriesProfilePanel({
  detail,
  busy,
  run,
  onChanged,
  onDeleted,
}: SeriesPanelProps & {
  detail: SeriesDetail;
  /** Reloads the list and the detail after a save. */
  onChanged: () => Promise<void>;
  onDeleted: () => Promise<void>;
}) {
  const memory = (key: string): string =>
    detail.memory.find((row) => row.key === key)?.value ?? "";
  const [name, setName] = useState(detail.series.name);
  const [sourceLang, setSourceLang] = useState(detail.series.source_lang ?? "");
  const [targetLang, setTargetLang] = useState(detail.series.target_lang ?? "");
  const [styleGuide, setStyleGuide] = useState(memory("style_guide"));
  const [synopsis, setSynopsis] = useState(memory("synopsis"));

  // A reload (after a save, a confirmed candidate, an import) shows the stored values.
  useEffect(() => {
    const value = (key: string) => detail.memory.find((row) => row.key === key)?.value ?? "";
    setName(detail.series.name);
    setSourceLang(detail.series.source_lang ?? "");
    setTargetLang(detail.series.target_lang ?? "");
    setStyleGuide(value("style_guide"));
    setSynopsis(value("synopsis"));
  }, [detail]);

  async function handleSave(): Promise<void> {
    await run("save", async () => {
      await seriesUpdate({
        id: detail.series.id,
        name: name.trim(),
        source_lang: sourceLang.trim().length > 0 ? sourceLang.trim() : null,
        target_lang: targetLang.trim().length > 0 ? targetLang.trim() : null,
        style_guide: styleGuide,
        synopsis,
      });
      await onChanged();
      return "Anagrafica e memoria di serie salvate.";
    });
  }

  async function handleDelete(): Promise<void> {
    if (
      !window.confirm(`Eliminare la serie «${detail.series.name}»? I libri restano, il canone no.`)
    ) {
      return;
    }
    await run("delete", async () => {
      await seriesDelete(detail.series.id);
      await onDeleted();
      return "Serie eliminata: i libri sono stati scollegati.";
    });
  }

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Anagrafica e memoria</span>
        <button
          type="button"
          className="btn btn-sm btn-danger"
          disabled={busy}
          onClick={() => void handleDelete()}
        >
          Elimina serie
        </button>
      </div>
      <div className="panel-pad section-stack">
        <div className="grid grid-cols-1 gap-2 md:grid-cols-3">
          <FormField label="Nome" htmlFor="series-edit-name" required>
            <input
              id="series-edit-name"
              className="input"
              value={name}
              onChange={(event) => {
                setName(event.target.value);
              }}
            />
          </FormField>
          <FormField label="Lingua di partenza" htmlFor="series-edit-source">
            <input
              id="series-edit-source"
              className="input"
              value={sourceLang}
              onChange={(event) => {
                setSourceLang(event.target.value);
              }}
            />
          </FormField>
          <FormField label="Lingua di arrivo" htmlFor="series-edit-target">
            <input
              id="series-edit-target"
              className="input"
              value={targetLang}
              onChange={(event) => {
                setTargetLang(event.target.value);
              }}
            />
          </FormField>
        </div>
        <FormField
          label="Style guide di serie"
          htmlFor="series-style"
          hint="Accodata a quella del libro: il testo del libro viene prima nel budget."
        >
          <textarea
            id="series-style"
            className="input min-h-20"
            value={styleGuide}
            onChange={(event) => {
              setStyleGuide(event.target.value);
            }}
          />
        </FormField>
        <FormField
          label="Sinossi di serie"
          htmlFor="series-synopsis"
          hint="Contesto per tutti i libri; quella del singolo libro resta prioritaria."
        >
          <textarea
            id="series-synopsis"
            className="input min-h-20"
            value={synopsis}
            onChange={(event) => {
              setSynopsis(event.target.value);
            }}
          />
        </FormField>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="btn btn-primary"
            disabled={busy || name.trim().length === 0}
            onClick={() => void handleSave()}
          >
            Salva
          </button>
          <span className="field-hint">
            revisione {memory("style_guide").length > 0 ? "presente" : "assente"}
          </span>
        </div>
      </div>
    </div>
  );
}
