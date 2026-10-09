import { useCallback, useEffect, useMemo, useState } from "react";
import { BookProfilePanel } from "../components/BookProfilePanel";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { StatusBadge, statusLabel } from "../components/StatusBadge";
import { onJobProgress } from "../lib/events";
import { glossaryDelete, glossaryList, glossaryUpsert, toErrorMessage } from "../lib/ipc";
import {
  GLOSSARY_KINDS,
  GLOSSARY_STATUS_ORDER,
  glossaryCounts,
  glossaryMatches,
  glossaryOrder,
  needsTarget,
  pendingDecisionCount,
} from "../lib/glossary";
import type { GlossaryTerm, Project } from "../lib/types";
import type { ViewId } from "../App";

/**
 * Project glossary (PLAN.md §9.2): the translation canon of one book, as its own destination.
 *
 * The same table lives inside the "Profilo del libro" panel on the translation page, under the
 * profile fields — a candidate proposed by the reconnaissance or the summarizer is easy to miss
 * there. This view is where the terms are reviewed: the ones waiting for a decision come first,
 * the filters narrow them, and the header counters agree with the rows on screen.
 *
 * Edits stay local until "Salva", which writes every changed row with the revision it was loaded
 * from, so a summarizer write in the meantime surfaces as a stale-row error instead of being
 * overwritten. Filtering and ordering read the persisted row, never the draft: nothing moves
 * under the cursor while a term is being edited. The translator prompt only sees terms that are
 * not rejected.
 */

export interface GlossaryViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

interface RowDraft {
  id: string | null;
  source: string;
  target: string;
  kind: string;
  status: string;
  note: string;
  /** Revision the row was loaded from; `0` for a term that does not exist yet. */
  revision: number;
}

function draftOf(term: GlossaryTerm): RowDraft {
  return {
    id: term.id,
    source: term.source,
    target: term.target,
    kind: term.kind,
    status: term.status,
    note: term.note ?? "",
    revision: term.revision,
  };
}

function newDraft(): RowDraft {
  return {
    id: null,
    source: "",
    target: "",
    kind: "term",
    status: "candidate",
    note: "",
    revision: 0,
  };
}

/** A row is written only when it carries something: an edit, or a new term with a source. */
function isDirty(draft: RowDraft, terms: readonly GlossaryTerm[]): boolean {
  if (draft.id === null) {
    return draft.source.trim().length > 0;
  }
  const original = terms.find((term) => term.id === draft.id);
  if (original === undefined) {
    return false;
  }
  return (
    draft.target !== original.target ||
    draft.kind !== original.kind ||
    draft.status !== original.status ||
    draft.note !== (original.note ?? "")
  );
}

