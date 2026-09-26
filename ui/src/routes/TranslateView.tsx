import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChunkTable } from "../components/ChunkTable";
import { EmptyState } from "../components/EmptyState";
import { LogView } from "../components/LogView";
import { ProgressBar } from "../components/ProgressBar";
import { ResourceGauge } from "../components/ResourceGauge";
import { StatusBadge } from "../components/StatusBadge";
import { onJobProgress, onMetricsTick } from "../lib/events";
import { countLabel, formatNumber, formatTokens } from "../lib/format";
import {
  chunkGet,
  chunkList,
  metricsGet,
  roleBindingList,
  toErrorMessage,
  translationCancel,
  translationPause,
  translationStart,
} from "../lib/ipc";
import type {
  BlockTranslation,
  ChunkDetail,
  ChunkStatus,
  ChunkSummary,
  Metrics,
  Project,
} from "../lib/types";
import type { ViewId } from "../App";

/**
 * Step 3 of the wizard: the chunk table, the run controls and the resource gauge
 * (`PLAN.md` §11.3).
 *
 * The table is patched from `job://progress` events — one row at a time, keyed by `chunk_id` —
 * instead of re-querying `chunk_list` on every event. `chunk_list` is only called when the
 * filter, the page size or the project changes.
 */

export interface TranslateViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

const STATUS_FILTERS: ReadonlyArray<{ value: ChunkStatus | "all"; label: string }> = [
  { value: "all", label: "Tutti gli stati" },
  { value: "pending", label: "In attesa" },
  { value: "running", label: "In corso" },
  { value: "done", label: "Completati" },
  { value: "failed", label: "Falliti" },
  { value: "needs_review", label: "Da rivedere" },
];

const PAGE_SIZE = 200;
const HIGHLIGHT_MS = 900;

interface Counts {
  pending: number;
  running: number;
  done: number;
  failed: number;
  needs_review: number;
}

function countStatuses(chunks: readonly ChunkSummary[]): Counts {
  const counts: Counts = { pending: 0, running: 0, done: 0, failed: 0, needs_review: 0 };
  for (const chunk of chunks) {
    counts[chunk.status] += 1;
  }
  return counts;
}

function blockOriginLabel(origin: BlockTranslation["origin"]): string {
  switch (origin) {
    case "translator":
      return "traduttore";
    case "editor":
      return "editor";
    case "proofreader":
      return "proofreader";
    case "user":
      return "utente";
  }
}

