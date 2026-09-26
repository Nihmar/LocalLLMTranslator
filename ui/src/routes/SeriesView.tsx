import { useCallback, useEffect, useMemo, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { StatusBadge } from "../components/StatusBadge";
import {
  glossaryList,
  openPath,
  projectList,
  projectSetSeries,
  qaReport,
  seriesCreate,
  seriesDelete,
  seriesGet,
  seriesGlossaryDelete,
  seriesGlossaryUpsert,
  seriesList,
  seriesPromoteTerm,
  seriesUpdate,
  seriesVariantDelete,
  seriesVariantUpsert,
  toErrorMessage,
} from "../lib/ipc";
import type {
  GlossaryTerm,
  Project,
  QaFinding,
  Series,
  SeriesDetail,
  SeriesGlossaryTerm,
} from "../lib/types";

/**
 * Series: the shared canon across books (`PLAN.md` §9.5).
 *
 * A series owns the pinned language pair, the glossary its books inherit (a book term
 * overrides the series rendering for the same source) and memory values. The engine
 * already resolves the effective glossary; this view is where the user authors the canon,
 * attaches books, promotes a book term and resolves the conflicts a canon change opens.
 */

const KINDS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "term", label: "Termine" },
  { value: "proper_noun", label: "Nome proprio" },
  { value: "do_not_translate", label: "Non tradurre" },
];

const STATUSES: ReadonlyArray<{ value: string; label: string }> = [
  { value: "approved", label: "Approvato" },
  { value: "candidate", label: "Candidato" },
  { value: "conflict", label: "Conflitto" },
  { value: "rejected", label: "Rifiutato" },
];

function statusLabel(status: string): string {
  return STATUSES.find((entry) => entry.value === status)?.label ?? status;
}

function kindLabel(kind: string): string {
  return KINDS.find((entry) => entry.value === kind)?.label ?? kind;
}

interface TermDraft {
  target: string;
  kind: string;
  status: string;
  note: string;
}

function draftFrom(term: SeriesGlossaryTerm): TermDraft {
  return {
    target: term.target,
    kind: term.kind,
    status: term.status,
    note: term.note ?? "",
  };
}

