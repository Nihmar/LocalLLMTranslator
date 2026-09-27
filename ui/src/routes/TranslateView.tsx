import { useCallback, useEffect, useMemo, useState } from "react";
import { BookProfilePanel } from "../components/BookProfilePanel";
import { ChunkTable } from "../components/ChunkTable";
import type { ChunkRow } from "../components/ChunkTable";
import { EmptyState } from "../components/EmptyState";
import { LogView } from "../components/LogView";
import { ProgressBar } from "../components/ProgressBar";
import { ResourceGauge } from "../components/ResourceGauge";
import { StatusBadge } from "../components/StatusBadge";
import { onJobProgress, onMetricsTick } from "../lib/events";
import { countLabel, formatNumber } from "../lib/format";
import {
  chunkGet,
  chunkList,
  jobList,
  metricsGet,
  projectGet,
  toErrorMessage,
  translationCancel,
  translationPause,
  translationStart,
} from "../lib/ipc";
import type { Chunk, ChunkDetail, Chapter, Job, Metrics, Project } from "../lib/types";
import type { ViewId } from "../App";

/**
 * Step 3 of the wizard: the chunk table, the run controls and the resource gauge
 * (`PLAN.md` §11.3).
 *
 * The translation commands are **project-scoped**: `translation_start` takes a project and an
 * `only_retry` flag and `translation_cancel` takes the project id, while `translation_pause` takes
 * no argument and acts on the whole worker pool. There is no per-chunk command, so the table offers
 * a read-only detail drawer instead of per-row retry/skip controls.
 *
 * `job://progress` carries the serialized `Job` row; it is still treated as an invalidation trigger
 * (one row says nothing about the others), so the table is refetched rather than patched field by
 * field.
 */

export interface TranslateViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

const STATUS_FILTERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "all", label: "Tutti gli stati" },
  { value: "pending", label: "In attesa" },
  { value: "running", label: "In corso" },
  { value: "done", label: "Completati" },
  { value: "failed", label: "Falliti" },
  { value: "needs_review", label: "Da rivedere" },
];

const PAGE_SIZE = 200;

interface Counts {
  pending: number;
  running: number;
  done: number;
  failed: number;
  needs_review: number;
}

function countStatuses(chunks: readonly Chunk[]): Counts {
  const counts: Counts = { pending: 0, running: 0, done: 0, failed: 0, needs_review: 0 };
  for (const chunk of chunks) {
    if (chunk.status === "pending") {
      counts.pending += 1;
    } else if (chunk.status === "running") {
      counts.running += 1;
    } else if (chunk.status === "done") {
      counts.done += 1;
    } else if (chunk.status === "failed") {
      counts.failed += 1;
    } else if (chunk.status === "needs_review") {
      counts.needs_review += 1;
    }
  }
  return counts;
}

/** Parses a `*_json` column that holds a JSON array of strings; `[]` on anything unexpected. */
function parseStringArray(json: string): string[] {
  try {
    const parsed: unknown = JSON.parse(json);
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed.filter((item): item is string => typeof item === "string");
  } catch {
    return [];
  }
}

/** Reads `chunk_id` out of a job `payload_json`; `null` when the payload has no such field. */
function parseChunkId(payloadJson: string): string | null {
  try {
    const parsed: unknown = JSON.parse(payloadJson);
    if (parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)) {
      const value = (parsed as Record<string, unknown>)["chunk_id"];
      return typeof value === "string" ? value : null;
    }
    return null;
  } catch {
    return null;
  }
}

function blockOriginLabel(origin: string): string {
  switch (origin) {
    case "translator":
      return "traduttore";
    case "editor":
      return "editor";
    case "proofreader":
      return "proofreader";
    case "user":
      return "utente";
    default:
      return origin;
  }
}

