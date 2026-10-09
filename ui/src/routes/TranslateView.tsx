import { useCallback, useEffect, useMemo, useState } from "react";
import { ChapterReader } from "../components/ChapterReader";
import type { ChunkRow } from "../components/ChunkTable";
import { EmptyState } from "../components/EmptyState";
import { chapterProgress, chunkTranslated } from "../lib/chapters";
import { onJobProgress, onMetricsTick } from "../lib/events";
import { countLabel } from "../lib/format";
import { isActiveJobState } from "../lib/jobs";
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
import { ChunkBrowser } from "./translate/ChunkBrowser";
import { ChunkDetailPanel } from "./translate/ChunkDetailPanel";
import { TranslateControls } from "./translate/TranslateControls";
import { TranslateProgressPanel } from "./translate/TranslateProgressPanel";
import { TranslateSidebar } from "./translate/TranslateSidebar";
import {
  countStatuses,
  PAGE_SIZE,
  parseChunkId,
  parseStringArray,
} from "./translate/shared";

/**
 * Translation page (`PLAN.md` §11.3): the chunk table, the run controls and the resource gauge.
 *
 * The translation commands are **project-scoped**: `translation_start` takes a project and an
 * `only_retry` flag and `translation_cancel` takes the project id, while `translation_pause` takes
 * no argument and acts on the whole worker pool. There is no per-chunk command, so the table offers
 * a read-only detail drawer instead of per-row retry/skip controls.
 *
 * `job://progress` carries the serialized `Job` row; it is still treated as an invalidation trigger
 * (one row says nothing about the others), so the table is refetched rather than patched field by
 * field.
 *
 * The chapter outline is a second, coarser reading of the same rows: double-clicking a chapter
 * opens a live preview composed from the chunks already in memory, so it follows a running
 * translation without another command or a second data source (`PLAN.md` §11.3).
 *
 * The panels in `./translate/` own the presentation; this component keeps the loaded rows, the
 * filters and the run controls.
 */

export interface TranslateViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
  /** Opens the shared job monitor, scoped to the open project. */
  onOpenJobs: () => void;
}

export function TranslateView({ project, onNavigate, onOpenJobs }: TranslateViewProps) {
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
    () => (statusFilter === "all" ? chunks : chunks.filter((chunk) => chunk.status === statusFilter)),
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
  // Progress follows the text, not the status: a `needs_review` chunk that kept its
  // translation is translated, the same predicate the export uses.
  const translatedCount = useMemo(() => chunks.filter(chunkTranslated).length, [chunks]);
  const activeJobs = useMemo(() => jobs.filter((job) => isActiveJobState(job.state)).length, [jobs]);
  const pendingJobs = useMemo(() => jobs.filter((job) => job.state === "pending").length, [jobs]);
  const totalTokens = useMemo(
    () => chunks.reduce((sum, chunk) => sum + chunk.token_estimate, 0),
    [chunks],
  );

  const chapterRows = useMemo(() => chapterProgress(chapters, chunks), [chapters, chunks]);

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

  if (project === null) {
    return (
      <div className="section-stack">
        <h1 className="font-serif text-2xl font-medium text-ink">Traduci</h1>
        <EmptyState
          title="Nessun libro aperto"
          description="La traduzione lavora sui chunk di un progetto: aprine uno e importa il documento."
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
      <TranslateControls
        projectName={project.name}
        chunksTotal={chunks.length}
        visibleCount={visibleChunks.length}
        limit={limit}
        statusFilter={statusFilter}
        onStatusFilter={setStatusFilter}
        control={control}
        loading={loading}
        onStart={() => {
          void runStart(false);
        }}
        onRetry={() => {
          void runStart(true);
        }}
        onPause={() => {
          void runPause();
        }}
        onCancel={() => {
          void runCancel();
        }}
        onReload={() => {
          void loadChunks();
          void loadMetrics();
        }}
      />

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

      <TranslateProgressPanel
        translatedCount={translatedCount}
        chunksTotal={chunks.length}
        counts={counts}
        totalTokens={totalTokens}
      />

      {chapterRows.length > 0 ? <ChapterReader chapters={chapterRows} chunks={chunks} /> : null}

      <details className="panel">
        <summary className="panel-head cursor-pointer">
          <span className="panel-title">Dettagli tecnici: chunk, risorse, coda e log</span>
        </summary>
        <div className="panel-pad grid grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_20rem]">
          <div className="section-stack min-w-0">
            <ChunkBrowser
              loading={loading}
              error={error}
              rows={rows}
              visibleCount={visibleChunks.length}
              limit={limit}
              statusFilter={statusFilter}
              onReload={() => void loadChunks()}
              onLoadMore={() => {
                setLimit((current) => current + PAGE_SIZE);
              }}
              onOpenDetails={(chunkId) => {
                void openDetail(chunkId);
              }}
              onNavigate={onNavigate}
              onResetFilter={() => {
                setStatusFilter("all");
              }}
            />

            {detailId !== null ? (
              <ChunkDetailPanel
                detailId={detailId}
                detail={detail}
                detailLoading={detailLoading}
                detailError={detailError}
                chapterTitle={chapterTitle}
                onReload={() => {
                  void openDetail(detailId);
                }}
                onClose={() => {
                  setDetailId(null);
                  setDetail(null);
                  setDetailError(null);
                }}
              />
            ) : null}
          </div>

          <TranslateSidebar
            metrics={metrics}
            metricsLoading={metricsLoading}
            activeJobs={activeJobs}
            pendingJobs={pendingJobs}
            onOpenJobs={onOpenJobs}
          />
        </div>
      </details>
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