export function SeriesView() {
  const [series, setSeries] = useState<Series[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SeriesDetail | null>(null);
  const [conflicts, setConflicts] = useState<ReadonlyArray<{ project: Project; finding: QaFinding }>>(
    [],
  );
  const [promoteProjectId, setPromoteProjectId] = useState<string>("");
  const [promoteTerms, setPromoteTerms] = useState<GlossaryTerm[]>([]);
  const [editingTermId, setEditingTermId] = useState<string | null>(null);
  const [draft, setDraft] = useState<TermDraft | null>(null);
  const [newVariant, setNewVariant] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  // Create form.
  const [newName, setNewName] = useState("");
  const [newSourceLang, setNewSourceLang] = useState("");
  const [newTargetLang, setNewTargetLang] = useState("");
  // Anagrafica + memory form.
  const [name, setName] = useState("");
  const [sourceLang, setSourceLang] = useState("");
  const [targetLang, setTargetLang] = useState("");
  const [styleGuide, setStyleGuide] = useState("");
  const [synopsis, setSynopsis] = useState("");
  // New term form.
  const [termSource, setTermSource] = useState("");
  const [termTarget, setTermTarget] = useState("");
  const [termKind, setTermKind] = useState("term");

  const selected = useMemo(
    () => series.find((entry) => entry.id === selectedId) ?? null,
    [series, selectedId],
  );

  const memoryValue = useCallback(
    (key: string): string => detail?.memory.find((row) => row.key === key)?.value ?? "",
    [detail],
  );

  const loadSeries = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [seriesRows, projectRows] = await Promise.all([seriesList(), projectList()]);
      setSeries(seriesRows);
      setProjects(projectRows);
      setSelectedId((current) =>
        current !== null && seriesRows.some((row) => row.id === current)
          ? current
          : (seriesRows[0]?.id ?? null),
      );
    } catch (loadError) {
      setError(toErrorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }, []);

  const loadDetail = useCallback(async () => {
    if (selectedId === null) {
      setDetail(null);
      setConflicts([]);
      return;
    }
    try {
      const loaded = await seriesGet(selectedId);
      setDetail(loaded);
      setName(loaded.series.name);
      setSourceLang(loaded.series.source_lang ?? "");
      setTargetLang(loaded.series.target_lang ?? "");
      setStyleGuide(loaded.memory.find((row) => row.key === "style_guide")?.value ?? "");
      setSynopsis(loaded.memory.find((row) => row.key === "synopsis")?.value ?? "");
      setEditingTermId(null);
      setDraft(null);

      // Conflicts are project-scoped findings: gather the open ones from every book.
      const perProject = await Promise.all(
        loaded.projects.map(async (project) => {
          const findings = await qaReport({ project_id: project.id, kind: "glossary_conflict" });
          return findings
            .filter((finding) => finding.status === "open")
            .map((finding) => ({ project, finding }));
        }),
      );
      setConflicts(perProject.flat());
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setDetail(null);
    }
  }, [selectedId]);

  useEffect(() => {
    void loadSeries();
  }, [loadSeries]);

  useEffect(() => {
    void loadDetail();
  }, [loadDetail]);

  useEffect(() => {
    if (promoteProjectId === "") {
      setPromoteTerms([]);
      return;
    }
    let cancelled = false;
    void glossaryList(promoteProjectId)
      .then((terms) => {
        if (!cancelled) {
          setPromoteTerms(terms.filter((term) => term.status !== "rejected"));
        }
      })
      .catch((loadError: unknown) => {
        if (!cancelled) {
          setError(toErrorMessage(loadError));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [promoteProjectId]);

  async function run(action: string, work: () => Promise<void>): Promise<void> {
    setBusy(action);
    setError(null);
    setNotice(null);
    try {
      await work();
    } catch (actionError) {
      setError(toErrorMessage(actionError));
    } finally {
      setBusy(null);
    }
  }

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
      await loadSeries();
      setSelectedId(created.id);
      setNotice(`Serie «${created.name}» creata.`);
    });
  }

  async function handleSaveSeries(): Promise<void> {
    if (selected === null) {
      return;
    }
    await run("save", async () => {
      await seriesUpdate({
        id: selected.id,
        name: name.trim(),
        source_lang: sourceLang.trim().length > 0 ? sourceLang.trim() : null,
        target_lang: targetLang.trim().length > 0 ? targetLang.trim() : null,
        style_guide: styleGuide,
        synopsis,
      });
      await loadSeries();
      await loadDetail();
      setNotice("Anagrafica e memoria di serie salvate.");
    });
  }

  async function handleDelete(): Promise<void> {
    if (selected === null) {
      return;
    }
    if (!window.confirm(`Eliminare la serie «${selected.name}»? I libri restano, il canone no.`)) {
      return;
    }
    await run("delete", async () => {
      await seriesDelete(selected.id);
      setSelectedId(null);
      await loadSeries();
      setNotice("Serie eliminata: i libri sono stati scollegati.");
    });
  }

  async function handleAttach(projectId: string): Promise<void> {
    if (selected === null || projectId === "") {
      return;
    }
    await run(`attach:${projectId}`, async () => {
      const members = detail?.projects ?? [];
      await projectSetSeries({
        project_id: projectId,
        series_id: selected.id,
        series_order: members.length + 1,
      });
      await loadSeries();
      await loadDetail();
      setNotice("Libro collegato alla serie.");
    });
  }

  async function handleDetach(projectId: string): Promise<void> {
    await run(`detach:${projectId}`, async () => {
      await projectSetSeries({ project_id: projectId, series_id: null, series_order: null });
      await loadSeries();
      await loadDetail();
      setNotice("Libro scollegato: userà solo il proprio glossario.");
    });
  }

  async function handleAddTerm(): Promise<void> {
    if (selected === null) {
      return;
    }
    await run("add-term", async () => {
      await seriesGlossaryUpsert({
        series_id: selected.id,
        source: termSource.trim(),
        target: termTarget.trim(),
        kind: termKind,
        status: "approved",
      });
      setTermSource("");
      setTermTarget("");
      setTermKind("term");
      await loadDetail();
      setNotice("Termine di serie aggiunto: i libri che lo rendono diversamente sono segnalati.");
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
      await loadDetail();
      setNotice("Termine aggiornato.");
    });
  }

  async function handleDeleteTerm(term: SeriesGlossaryTerm): Promise<void> {
    await run(`delete-term:${term.id}`, async () => {
      await seriesGlossaryDelete(term.id);
      setEditingTermId(null);
      setDraft(null);
      await loadDetail();
      setNotice("Termine di serie eliminato.");
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
      await loadDetail();
    });
  }

  async function handleDeleteVariant(id: string): Promise<void> {
    await run(`variant-del:${id}`, async () => {
      await seriesVariantDelete(id);
      await loadDetail();
    });
  }

  async function handlePromote(term: GlossaryTerm): Promise<void> {
    if (promoteProjectId === "") {
      return;
    }
    await run(`promote:${term.id}`, async () => {
      const result = await seriesPromoteTerm({
        project_id: promoteProjectId,
        term_id: term.id,
      });
      await loadDetail();
      setNotice(
        result.outcome === "added"
          ? `«${term.source}» promosso nel canone di serie.`
          : result.outcome === "conflict"
            ? `«${term.source}»: il canone ha già un rendering diverso; il conflitto è stato registrato.`
            : `«${term.source}» era già nel canone con lo stesso rendering.`,
      );
    });
  }

  const freeProjects = projects.filter(
    (project) => !(detail?.projects ?? []).some((member) => member.id === project.id),
  );
  const variantsByTerm = useMemo(() => {
    const map = new Map<string, ReadonlyArray<{ id: string; text: string }>>();
    for (const variant of detail?.variants ?? []) {
      const list = map.get(variant.term_id) ?? [];
      map.set(variant.term_id, [...list, { id: variant.id, text: variant.text }]);
    }
    return map;
  }, [detail]);

  return (
    <div className="section-stack">
      <div>
        <h2 className="text-lg font-semibold text-ink">Serie</h2>
        <p className="mt-0.5 text-xs text-muted">
          Il canone condiviso: glossario e memoria che i libri della saga ereditano. Un termine
          del libro vince su quello di serie; una modifica al canone segnala i libri incoerenti.
        </p>
      </div>

      {error !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{error}</span>
        </div>
      ) : null}
      {notice !== null ? (
        <div className="banner banner-ok" role="status">
          <span aria-hidden="true">✓</span>
          <span>{notice}</span>
        </div>
      ) : null}

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[20rem_minmax(0,1fr)]">
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
                        setSelectedId(entry.id);
                      }}
                    >
                      <span className="min-w-0">
                        <span className="block truncate text-[0.82rem] font-medium">
                          {entry.name}
                        </span>
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
                disabled={busy !== null || newName.trim().length === 0}
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

        <div className="section-stack min-w-0">
          {selected === null ? (
            <div className="panel">
              <div className="panel-pad">
                <EmptyState
                  title="Nessuna serie selezionata"
                  description="Crea una serie per condividere glossario e memoria tra i libri di una saga."
                />
              </div>
            </div>
          ) : detail === null ? (
            <div className="panel">
              <div className="panel-pad">
                <EmptyState tone="loading" compact title="Lettura della serie…" />
              </div>
            </div>
          ) : (
            <>
              <div className="panel">
                <div className="panel-head">
                  <span className="panel-title">Anagrafica e memoria</span>
                  <button
                    type="button"
                    className="btn btn-sm btn-danger"
                    disabled={busy !== null}
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
                      disabled={busy !== null || name.trim().length === 0}
                      onClick={() => void handleSaveSeries()}
                    >
                      Salva
                    </button>
                    <span className="field-hint">
                      revisione {memoryValue("style_guide").length > 0 ? "presente" : "assente"}
                    </span>
                  </div>
                </div>
              </div>

              <div className="panel">
                <div className="panel-head">
                  <span className="panel-title">Libri della serie</span>
                  <span className="mono-chip">{detail.projects.length}</span>
                </div>
                <div className="panel-pad section-stack">
                  {detail.projects.length === 0 ? (
                    <p className="field-hint">Nessun libro collegato.</p>
                  ) : (
                    <ul className="section-stack">
                      {detail.projects.map((member) => (
                        <li
                          key={member.id}
                          className="flex flex-wrap items-center justify-between gap-2 rounded border border-line p-2"
                        >
                          <span className="min-w-0">
                            <span className="block truncate text-xs font-medium text-ink">
                              {member.series_order ?? "—"}. {member.name}
                            </span>
                            <span className="block truncate text-[0.68rem] text-faint">
                              {member.source_lang ?? "?"} → {member.target_lang}
                            </span>
                          </span>
                          <span className="flex items-center gap-1">
                            <button
                              type="button"
                              className="btn btn-sm btn-ghost"
                              onClick={() => void openPath(member.source_path)}
                            >
                              Apri sorgente
                            </button>
                            <button
                              type="button"
                              className="btn btn-sm"
                              disabled={busy !== null}
                              onClick={() => void handleDetach(member.id)}
                            >
                              Scollega
                            </button>
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}

                  {freeProjects.length > 0 ? (
                    <FormField label="Collega un libro" htmlFor="series-attach">
                      <select
                        id="series-attach"
                        className="select"
                        value=""
                        disabled={busy !== null}
                        onChange={(event) => {
                          void handleAttach(event.target.value);
                        }}
                      >
                        <option value="">Scegli un progetto…</option>
                        {freeProjects.map((project) => (
                          <option key={project.id} value={project.id}>
                            {project.name} ({project.source_lang ?? "?"} → {project.target_lang})
                          </option>
                        ))}
                      </select>
                    </FormField>
                  ) : null}
                </div>
              </div>

              <div className="panel">
                <div className="panel-head">
                  <span className="panel-title">Conflitti di canone</span>
                  <span className="mono-chip">{conflicts.length}</span>
                </div>
                <div className="panel-pad section-stack">
                  {conflicts.length === 0 ? (
                    <p className="field-hint">
                      Nessun conflitto aperto: i libri rendono i termini di serie come il canone.
                    </p>
                  ) : (
                    <ul className="section-stack">
                      {conflicts.map(({ project, finding }) => (
                        <li
                          key={finding.id}
                          className="rounded border border-line p-2 text-xs"
                        >
                          <span className="flex flex-wrap items-center gap-1">
                            <span className="badge badge-warning">{project.name}</span>
                            <span className="badge badge-neutral">{finding.kind}</span>
                          </span>
                          <p className="mt-1 font-mono text-[0.65rem] break-all text-muted">
                            {finding.details_json}
                          </p>
                          <p className="field-hint">
                            Modifica il termine di serie oppure il glossario del libro per
                            risolverlo; il libro resta comunque sovrano.
                          </p>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              </div>

              <div className="panel">
                <div className="panel-head">
                  <span className="panel-title">Promuovi dal libro al canone</span>
                </div>
                <div className="panel-pad section-stack">
                  <FormField label="Libro" htmlFor="series-promote-book">
                    <select
                      id="series-promote-book"
                      className="select"
                      value={promoteProjectId}
                      onChange={(event) => {
                        setPromoteProjectId(event.target.value);
                      }}
                    >
                      <option value="">Scegli un libro membro…</option>
                      {detail.projects.map((member) => (
                        <option key={member.id} value={member.id}>
                          {member.name}
                        </option>
                      ))}
                    </select>
                  </FormField>
                  {promoteProjectId !== "" ? (
                    promoteTerms.length === 0 ? (
                      <p className="field-hint">Il libro non ha termini di glossario.</p>
                    ) : (
                      <ul className="section-stack" style={{ maxHeight: "16rem", overflowY: "auto" }}>
                        {promoteTerms.map((term) => (
                          <li
                            key={term.id}
                            className="flex flex-wrap items-center justify-between gap-2 rounded border border-line p-2"
                          >
                            <span className="min-w-0 text-xs">
                              <span className="font-medium text-ink">{term.source}</span> →{" "}
                              {term.target}{" "}
                              <span className="badge badge-neutral">{kindLabel(term.kind)}</span>
                            </span>
                            <button
                              type="button"
                              className="btn btn-sm"
                              disabled={busy !== null}
                              onClick={() => void handlePromote(term)}
                            >
                              Promuovi
                            </button>
                          </li>
                        ))}
                      </ul>
                    )
                  ) : null}
                </div>
              </div>

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
                        busy !== null ||
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
                                    disabled={busy !== null || draft.target.trim().length === 0}
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
                                    disabled={busy !== null}
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
                                      disabled={busy !== null}
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
                                      disabled={busy !== null || newVariant.trim().length === 0}
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
            </>
          )}
        </div>
      </div>
    </div>
  );
}
