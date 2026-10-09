import { useMemo, useState } from "react";
import { FormField } from "../../components/FormField";
import { StatusBadge } from "../../components/StatusBadge";
import {
  seriesGlossaryDelete,
  seriesGlossaryUpsert,
  seriesVariantDelete,
  seriesVariantUpsert,
} from "../../lib/ipc";
import type { SeriesDetail, SeriesGlossaryTerm } from "../../lib/types";
import {
  draftFrom,
  KINDS,
  kindLabel,
  statusLabel,
  STATUSES,
  type SeriesPanelProps,
  type TermDraft,
} from "./shared";

/** The series canon: add a term, edit one with its aliases, delete it. */
export function SeriesGlossaryPanel({
  detail,
  busy,
  run,
  onChanged,
}: SeriesPanelProps & { detail: SeriesDetail; onChanged: () => Promise<void> }) {
  const [editingTermId, setEditingTermId] = useState<string | null>(null);
  const [draft, setDraft] = useState<TermDraft | null>(null);
  const [newVariant, setNewVariant] = useState("");
  const [termSource, setTermSource] = useState("");
  const [termTarget, setTermTarget] = useState("");
  const [termKind, setTermKind] = useState("term");

  const variantsByTerm = useMemo(() => {
    const map = new Map<string, ReadonlyArray<{ id: string; text: string }>>();
    for (const variant of detail.variants) {
      const list = map.get(variant.term_id) ?? [];
      map.set(variant.term_id, [...list, { id: variant.id, text: variant.text }]);
    }
    return map;
  }, [detail]);

  async function handleAddTerm(): Promise<void> {
    await run("add-term", async () => {
      await seriesGlossaryUpsert({
        series_id: detail.series.id,
        source: termSource.trim(),
        target: termTarget.trim(),
        kind: termKind,
        status: "approved",
      });
      setTermSource("");
      setTermTarget("");
      setTermKind("term");
      await onChanged();
      return "Termine di serie aggiunto: i libri che lo rendono diversamente sono segnalati.";
    });
  }

  function startEdit(term: SeriesGlossaryTerm): void {
    setEditingTermId(term.id);
    setDraft(draftFrom(term));
    setNewVariant("");
  }

  async function handleSaveTerm(term: SeriesGlossaryTerm): Promise<void> {
    if (draft === null) {
      return;
    }
    await run(`term:${term.id}`, async () => {
      await seriesGlossaryUpsert({
        id: term.id,
        series_id: term.series_id,
        source: term.source,
        target: draft.target,
        kind: draft.kind,
        status: draft.status,
        note: draft.note,
        expected_revision: term.revision,
      });
      setEditingTermId(null);
      setDraft(null);
      await onChanged();
      return "Termine aggiornato.";
    });
  }

  async function handleDeleteTerm(term: SeriesGlossaryTerm): Promise<void> {
    await run(`delete-term:${term.id}`, async () => {
      await seriesGlossaryDelete(term.id);
      setEditingTermId(null);
      setDraft(null);
      await onChanged();
      return "Termine di serie eliminato.";
    });
  }

  async function handleAddVariant(term: SeriesGlossaryTerm): Promise<void> {
    const text = newVariant.trim();
    if (text.length === 0) {
      return;
    }
    await run(`variant:${term.id}`, async () => {
      await seriesVariantUpsert({ term_id: term.id, text });
      setNewVariant("");
      await onChanged();
      return null;
    });
  }

  async function handleDeleteVariant(id: string): Promise<void> {
    await run(`variant-del:${id}`, async () => {
      await seriesVariantDelete(id);
      await onChanged();
      return null;
    });
  }

  return (
    <div className="panel">
          <div className="panel-head">
            <span className="panel-title">Glossario di serie</span>
            <span className="mono-chip">{detail.glossary.length}</span>
          </div>
          <div className="panel-pad section-stack">
            <div className="grid grid-cols-1 gap-2 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_10rem_auto]">
              <input
                className="input"
                aria-label="Termine di partenza"
                placeholder="the Keeper"
                value={termSource}
                onChange={(event) => {
                  setTermSource(event.target.value);
                }}
              />
              <input
                className="input"
                aria-label="Rendering di serie"
                placeholder="il Custode"
                value={termTarget}
                onChange={(event) => {
                  setTermTarget(event.target.value);
                }}
              />
              <select
                className="select"
                aria-label="Tipo di termine"
                value={termKind}
                onChange={(event) => {
                  setTermKind(event.target.value);
                }}
              >
                {KINDS.map((entry) => (
                  <option key={entry.value} value={entry.value}>
                    {entry.label}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className="btn btn-primary"
                disabled={
                  busy ||
                  termSource.trim().length === 0 ||
                  (termTarget.trim().length === 0 && termKind !== "do_not_translate")
                }
                onClick={() => void handleAddTerm()}
              >
                Aggiungi
              </button>
            </div>
    
            {detail.glossary.length === 0 ? (
              <p className="field-hint">Nessun termine di serie.</p>
            ) : (
              <ul className="section-stack">
                {detail.glossary.map((term) => {
                  const editing = editingTermId === term.id && draft !== null;
                  const variants = variantsByTerm.get(term.id) ?? [];
                  return (
                    <li key={term.id} className="rounded border border-line p-2">
                      {editing ? (
                        <div className="section-stack">
                          <div className="grid grid-cols-1 gap-2 md:grid-cols-3">
                            <FormField label="Rendering" htmlFor={`term-target-${term.id}`}>
                              <input
                                id={`term-target-${term.id}`}
                                className="input"
                                value={draft.target}
                                onChange={(event) => {
                                  setDraft({ ...draft, target: event.target.value });
                                }}
                              />
                            </FormField>
                            <FormField label="Tipo" htmlFor={`term-kind-${term.id}`}>
                              <select
                                id={`term-kind-${term.id}`}
                                className="select"
                                value={draft.kind}
                                onChange={(event) => {
                                  setDraft({ ...draft, kind: event.target.value });
                                }}
                              >
                                {KINDS.map((entry) => (
                                  <option key={entry.value} value={entry.value}>
                                    {entry.label}
                                  </option>
                                ))}
                              </select>
                            </FormField>
                            <FormField label="Stato" htmlFor={`term-status-${term.id}`}>
                              <select
                                id={`term-status-${term.id}`}
                                className="select"
                                value={draft.status}
                                onChange={(event) => {
                                  setDraft({ ...draft, status: event.target.value });
                                }}
                              >
                                {STATUSES.map((entry) => (
                                  <option key={entry.value} value={entry.value}>
                                    {entry.label}
                                  </option>
                                ))}
                              </select>
                            </FormField>
                          </div>
                          <FormField label="Nota" htmlFor={`term-note-${term.id}`}>
                            <input
                              id={`term-note-${term.id}`}
                              className="input"
                              value={draft.note}
                              onChange={(event) => {
                                setDraft({ ...draft, note: event.target.value });
                              }}
                            />
                          </FormField>
                          <div className="flex flex-wrap items-center gap-2">
                            <button
                              type="button"
                              className="btn btn-sm btn-primary"
                              disabled={busy || draft.target.trim().length === 0}
                              onClick={() => void handleSaveTerm(term)}
                            >
                              Salva
                            </button>
                            <button
                              type="button"
                              className="btn btn-sm"
                              onClick={() => {
                                setEditingTermId(null);
                                setDraft(null);
                              }}
                            >
                              Annulla
                            </button>
                            <button
                              type="button"
                              className="btn btn-sm btn-danger ml-auto"
                              disabled={busy}
                              onClick={() => void handleDeleteTerm(term)}
                            >
                              Elimina termine
                            </button>
                          </div>
                        </div>
                      ) : (
                        <div className="flex flex-wrap items-center justify-between gap-2">
                          <span className="min-w-0 text-xs">
                            <span className="font-medium text-ink">{term.source}</span> →{" "}
                            {term.target}{" "}
                            <span className="badge badge-neutral">{kindLabel(term.kind)}</span>{" "}
                            <StatusBadge status={term.status} />
                            <span className="ml-1 text-[0.68rem] text-faint">
                              {statusLabel(term.status)} · revisione {term.revision} ·{" "}
                              {term.origin}
                            </span>
                          </span>
                          <button
                            type="button"
                            className="btn btn-sm"
                            onClick={() => {
                              startEdit(term);
                            }}
                          >
                            Modifica
                          </button>
                        </div>
                      )}
    
                      <div className="mt-2 border-t border-line pt-2">
                        <span className="stat-label">Alias</span>
                        <span className="ml-2 inline-flex flex-wrap items-center gap-1 align-middle">
                          {variants.map((variant) => (
                            <span key={variant.id} className="mono-chip">
                              {variant.text}
                              <button
                                type="button"
                                className="ml-1 text-danger"
                                aria-label={`Rimuovi alias ${variant.text}`}
                                disabled={busy}
                                onClick={() => void handleDeleteVariant(variant.id)}
                              >
                                ×
                              </button>
                            </span>
                          ))}
                          {editing ? (
                            <span className="inline-flex items-center gap-1">
                              <input
                                className="input"
                                style={{ width: "12rem" }}
                                aria-label="Nuovo alias"
                                placeholder="the Keeper"
                                value={newVariant}
                                onChange={(event) => {
                                  setNewVariant(event.target.value);
                                }}
                              />
                              <button
                                type="button"
                                className="btn btn-sm"
                                disabled={busy || newVariant.trim().length === 0}
                                onClick={() => void handleAddVariant(term)}
                              >
                                Aggiungi alias
                              </button>
                            </span>
                          ) : variants.length === 0 ? (
                            <span className="text-[0.68rem] text-faint">
                              nessuno — modifica il termine per aggiungerne
                            </span>
                          ) : null}
                        </span>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        </div>
  );
}