export function TranslateView({ project, onNavigate }: TranslateViewProps) {
  const [chunks, setChunks] = useState<ChunkSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState<ChunkStatus | "all">("all");
  const [limit, setLimit] = useState(PAGE_SIZE);

  const [selection, setSelection] = useState<ReadonlySet<string>>(new Set());
  const [busyIds, setBusyIds] = useState<ReadonlySet<string>>(new Set());
  const [highlightedIds, setHighlightedIds] = useState<ReadonlySet<string>>(new Set());

  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [metricsLoading, setMetricsLoading] = useState(true);

  const [translatorModels, setTranslatorModels] = useState<string[]>([]);
  const [modelOverride, setModelOverride] = useState("");

  const [control, setControl] = useState<"idle" | "starting" | "pausing" | "cancelling">("idle");
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionNotice, setActionNotice] = useState<string | null>(null);

  const [detailId, setDetailId] = useState<string | null>(null);
  const [detail, setDetail] = useState<ChunkDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);

  const highlightTimer = useRef<number | null>(null);
  const projectId = project?.id ?? null;

  const loadChunks = useCallback(async () => {
    if (projectId === null) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const rows = await chunkList({
        project_id: projectId,
        status: statusFilter === "all" ? undefined : statusFilter,
        limit,
        offset: 0,
      });
      setChunks(rows);
      setSelection((current) => {
        const visible = new Set(rows.map((row) => row.id));
        return new Set([...current].filter((id) => visible.has(id)));
      });
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setChunks([]);
    } finally {
      setLoading(false);
    }
  }, [projectId, statusFilter, limit]);

  useEffect(() => {
    setLimit(PAGE_SIZE);
  }, [projectId, statusFilter]);

  useEffect(() => {
    void loadChunks();
  }, [loadChunks]);

  const loadMetrics = useCallback(async () => {
    setMetricsLoading(true);
    try {
      const snapshot = await metricsGet(projectId);
      setMetrics(snapshot);
    } catch {
      // The gauge degrades to "unknown"; it never blocks the table.
      setMetrics(null);
    } finally {
      setMetricsLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    void loadMetrics();
  }, [loadMetrics]);

  useEffect(
    () =>
      onMetricsTick((snapshot) => {
        if (projectId !== null && snapshot.project_id !== null && snapshot.project_id !== projectId) {
          return;
        }
        setMetrics(snapshot);
        setMetricsLoading(false);
      }),
    [projectId],
  );

  // Incremental row updates: one event patches exactly one row.
  useEffect(
    () =>
      onJobProgress((event) => {
        const chunkId = event.chunk_id;
        const status = event.chunk_status;
        if (projectId === null || event.project_id !== projectId || chunkId === null || status === null) {
          return;
        }

        setChunks((current) => {
          let found = false;
          const next = current.map((row) => {
            if (row.id !== chunkId) {
              return row;
            }
            found = true;
            return {
              ...row,
              status,
              attempts: event.attempts,
              model_id: event.model ?? row.model_id,
              error: event.error,
              updated_at: event.updated_at,
            };
          });
          // The chunk is outside the loaded page/filter: nothing to patch, not worth a refetch.
          return found ? next : current;
        });

        setHighlightedIds((current) => new Set([...current, chunkId]));
        if (highlightTimer.current !== null) {
          window.clearTimeout(highlightTimer.current);
        }
        highlightTimer.current = window.setTimeout(() => {
          setHighlightedIds(new Set());
          highlightTimer.current = null;
        }, HIGHLIGHT_MS);
      }),
    [projectId],
  );

  useEffect(
    () => () => {
      if (highlightTimer.current !== null) {
        window.clearTimeout(highlightTimer.current);
      }
    },
    [],
  );

  useEffect(() => {
    let cancelled = false;
    void roleBindingList({ role: "translator" })
      .then((bindings) => {
        if (cancelled) {
          return;
        }
        const models = [...new Set(bindings.map((binding) => binding.model))];
        setTranslatorModels(models);
      })
      .catch(() => {
        if (!cancelled) {
          setTranslatorModels([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const counts = useMemo(() => countStatuses(chunks), [chunks]);
  const totalTokens = useMemo(
    () => chunks.reduce((sum, chunk) => sum + chunk.token_estimate, 0),
    [chunks],
  );

  const throughput = metrics?.throughput ?? null;
  const overallTotal = throughput === null ? chunks.length : throughput.chunks_total;
  const overallDone = throughput === null ? counts.done : throughput.chunks_done;

  function markBusy(ids: readonly string[], busy: boolean) {
    setBusyIds((current) => {
      const next = new Set(current);
      for (const id of ids) {
        if (busy) {
          next.add(id);
        } else {
          next.delete(id);
        }
      }
      return next;
    });
  }

  async function runStart(chunkIds: readonly string[], label: string, useModelOverride: boolean) {
    if (projectId === null) {
      return;
    }
    setControl("starting");
    setActionError(null);
    setActionNotice(null);
    markBusy(chunkIds, true);
    try {
      const ack = await translationStart({
        project_id: projectId,
        chunk_ids: chunkIds.length > 0 ? [...chunkIds] : undefined,
        model: useModelOverride && modelOverride.length > 0 ? modelOverride : null,
      });
      setActionNotice(
        `${label}: ${countLabel(ack.accepted, "job accodato", "job accodati")}${
          ack.message === null ? "." : ` — ${ack.message}`
        }`,
      );
    } catch (startError) {
      setActionError(toErrorMessage(startError));
    } finally {
      markBusy(chunkIds, false);
      setControl("idle");
    }
  }

  async function runPause() {
    if (projectId === null) {
      return;
    }
    setControl("pausing");
    setActionError(null);
    setActionNotice(null);
    try {
      const ack = await translationPause({ project_id: projectId });
      setActionNotice(
        `Esecuzione sospesa: ${countLabel(ack.accepted, "job rilasciato", "job rilasciati")}. Riprendi con «Avvia».`,
      );
      await loadMetrics();
    } catch (pauseError) {
      setActionError(toErrorMessage(pauseError));
    } finally {
      setControl("idle");
    }
  }

  async function runSkip(chunkIds: readonly string[]) {
    if (projectId === null || chunkIds.length === 0) {
      return;
    }
    setControl("cancelling");
    setActionError(null);
    setActionNotice(null);
    markBusy(chunkIds, true);
    try {
      const ack = await translationCancel({ project_id: projectId, chunk_ids: [...chunkIds] });
      setActionNotice(
        `${countLabel(ack.accepted, "job annullato", "job annullati")}. Il chunk resta nello stato attuale e viene ripreso solo con un nuovo avvio esplicito.`,
      );
      setSelection(new Set());
    } catch (cancelError) {
      setActionError(toErrorMessage(cancelError));
    } finally {
      markBusy(chunkIds, false);
      setControl("idle");
    }
  }

  async function openDetail(chunkId: string) {
    setDetailId(chunkId);
    setDetail(null);
    setDetailError(null);
    setDetailLoading(true);
    try {
      const loaded = await chunkGet(chunkId);
      setDetail(loaded);
    } catch (loadError) {
      setDetailError(toErrorMessage(loadError));
    } finally {
      setDetailLoading(false);
    }
  }

  const selectedIds = useMemo(() => [...selection], [selection]);

  const detailRows = useMemo(() => {
    if (detail === null) {
      return [];
    }
    const byBlock = new Map(detail.translations.map((entry) => [entry.block_id, entry]));
    return detail.blocks.map((block) => ({
      block,
      translation: byBlock.get(block.id) ?? null,
    }));
  }, [detail]);

  if (project === null) {
    return (
      <div className="section-stack">
        <h2 className="text-lg font-semibold text-ink">Traduzione</h2>
        <EmptyState
          title="Nessun progetto aperto"
          description="La traduzione lavora sui chunk di un progetto: aprine uno e importa il documento."
          actionLabel="Vai ai progetti"
          onAction={() => {
            onNavigate("projects");
          }}
        />
      </div>
    );
  }

  const busy = control !== "idle";
  const hasChunks = chunks.length > 0;

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-ink">Traduzione</h2>
          <p className="mt-0.5 text-xs text-muted">
            Progetto <span className="font-semibold text-ink-soft">{project.name}</span> —{" "}
            {countLabel(chunks.length, "chunk caricato", "chunk caricati")}
            {chunks.length === limit ? ` (limite di pagina ${formatNumber(limit)})` : ""}.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <label className="flex items-center gap-1 text-xs text-muted">
            Stato
            <select
              className="select"
              style={{ width: "auto" }}
              value={statusFilter}
              onChange={(event) => {
                const value = event.target.value;
                if (value === "all") {
                  setStatusFilter("all");
                  return;
                }
                const matched = STATUS_FILTERS.find((filter) => filter.value === value);
                if (matched !== undefined && matched.value !== "all") {
                  setStatusFilter(matched.value);
                }
              }}
            >
              {STATUS_FILTERS.map((filter) => (
                <option key={filter.value} value={filter.value}>
                  {filter.label}
                </option>
              ))}
            </select>
          </label>

          <label className="flex items-center gap-1 text-xs text-muted">
            Modello
            <select
              className="select"
              style={{ width: "auto" }}
              value={modelOverride}
              onChange={(event) => {
                setModelOverride(event.target.value);
              }}
              title="Sovrascrive il modello del ruolo traduttore per i prossimi avvii"
            >
              <option value="">dal ruolo traduttore</option>
              {translatorModels.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </label>

          <button
            type="button"
            className="btn btn-primary"
            disabled={busy}
            onClick={() => {
              void runStart([], "Traduzione avviata", true);
            }}
          >
            {control === "starting" ? <span className="spinner" aria-hidden="true" /> : null}
            Avvia / Riprendi
          </button>

          <button type="button" className="btn" disabled={busy} onClick={() => void runPause()}>
            {control === "pausing" ? <span className="spinner" aria-hidden="true" /> : null}
            Pausa
          </button>

          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => {
              void loadChunks();
            }}
            title="Rilegge la tabella dal database"
          >
            Aggiorna
          </button>
        </div>
      </div>

      {actionError !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{actionError}</span>
        </div>
      ) : null}

      {actionNotice !== null ? (
        <div className="banner banner-ok" role="status">
          <span aria-hidden="true">✓</span>
          <span>{actionNotice}</span>
        </div>
      ) : null}

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_20rem]">
        <div className="section-stack min-w-0">
          <div className="panel panel-pad section-stack">
            <ProgressBar
              value={overallDone}
              total={overallTotal}
              label="Avanzamento complessivo"
              tone="accent"
              showCounts
              hint={
                throughput === null
                  ? undefined
                  : `${formatTokens(throughput.tokens_completion)} generati`
              }
            />

            <div className="grid grid-cols-6 gap-2">
              <div className="stat-tile">
                <div className="stat-label">In attesa</div>
                <div className="stat-value">{formatNumber(counts.pending)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">In corso</div>
                <div className="stat-value">{formatNumber(counts.running)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Completati</div>
                <div className="stat-value">{formatNumber(counts.done)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Falliti</div>
                <div className="stat-value">{formatNumber(counts.failed)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Da rivedere</div>
                <div className="stat-value">{formatNumber(counts.needs_review)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Token stimati</div>
                <div className="stat-value">{formatNumber(totalTokens)}</div>
              </div>
            </div>
          </div>

          {selection.size > 0 ? (
            <div className="panel panel-pad flex flex-wrap items-center gap-2">
              <StatusBadge status="ok" tone="accent" label={`${formatNumber(selection.size)} selezionati`} />
              <button
                type="button"
                className="btn btn-sm btn-primary"
                disabled={busy}
                onClick={() => {
                  void runStart(selectedIds, "Riprova dei chunk selezionati", false);
                }}
                title="Rimette in coda i chunk selezionati con la configurazione del ruolo"
              >
                Riprova selezionati
              </button>
              <button
                type="button"
                className="btn btn-sm"
                disabled={busy || modelOverride.length === 0}
                onClick={() => {
                  void runStart(selectedIds, "Ritraduzione con un altro modello", true);
                }}
                title={
                  modelOverride.length === 0
                    ? "Scegli prima un modello nel selettore in alto"
                    : `Ritraduce i chunk selezionati con ${modelOverride}`
                }
              >
                Ritraduci con il modello scelto
              </button>
              <button
                type="button"
                className="btn btn-sm btn-ghost"
                disabled={busy}
                onClick={() => {
                  void runSkip(selectedIds);
                }}
                title="Annulla i job in attesa dei chunk selezionati"
              >
                Salta selezionati
              </button>
              <button
                type="button"
                className="btn btn-sm btn-ghost"
                onClick={() => {
                  setSelection(new Set());
                }}
              >
                Deseleziona
              </button>
            </div>
          ) : null}

          <div className="panel flex min-h-0 flex-col" style={{ maxHeight: "34rem" }}>
            <div className="panel-head">
              <span className="panel-title">Chunk</span>
              <span className="flex items-center gap-2">
                <span className="mono-chip">{formatNumber(chunks.length)} righe</span>
                {chunks.length === limit ? (
                  <button
                    type="button"
                    className="btn btn-sm"
                    disabled={loading}
                    onClick={() => {
                      setLimit((current) => current + PAGE_SIZE);
                    }}
                  >
                    Carica altri {formatNumber(PAGE_SIZE)}
                  </button>
                ) : null}
              </span>
            </div>

            {loading ? (
              <div className="panel-pad">
                <EmptyState tone="loading" compact title="Lettura dei chunk…" />
              </div>
            ) : error !== null ? (
              <div className="panel-pad">
                <EmptyState
                  tone="error"
                  compact
                  title="Impossibile leggere i chunk"
                  details={error}
                  actionLabel="Riprova"
                  onAction={() => {
                    void loadChunks();
                  }}
                />
              </div>
            ) : !hasChunks ? (
              <div className="panel-pad">
                <EmptyState
                  compact
                  title={
                    statusFilter === "all"
                      ? "Nessun chunk nel progetto"
                      : "Nessun chunk con questo stato"
                  }
                  description={
                    statusFilter === "all"
                      ? "Importa il documento dalla pagina Ingestione: i chunk vengono costruiti lì."
                      : "Cambia il filtro per vedere gli altri chunk."
                  }
                  actionLabel={statusFilter === "all" ? "Vai all'ingestione" : "Mostra tutti"}
                  onAction={() => {
                    if (statusFilter === "all") {
                      onNavigate("ingest");
                    } else {
                      setStatusFilter("all");
                    }
                  }}
                />
              </div>
            ) : (
              <ChunkTable
                chunks={chunks}
                selection={selection}
                onSelectionChange={setSelection}
                onRetry={(chunkId) => {
                  void runStart([chunkId], "Riprova del chunk", false);
                }}
                onSkip={(chunkId) => {
                  void runSkip([chunkId]);
                }}
                onOpenDetails={(chunkId) => {
                  void openDetail(chunkId);
                }}
                busyIds={busyIds}
                highlightedIds={highlightedIds}
              />
            )}
          </div>

          {detailId !== null ? (
            <div className="panel">
              <div className="panel-head">
                <span className="panel-title">
                  Dettagli chunk <span className="mono-chip">{detailId}</span>
                </span>
                <span className="flex items-center gap-2">
                  <button
                    type="button"
                    className="btn btn-sm"
                    disabled={detailLoading}
                    onClick={() => {
                      void openDetail(detailId);
                    }}
                  >
                    Ricarica
                  </button>
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => {
                      setDetailId(null);
                      setDetail(null);
                      setDetailError(null);
                    }}
                  >
                    Chiudi
                  </button>
                </span>
              </div>

              {detailLoading ? (
                <div className="panel-pad">
                  <EmptyState tone="loading" compact title="Lettura del chunk…" />
                </div>
              ) : detailError !== null ? (
                <div className="panel-pad">
                  <EmptyState tone="error" compact title="Impossibile leggere il chunk" details={detailError} />
                </div>
              ) : detail === null ? (
                <div className="panel-pad">
                  <EmptyState compact title="Nessun dato" />
                </div>
              ) : (
                <div className="panel-pad section-stack">
                  <div className="grid grid-cols-4 gap-2">
                    <div className="stat-tile">
                      <div className="stat-label">Capitolo</div>
                      <div className="truncate text-xs text-ink-soft">
                        {detail.chunk.chapter_id ?? "—"}
                      </div>
                    </div>
                    <div className="stat-tile">
                      <div className="stat-label">Token stimati</div>
                      <div className="stat-value">{formatNumber(detail.chunk.token_estimate)}</div>
                    </div>
                    <div className="stat-tile">
                      <div className="stat-label">Blocchi</div>
                      <div className="stat-value">{formatNumber(detail.blocks.length)}</div>
                    </div>
                    <div className="stat-tile">
                      <div className="stat-label">Hash prompt</div>
                      <div className="truncate font-mono text-[0.7rem] text-muted">
                        {detail.chunk.prompt_hash ?? "—"}
                      </div>
                    </div>
                  </div>

                  {detailRows.length === 0 ? (
                    <EmptyState
                      compact
                      title="Nessun blocco associato"
                      description="Il chunk è stato salvato ma i blocchi non sono leggibili: verifica la migrazione o ripeti l'ingestione."
                    />
                  ) : (
                    <div className="table-scroll" style={{ maxHeight: "26rem" }}>
                      <table className="data-table">
                        <thead>
                          <tr>
                            <th style={{ width: "9rem" }}>Blocco</th>
                            <th>Sorgente</th>
                            <th>Traduzione</th>
                          </tr>
                        </thead>
                        <tbody>
                          {detailRows.map(({ block, translation }) => (
                            <tr key={block.id}>
                              <td>
                                <span className="block">
                                  <span className="mono-chip">{block.id}</span>
                                </span>
                                <span className="mt-1 block text-[0.68rem] text-faint">
                                  {block.kind}
                                  {block.level > 0 ? ` · L${formatNumber(block.level)}` : ""}
                                  {block.translatable ? "" : " · non traducibile"}
                                </span>
                              </td>
                              <td>
                                <pre className="max-w-prose font-mono text-[0.72rem] whitespace-pre-wrap text-ink-soft">
                                  {block.source_md}
                                </pre>
                              </td>
                              <td>
                                {translation === null ? (
                                  <span className="text-faint">— nessuna traduzione —</span>
                                ) : (
                                  <>
                                    <pre className="max-w-prose font-mono text-[0.72rem] whitespace-pre-wrap text-ink">
                                      {translation.text_md}
                                    </pre>
                                    <span className="mt-1 flex items-center gap-1">
                                      <span className="mono-chip">{blockOriginLabel(translation.origin)}</span>
                                      <StatusBadge
                                        status={translation.placeholders_ok ? "ok" : "failed"}
                                        tone={translation.placeholders_ok ? "success" : "danger"}
                                        label={
                                          translation.placeholders_ok
                                            ? "placeholder integri"
                                            : "placeholder alterati"
                                        }
                                      />
                                      {translation.edited_by_user ? (
                                        <span className="badge badge-neutral">modificato a mano</span>
                                      ) : null}
                                    </span>
                                  </>
                                )}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )}

                  {detail.chunk.error !== null ? (
                    <div className="banner banner-error" role="alert">
                      <span aria-hidden="true">⚠</span>
                      <span>{detail.chunk.error}</span>
                    </div>
                  ) : null}
                </div>
              )}
            </div>
          ) : null}
        </div>

        <div className="section-stack">
          <ResourceGauge resources={metrics?.resources ?? null} loading={metricsLoading} compact />

          <div className="panel panel-pad">
            <div className="panel-title mb-2">Come si comporta la coda</div>
            <ul className="list-disc space-y-1 pl-4 text-xs text-muted">
              <li>Un chunk è l&apos;unità di lavoro e di checkpoint: ogni chunk completato è salvato.</li>
              <li>
                «Pausa» rilascia i lease; «Avvia / Riprendi» rimette in coda ciò che non è ancora
                completato.
              </li>
              <li>
                «Salta» annulla i job in attesa del chunk: non marca il chunk come tradotto e non
                esiste un comando per uno scarto permanente nel contratto attuale.
              </li>
              <li>I retry automatici rispettano il limite di tentativi del job.</li>
            </ul>
          </div>

          <LogView projectId={projectId} limit={300} heightClass="h-64" />
        </div>
      </div>
    </div>
  );
}