export function GlossaryView({ project, onNavigate }: GlossaryViewProps) {
  const [terms, setTerms] = useState<GlossaryTerm[]>([]);
  const [drafts, setDrafts] = useState<RowDraft[]>([]);
  const [deleted, setDeleted] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState("");
  const [query, setQuery] = useState("");
  // Bumped on every job transition so the book profile refetches when a reconnaissance ends.
  const [profileToken, setProfileToken] = useState(0);

  useEffect(
    () =>
      onJobProgress(() => {
        setProfileToken((current) => current + 1);
      }),
    [],
  );

  const projectId = project?.id ?? null;
  const load = useCallback(async () => {
    if (projectId === null) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const loaded = await glossaryList(projectId);
      setTerms(loaded);
      setDrafts(glossaryOrder(loaded.map(draftOf)));
      setDeleted([]);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    void load();
  }, [load]);

  const counts = useMemo(() => glossaryCounts(terms), [terms]);
  const pending = useMemo(() => pendingDecisionCount(terms), [terms]);

  /**
   * The rows on screen: the persisted status and source decide, so approving a candidate does not
   * move it while the cursor is still in the row. Everything unsaved stays visible, or a new term
   * would vanish the moment the filters do not describe it yet.
   */
  const visible = useMemo(() => {
    const persisted = new Map(terms.map((term) => [term.id, term]));
    return drafts
      .map((draft, index) => ({
        draft,
        index,
        original: draft.id === null ? null : persisted.get(draft.id) ?? null,
      }))
      .filter(
        ({ draft, original }) =>
          draft.id === null ||
          isDirty(draft, terms) ||
          glossaryMatches(original ?? draft, statusFilter, query),
      );
  }, [drafts, terms, statusFilter, query]);

  const dirtyDrafts = useMemo(
    () => drafts.filter((draft) => isDirty(draft, terms)),
    [drafts, terms],
  );
  const changeCount = dirtyDrafts.length + deleted.length;

  function updateDraft(index: number, patch: Partial<RowDraft>) {
    setDrafts((current) =>
      current.map((draft, position) => (position === index ? { ...draft, ...patch } : draft)),
    );
  }

  function removeDraft(index: number) {
    setDrafts((current) => {
      const removed = current[index];
      if (removed !== undefined && removed.id !== null) {
        setDeleted((ids) => [...ids, removed.id as string]);
      }
      return current.filter((_, position) => position !== index);
    });
  }

  async function save() {
    if (projectId === null) {
      return;
    }
    const incomplete = dirtyDrafts.find(
      (draft) =>
        draft.source.trim().length === 0 ||
        (needsTarget(draft.kind) && draft.target.trim().length === 0),
    );
    if (incomplete !== undefined) {
      setActionError(
        "Ogni termine richiede un testo di partenza e una traduzione (oppure il tipo «Non tradurre»).",
      );
      return;
    }

    setSaving(true);
    setActionError(null);
    setNotice(null);
    try {
      for (const id of deleted) {
        await glossaryDelete(id);
      }
      for (const draft of dirtyDrafts) {
        await glossaryUpsert({
          id: draft.id,
          project_id: projectId,
          source: draft.source.trim(),
          target: draft.target.trim(),
          kind: draft.kind,
          note: draft.note,
          status: draft.status,
          expected_revision: draft.id === null ? null : draft.revision,
        });
      }
      await load();
      setNotice(
        dirtyDrafts.length === 0
          ? "Termini rimossi."
          : "Glossario aggiornato: il prossimo chunk tradotto usa già questi termini.",
      );
    } catch (saveError) {
      setActionError(toErrorMessage(saveError));
      // Reload so the table holds the revisions the next write has to start from.
      await load();
    } finally {
      setSaving(false);
    }
  }

  if (project === null) {
    return (
      <div className="section-stack">
        <h1 className="font-serif text-2xl font-medium text-ink">Prepara</h1>
        <EmptyState
          title="Nessun libro aperto"
          description="Il glossario appartiene a un progetto: aprine uno per rivedere i termini che il traduttore riceve."
          actionLabel="Vai alla libreria"
          onAction={() => {
            onNavigate("projects");
          }}
        />
      </div>
    );
  }

  return (
    <div className="section-stack">
      <BookProfilePanel projectId={project.id} reloadToken={profileToken} initiallyExpanded />

      <section className="panel">
        <div className="panel-head">
          <span className="panel-title">Glossario del progetto</span>
          <span className="flex flex-wrap items-center gap-2">
            <span className="mono-chip">
              {project.source_lang ?? "?"} → {project.target_lang}
            </span>
            <span className="mono-chip">{terms.length} termini</span>
            {pending > 0 ? (
              <StatusBadge status="candidate" label={`${pending} da decidere`} tone="warning" />
            ) : null}
          </span>
        </div>

        <div className="panel-pad section-stack">
          <p className="text-xs text-muted">
            Il prompt del traduttore riceve solo i termini approvati. I candidati arrivano dalla
            ricognizione e dai riassunti e aspettano te: approva, correggi o rifiuta. Un conflitto segnala una resa
            diversa da quella del canone di serie.
          </p>

          {actionError !== null ? (
            <div className="banner banner-error" role="alert">
              <span aria-hidden="true">⚠</span>
              <span>{actionError}</span>
            </div>
          ) : null}
          {notice !== null ? (
            <div className="banner banner-ok" role="status">
              <span aria-hidden="true">✓</span>
              <span>{notice}</span>
            </div>
          ) : null}

          <div className="grid grid-cols-1 items-end gap-2 md:grid-cols-[12rem_minmax(0,1fr)_auto]">
            <FormField label="Stato" htmlFor="glossary-status-filter">
              <select
                id="glossary-status-filter"
                className="select"
                value={statusFilter}
                onChange={(event) => {
                  setStatusFilter(event.target.value);
                }}
              >
                <option value="">Tutti gli stati</option>
                {GLOSSARY_STATUS_ORDER.map((status) => (
                  <option key={status} value={status}>
                    {statusLabel(status)} ({counts[status] ?? 0})
                  </option>
                ))}
              </select>
            </FormField>

            <FormField label="Cerca" htmlFor="glossary-query">
              <input
                id="glossary-query"
                className="input"
                placeholder="Sorgente, traduzione o nota"
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                }}
              />
            </FormField>

            <div className="flex items-center gap-2">
              <button
                type="button"
                className="btn"
                onClick={() => {
                  setDrafts((current) => [newDraft(), ...current]);
                  setNotice(null);
                }}
              >
                Aggiungi termine
              </button>
              <button
                type="button"
                className="btn btn-primary"
                disabled={saving || changeCount === 0}
                onClick={() => void save()}
                title={
                  changeCount === 0
                    ? "Nessuna modifica da salvare"
                    : "Scrive le righe modificate con la revisione caricata"
                }
              >
                {saving ? <span className="spinner" aria-hidden="true" /> : null}
                Salva{changeCount > 0 ? ` (${changeCount})` : ""}
              </button>
              <button
                type="button"
                className="btn"
                disabled={loading}
                onClick={() => void load()}
                title="Rilegge il glossario dal database"
              >
                Ricarica
              </button>
            </div>
          </div>

          {error !== null ? (
            <EmptyState
              tone="error"
              compact
              title="Glossario non leggibile"
              description="Il progetto risponde ma i termini non arrivano: riprova o controlla i log."
              details={error}
              actionLabel="Riprova"
              onAction={() => void load()}
            />
          ) : loading ? (
            <EmptyState tone="loading" compact title="Caricamento del glossario…" />
          ) : visible.length === 0 ? (
            <p className="field-hint">
              {terms.length === 0
                ? "Nessun termine: aggiungine uno oppure lancia la ricognizione dalla pagina Traduzione."
                : "Nessun termine corrisponde al filtro."}
            </p>
          ) : (
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
                  {visible.map(({ draft, index }) => (
                    <tr key={draft.id ?? `new-${index}`}>
                      <td>
                        {draft.id === null ? (
                          <input
                            className="input"
                            aria-label="Termine di partenza"
                            placeholder="harbour"
                            value={draft.source}
                            onChange={(event) => {
                              updateDraft(index, { source: event.target.value });
                            }}
                          />
                        ) : (
                          <span
                            className="mono-chip"
                            title="Il termine identifica la riga: per rinominarlo aggiungine uno nuovo"
                          >
                            {draft.source}
                          </span>
                        )}
                      </td>
                      <td>
                        <input
                          className="input"
                          aria-label={`Traduzione di ${draft.source}`}
                          placeholder={needsTarget(draft.kind) ? "porto" : "lasciato invariato"}
                          value={draft.target}
                          onChange={(event) => {
                            updateDraft(index, { target: event.target.value });
                          }}
                        />
                      </td>
                      <td>
                        <select
                          className="select"
                          aria-label={`Tipo di ${draft.source}`}
                          value={draft.kind}
                          onChange={(event) => {
                            updateDraft(index, { kind: event.target.value });
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
                          aria-label={`Stato di ${draft.source}`}
                          value={draft.status}
                          onChange={(event) => {
                            updateDraft(index, { status: event.target.value });
                          }}
                        >
                          {GLOSSARY_STATUS_ORDER.map((status) => (
                            <option key={status} value={status}>
                              {statusLabel(status)}
                            </option>
                          ))}
                        </select>
                      </td>
                      <td>
                        <input
                          className="input"
                          aria-label={`Nota su ${draft.source}`}
                          value={draft.note}
                          onChange={(event) => {
                            updateDraft(index, { note: event.target.value });
                          }}
                        />
                      </td>
                      <td>
                        <button
                          type="button"
                          className="btn btn-sm btn-ghost"
                          title={
                            draft.id === null
                              ? "Togli la riga non salvata"
                              : "Rimuovi il termine (al salvataggio)"
                          }
                          onClick={() => {
                            removeDraft(index);
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
          )}

          <p className="field-hint">
            «Salva» scrive le righe modificate con la revisione da cui sei partito: se nel frattempo
            il glossario è cambiato, la scrittura viene segnalata invece di sovrascrivere.
          </p>
        </div>
      </section>
    </div>
  );
}