export function TranslateView({ project, onNavigate }: TranslateViewProps) {
  const [chunks, setChunks] = useState<Chunk[]>([]);
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState("all");
  const [limit, setLimit] = useState(PAGE_SIZE);

  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [metricsLoading, setMetricsLoading] = useState(true);

  const [control, setControl] = useState<"idle" | "starting" | "pausing" | "cancelling">("idle");
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionNotice, setActionNotice] = useState<string | null>(null);

  const [detailId, setDetailId] = useState<string | null>(null);
  const [detail, setDetail] = useState<ChunkDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);

  // Bumped on every `job://progress` so the book profile panel refetches the
  // reconnaissance outcome without opening its own event listener.
  const [reconToken, setReconToken] = useState(0);

  const projectId = project?.id ?? null;

  // Always read the whole project: `chunk_list` has no limit and the status filter is applied
  // in the view, so the counters, the chapter outline and the table all describe the same
  // snapshot instead of drifting with the selected filter.
  const loadChunks = useCallback(async () => {
    if (projectId === null) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const rows = await chunkList({ project_id: projectId, status: null });
      setChunks(rows);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setChunks([]);
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  const loadContext = useCallback(async () => {
    if (projectId === null) {
      setChapters([]);
      setJobs([]);
      return;
    }
    try {
      const [detailRow, jobRows] = await Promise.all([
        projectGet(projectId),
        jobListSafe(projectId),
      ]);
      setChapters(detailRow.chapters);
      setJobs(jobRows);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
    }
  }, [projectId]);

  const loadMetrics = useCallback(async () => {
    try {
      setMetrics(await metricsGet());
    } catch {
      // The gauge degrades to "unknown"; it never blocks the table.
      setMetrics(null);
    }
  }, []);

  useEffect(() => {
    void loadChunks();
  }, [loadChunks]);

  useEffect(() => {
    void loadContext();
  }, [loadContext]);

  useEffect(() => {
    setMetricsLoading(true);
    void loadMetrics().finally(() => {
      setMetricsLoading(false);
    });
  }, [loadMetrics]);

  useEffect(() => {
    setLimit(PAGE_SIZE);
  }, [projectId, statusFilter]);

  // Any progress event invalidates the queue and the table; refetch instead of patching the single
  // job row it carries.
  useEffect(
    () =>
      onJobProgress(() => {
        void loadChunks();
        void loadContext();
        void loadMetrics();
        setReconToken((current) => current + 1);
      }),
    [loadChunks, loadContext, loadMetrics],
  );

  useEffect(
    () =>
      onMetricsTick(() => {
        void loadMetrics();
      }),
    [loadMetrics],
  );

  const chapterTitle = useMemo(() => {
    const map = new Map<string, string>();
    for (const chapter of chapters) {
      map.set(chapter.id, chapter.title);
    }
    return map;
  }, [chapters]);

  const attemptsByChunk = useMemo(() => {
    const map = new Map<string, number>();
    for (const job of jobs) {
      if (job.kind !== "translate_chunk") {
        continue;
      }
      const chunkId = parseChunkId(job.payload_json);
      if (chunkId === null) {
        continue;
      }
      map.set(chunkId, Math.max(map.get(chunkId) ?? 0, job.attempts));
    }
    return map;
  }, [jobs]);

  const visibleChunks = useMemo(
    () =>
      statusFilter === "all"
        ? chunks
        : chunks.filter((chunk) => chunk.status === statusFilter),
    [chunks, statusFilter],
  );

  const rows: ChunkRow[] = useMemo(
    () =>
      visibleChunks.slice(0, limit).map((chunk) => ({
        id: chunk.id,
        chapter_title:
          chunk.chapter_id === null ? null : (chapterTitle.get(chunk.chapter_id) ?? null),
        order_index: chunk.order_index,
        flags: parseStringArray(chunk.flags_json),
        block_count: parseStringArray(chunk.block_ids_json).length,
        token_estimate: chunk.token_estimate,
        status: chunk.status,
        model_id: chunk.model_id,
        attempts: attemptsByChunk.get(chunk.id) ?? 0,
        error: chunk.error,
      })),
    [visibleChunks, limit, chapterTitle, attemptsByChunk],
  );

  const counts = useMemo(() => countStatuses(chunks), [chunks]);
  const totalTokens = useMemo(
    () => chunks.reduce((sum, chunk) => sum + chunk.token_estimate, 0),
    [chunks],
  );

  async function runStart(onlyRetry: boolean) {
    if (projectId === null) {
      return;
    }
    setControl("starting");
    setActionError(null);
    setActionNotice(null);
    try {
      const result = await translationStart({ project_id: projectId, only_retry: onlyRetry });
      setActionNotice(
        `${countLabel(result.enqueued, "job accodato", "job accodati")}. L'esecutore è ${
          result.running ? "in esecuzione" : "fermo"
        }.`,
      );
      await loadChunks();
      await loadMetrics();
    } catch (startError) {
      setActionError(toErrorMessage(startError));
    } finally {
      setControl("idle");
    }
  }

  async function runPause() {
    setControl("pausing");
    setActionError(null);
    setActionNotice(null);
    try {
      await translationPause();
      setActionNotice(
        "Esecuzione sospesa: la coda resta e i chunk già in corso arrivano a termine. Riprendi con «Avvia».",
      );
      await loadMetrics();
    } catch (pauseError) {
      setActionError(toErrorMessage(pauseError));
    } finally {
      setControl("idle");
    }
  }

  async function runCancel() {
    setControl("cancelling");
    setActionError(null);
    setActionNotice(null);
    try {
      await translationCancel(projectId);
      setActionNotice("Esecuzione annullata: i job in attesa sono stati annullati.");
      await loadChunks();
      await loadContext();
      await loadMetrics();
    } catch (cancelError) {
      setActionError(toErrorMessage(cancelError));
    } finally {
      setControl("idle");
    }
  }

  async function openDetail(chunkId: string) {
    setDetailId(chunkId);
    setDetail(null);
    setDetailError(null);
    setDetailLoading(true);
    try {
      setDetail(await chunkGet(chunkId));
    } catch (loadError) {
      setDetailError(toErrorMessage(loadError));
    } finally {
      setDetailLoading(false);
    }
  }

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
  const hasChunks = visibleChunks.length > 0;

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-ink">Traduzione</h2>
          <p className="mt-0.5 text-xs text-muted">
            Progetto <span className="font-semibold text-ink-soft">{project.name}</span> —{" "}
            {countLabel(chunks.length, "chunk nel progetto", "chunk nel progetto")}
            {visibleChunks.length !== chunks.length
              ? ` · ${formatNumber(visibleChunks.length)} con lo stato scelto`
              : ""}
            {visibleChunks.length > limit ? ` · mostrati ${formatNumber(limit)}` : ""}.
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
                setStatusFilter(event.target.value);
              }}
            >
              {STATUS_FILTERS.map((filter) => (
                <option key={filter.value} value={filter.value}>
                  {filter.label}
                </option>
              ))}
            </select>
          </label>

          <button
            type="button"
            className="btn btn-primary"
            disabled={busy}
            onClick={() => {
              void runStart(false);
            }}
          >
            {control === "starting" ? <span className="spinner" aria-hidden="true" /> : null}
            Avvia / Riprendi
          </button>

          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => {
              void runStart(true);
            }}
            title="Rimette in coda solo i chunk falliti o da rivedere"
          >
            Riprova falliti
          </button>

          <button type="button" className="btn" disabled={busy} onClick={() => void runPause()}>
            {control === "pausing" ? <span className="spinner" aria-hidden="true" /> : null}
            Pausa
          </button>

          <button type="button" className="btn" disabled={busy} onClick={() => void runCancel()}>
            {control === "cancelling" ? <span className="spinner" aria-hidden="true" /> : null}
            Annulla
          </button>

          <button
            type="button"
            className="btn"
            disabled={loading}
            onClick={() => {
              void loadChunks();
              void loadMetrics();
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
          <BookProfilePanel projectId={project.id} reloadToken={reconToken} />

          <div className="panel panel-pad section-stack">
            <ProgressBar
              value={counts.done}
              total={chunks.length}
              label="Avanzamento complessivo"
              tone="accent"
              showCounts
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

          <div className="panel flex min-h-0 flex-col" style={{ maxHeight: "34rem" }}>
            <div className="panel-head">
              <span className="panel-title">Chunk</span>
              <span className="flex items-center gap-2">
                <span className="mono-chip">
                  {visibleChunks.length > limit
                    ? `${formatNumber(limit)} di ${formatNumber(visibleChunks.length)} righe`
                    : `${formatNumber(visibleChunks.length)} righe`}
                </span>
                {visibleChunks.length > limit ? (
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
                chunks={rows}
                onOpenDetails={(chunkId) => {
                  void openDetail(chunkId);
                }}
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
                        {detail.chunk.chapter_id === null
                          ? "—"
                          : (chapterTitle.get(detail.chunk.chapter_id) ?? detail.chunk.chapter_id)}
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
          <ResourceGauge metrics={metrics} loading={metricsLoading} compact />

          <div className="panel panel-pad">
            <div className="panel-title mb-2">Come si comporta la coda</div>
            <ul className="list-disc space-y-1 pl-4 text-xs text-muted">
              <li>Un chunk è l&apos;unità di lavoro e di checkpoint: ogni chunk completato è salvato.</li>
              <li>
                «Pausa» ferma l&apos;intero esecutore e non riceve argomenti (
                <span className="mono-chip">translation_pause</span>); «Annulla» agisce sul progetto
                aperto, il cui id viene passato a{" "}
                <span className="mono-chip">translation_cancel</span>.
              </li>
              <li>
                «Avvia / Riprendi» rimette in coda i chunk non completati; «Riprova falliti» solo
                quelli falliti o da rivedere.
              </li>
              <li>I retry automatici rispettano il limite di tentativi del job.</li>
            </ul>
          </div>

          <LogView limit={300} heightClass="h-64" />
        </div>
      </div>
    </div>
  );
}

/** `job_list` for a project, tolerating a failure (the table can render without attempts). */
async function jobListSafe(projectId: string): Promise<Job[]> {
  try {
    return await jobList({ project_id: projectId, limit: 1000 });
  } catch {
    return [];
  }
}
