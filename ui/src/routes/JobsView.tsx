import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { LogView } from "../components/LogView";
import { ProgressBar } from "../components/ProgressBar";
import { ResourceGauge } from "../components/ResourceGauge";
import { StatusBadge } from "../components/StatusBadge";
import { onJobProgress, onMetricsTick } from "../lib/events";
import {
  countLabel,
  estimateRemainingMs,
  formatDateTime,
  formatDuration,
  formatEta,
  formatNumber,
  formatRelative,
  formatThroughput,
  formatTokens,
  shortId,
  truncate,
} from "../lib/format";
import { jobList, metricsGet, toErrorMessage } from "../lib/ipc";
import type { Job, JobKind, JobState, Metrics, Project } from "../lib/types";
import type { ViewId } from "../App";

/**
 * Job dashboard: queue progress, ETA, resources and the live log (`PLAN.md` §11, "Dashboard job").
 *
 * The ETA is never a constant. It is derived from observed throughput only:
 *
 * 1. preferred source — the scheduler's own counters (`metrics_get` / `metrics://tick`):
 *    `elapsed_ms / chunks_done * chunks_remaining`;
 * 2. fallback for the window between the start of a run and the first metrics tick — what this
 *    page has actually watched: distinct chunks reaching a terminal state since the first
 *    `job://progress` event, over the wall-clock time since that event.
 *
 * When neither source has a completed chunk yet, the page says the estimate is not available
 * instead of inventing a number.
 */

export interface JobsViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

const JOB_LIMIT = 1000;
const TERMINAL_STATES: ReadonlySet<JobState> = new Set(["done", "failed", "cancelled"]);

const KIND_LABELS: Readonly<Record<JobKind, string>> = {
  ingest: "Ingestione",
  translate_chunk: "Traduzione chunk",
  summarize: "Riassunto",
  edit_chunk: "Revisione editor",
  proofread_chunk: "Proofread",
  qa_scan: "Scansione QA",
  export_unit: "Export",
};

const STATE_FILTERS: ReadonlyArray<{ value: JobState | "all"; label: string }> = [
  { value: "all", label: "Tutti gli stati" },
  { value: "pending", label: "In attesa" },
  { value: "leased", label: "Assegnati" },
  { value: "running", label: "In corso" },
  { value: "done", label: "Completati" },
  { value: "failed", label: "Falliti" },
  { value: "cancelled", label: "Annullati" },
];

const KIND_FILTERS: ReadonlyArray<{ value: JobKind | "all"; label: string }> = [
  { value: "all", label: "Tutti i tipi" },
  { value: "ingest", label: KIND_LABELS.ingest },
  { value: "translate_chunk", label: KIND_LABELS.translate_chunk },
  { value: "summarize", label: KIND_LABELS.summarize },
  { value: "edit_chunk", label: KIND_LABELS.edit_chunk },
  { value: "proofread_chunk", label: KIND_LABELS.proofread_chunk },
  { value: "qa_scan", label: KIND_LABELS.qa_scan },
  { value: "export_unit", label: KIND_LABELS.export_unit },
];

/** Re-renders on an interval while `active`, so elapsed time and ETA keep moving. */
function useTicker(active: boolean, intervalMs = 1000): number {
  const [now, setNow] = useState<number>(() => Date.now());

  useEffect(() => {
    if (!active) {
      return;
    }
    const timer = window.setInterval(() => {
      setNow(Date.now());
    }, intervalMs);
    return () => {
      window.clearInterval(timer);
    };
  }, [active, intervalMs]);

  return now;
}

interface EtaView {
  etaMs: number | null;
  /** Italian sentence explaining where the estimate comes from. */
  source: string;
}

export function JobsView({ project, onNavigate }: JobsViewProps) {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [stateFilter, setStateFilter] = useState<JobState | "all">("all");
  const [kindFilter, setKindFilter] = useState<JobKind | "all">("all");
  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [metricsError, setMetricsError] = useState<string | null>(null);

  // Observed-throughput fallback: distinct chunks seen finishing since the first event.
  const [sessionFinished, setSessionFinished] = useState(0);
  const [sessionStartedAt, setSessionStartedAt] = useState<number | null>(null);
  const finishedIds = useRef<Set<string>>(new Set());

  const projectId = project?.id ?? null;

  const load = useCallback(async () => {
    if (projectId === null) {
      setJobs([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const rows = await jobList({ project_id: projectId, limit: JOB_LIMIT });
      setJobs(rows);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setJobs([]);
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  const loadMetrics = useCallback(async () => {
    setMetricsError(null);
    try {
      const snapshot = await metricsGet(projectId);
      setMetrics(snapshot);
    } catch (loadError) {
      setMetricsError(toErrorMessage(loadError));
    }
  }, [projectId]);

  useEffect(() => {
    void load();
  }, [load]);

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
      }),
    [projectId],
  );

  // Live job state: one event patches exactly one row.
  useEffect(
    () =>
      onJobProgress((event) => {
        if (projectId === null || event.project_id !== projectId) {
          return;
        }

        setSessionStartedAt((current) => current ?? Date.now());

        setJobs((current) =>
          current.map((job) =>
            job.id === event.job_id
              ? {
                  ...job,
                  state: event.state,
                  attempts: event.attempts,
                  last_error: event.error,
                  finished_at: TERMINAL_STATES.has(event.state)
                    ? (job.finished_at ?? event.updated_at)
                    : job.finished_at,
                }
              : job,
          ),
        );

        if (event.kind === "translate_chunk" && TERMINAL_STATES.has(event.state)) {
          const key = event.chunk_id ?? event.job_id;
          if (!finishedIds.current.has(key)) {
            finishedIds.current.add(key);
            setSessionFinished(finishedIds.current.size);
          }
        }
      }),
    [projectId],
  );

  // The observation window belongs to one project only.
  useEffect(() => {
    finishedIds.current = new Set();
    setSessionFinished(0);
    setSessionStartedAt(null);
  }, [projectId]);

  const counts = useMemo(() => {
    const result: Record<JobState, number> = {
      pending: 0,
      leased: 0,
      running: 0,
      done: 0,
      failed: 0,
      cancelled: 0,
    };
    for (const job of jobs) {
      result[job.state] += 1;
    }
    return result;
  }, [jobs]);

  const translateJobs = useMemo(
    () => jobs.filter((job) => job.kind === "translate_chunk"),
    [jobs],
  );
  const translateTotal = translateJobs.length;
  const translateDone = useMemo(
    () => translateJobs.filter((job) => job.state === "done").length,
    [translateJobs],
  );

  const activeJobs = counts.pending + counts.leased + counts.running;
  const now = useTicker(activeJobs > 0);

  const throughput = metrics?.throughput ?? null;
  const sessionElapsedMs =
    sessionStartedAt === null ? 0 : Math.max(0, now - sessionStartedAt);

  const eta = useMemo((): EtaView => {
    if (throughput !== null && throughput.chunks_done > 0 && throughput.elapsed_ms > 0) {
      const total = throughput.chunks_total > 0 ? throughput.chunks_total : translateTotal;
      const etaMs = estimateRemainingMs(throughput.chunks_done, total, throughput.elapsed_ms);
      return {
        etaMs,
        source:
          etaMs === null
            ? "Stima non disponibile: la coda risulta già conclusa."
            : `Stima dal throughput reale del scheduler: ${formatNumber(
                throughput.chunks_done,
              )} chunk in ${formatDuration(throughput.elapsed_ms)}.`,
      };
    }

    if (sessionFinished > 0 && sessionElapsedMs > 0) {
      const total = translateTotal > 0 ? translateTotal : sessionFinished;
      const etaMs = estimateRemainingMs(sessionFinished, total, sessionElapsedMs);
      return {
        etaMs,
        source:
          etaMs === null
            ? "Stima non disponibile: la coda osservata risulta già conclusa."
            : `Stima da questa sessione: ${formatNumber(sessionFinished)} chunk conclusi in ${formatDuration(
                sessionElapsedMs,
              )}.`,
      };
    }

    return {
      etaMs: null,
      source:
        "Stima non disponibile: nessun chunk è ancora stato completato, quindi non esiste un throughput da cui derivarla.",
    };
  }, [throughput, sessionFinished, sessionElapsedMs, translateTotal]);

  const observedThroughput =
    throughput !== null && throughput.chunks_done > 0
      ? formatThroughput(throughput.chunks_done, throughput.elapsed_ms)
      : sessionFinished > 0
        ? formatThroughput(sessionFinished, sessionElapsedMs)
        : "—";

  const observedElapsed =
    throughput !== null && throughput.elapsed_ms > 0
      ? formatDuration(throughput.elapsed_ms)
      : sessionElapsedMs > 0
        ? formatDuration(sessionElapsedMs)
        : "—";

  const filteredJobs = useMemo(
    () =>
      jobs.filter((job) => {
        if (stateFilter !== "all" && job.state !== stateFilter) {
          return false;
        }
        if (kindFilter !== "all" && job.kind !== kindFilter) {
          return false;
        }
        return true;
      }),
    [jobs, stateFilter, kindFilter],
  );

  if (project === null) {
    return (
      <div className="section-stack">
        <h2 className="text-lg font-semibold text-ink">Job</h2>
        <EmptyState
          title="Nessun progetto aperto"
          description="La dashboard mostra la coda di un progetto: aprine uno per vedere job, progresso e log."
          actionLabel="Vai ai progetti"
          onAction={() => {
            onNavigate("projects");
          }}
        />
      </div>
    );
  }

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-ink">Job</h2>
          <p className="mt-0.5 text-xs text-muted">
            Progetto <span className="font-semibold text-ink-soft">{project.name}</span> — coda,
            progresso e log. {countLabel(activeJobs, "job attivo", "job attivi")}.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="btn"
            disabled={loading}
            onClick={() => {
              void load();
              void loadMetrics();
            }}
          >
            Aggiorna
          </button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => {
              onNavigate("translate");
            }}
          >
            Vai alla traduzione
          </button>
        </div>
      </div>

      {error !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{error}</span>
        </div>
      ) : null}

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_22rem]">
        <div className="section-stack min-w-0">
          <div className="panel panel-pad section-stack">
            <ProgressBar
              value={translateDone}
              total={translateTotal}
              label="Chunk di traduzione completati"
              showCounts
            />

            <div className="grid grid-cols-3 gap-2 xl:grid-cols-6">
              <div className="stat-tile">
                <div className="stat-label">In attesa</div>
                <div className="stat-value">{formatNumber(counts.pending)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">In corso</div>
                <div className="stat-value">{formatNumber(counts.running + counts.leased)}</div>
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
                <div className="stat-label">Annullati</div>
                <div className="stat-value">{formatNumber(counts.cancelled)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Tempo stimato</div>
                <div className="stat-value" style={{ fontSize: "0.85rem" }}>
                  {formatEta(eta.etaMs)}
                </div>
              </div>
            </div>

            <div className="grid grid-cols-3 gap-2">
              <div className="stat-tile">
                <div className="stat-label">Throughput osservato</div>
                <div className="stat-value" style={{ fontSize: "0.85rem" }}>
                  {observedThroughput}
                </div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Tempo trascorso</div>
                <div className="stat-value" style={{ fontSize: "0.85rem" }}>
                  {observedElapsed}
                </div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Latenza media</div>
                <div className="stat-value" style={{ fontSize: "0.85rem" }}>
                  {throughput === null || throughput.avg_latency_ms === null
                    ? "—"
                    : formatDuration(throughput.avg_latency_ms)}
                </div>
              </div>
            </div>

            <p className="text-[0.72rem] text-faint">
              {eta.source}
              {throughput === null
                ? ""
                : ` Token: ${formatTokens(throughput.tokens_prompt)} in ingresso, ${formatTokens(
                    throughput.tokens_completion,
                  )} generati.`}
            </p>
          </div>

          <div className="panel flex min-h-0 flex-col" style={{ maxHeight: "34rem" }}>
            <div className="panel-head">
              <span className="panel-title">Coda</span>
              <span className="flex flex-wrap items-center gap-2">
                <label className="flex items-center gap-1 text-xs text-muted">
                  Stato
                  <select
                    className="select"
                    style={{ width: "auto" }}
                    value={stateFilter}
                    onChange={(event) => {
                      const matched = STATE_FILTERS.find(
                        (entry) => entry.value === event.target.value,
                      );
                      if (matched !== undefined) {
                        setStateFilter(matched.value);
                      }
                    }}
                  >
                    {STATE_FILTERS.map((entry) => (
                      <option key={entry.value} value={entry.value}>
                        {entry.label}
                      </option>
                    ))}
                  </select>
                </label>

                <label className="flex items-center gap-1 text-xs text-muted">
                  Tipo
                  <select
                    className="select"
                    style={{ width: "auto" }}
                    value={kindFilter}
                    onChange={(event) => {
                      const matched = KIND_FILTERS.find(
                        (entry) => entry.value === event.target.value,
                      );
                      if (matched !== undefined) {
                        setKindFilter(matched.value);
                      }
                    }}
                  >
                    {KIND_FILTERS.map((entry) => (
                      <option key={entry.value} value={entry.value}>
                        {entry.label}
                      </option>
                    ))}
                  </select>
                </label>

                <span className="mono-chip">
                  {formatNumber(filteredJobs.length)} / {formatNumber(jobs.length)}
                </span>
              </span>
            </div>

            {loading ? (
              <div className="panel-pad">
                <EmptyState tone="loading" compact title="Lettura della coda…" />
              </div>
            ) : jobs.length === 0 ? (
              <div className="panel-pad">
                <EmptyState
                  compact
                  title="Coda vuota"
                  description="Nessun job registrato per questo progetto: avvia l'ingestione o la traduzione."
                  actionLabel="Vai alla traduzione"
                  onAction={() => {
                    onNavigate("translate");
                  }}
                />
              </div>
            ) : filteredJobs.length === 0 ? (
              <div className="panel-pad">
                <EmptyState
                  compact
                  title="Nessun job con questo filtro"
                  actionLabel="Azzera i filtri"
                  onAction={() => {
                    setStateFilter("all");
                    setKindFilter("all");
                  }}
                />
              </div>
            ) : (
              <div className="table-scroll">
                <table className="data-table">
                  <thead>
                    <tr>
                      <th style={{ width: "7rem" }}>Job</th>
                      <th style={{ minWidth: "10rem" }}>Tipo</th>
                      <th style={{ width: "9rem" }}>Stato</th>
                      <th style={{ width: "4.5rem" }} className="num">
                        Prio
                      </th>
                      <th style={{ width: "5.5rem" }} className="num">
                        Tent.
                      </th>
                      <th style={{ width: "9rem" }}>Creato</th>
                      <th style={{ width: "9rem" }}>Concluso</th>
                      <th style={{ minWidth: "12rem" }}>Ultimo errore</th>
                    </tr>
                  </thead>
                  <tbody>
                    {filteredJobs.map((job) => (
                      <tr key={job.id}>
                        <td className="num" title={job.id}>
                          {shortId(job.id, 8)}
                        </td>
                        <td className="text-ink-soft">{KIND_LABELS[job.kind]}</td>
                        <td>
                          <StatusBadge
                            status={job.state}
                            pulse={job.state === "running" || job.state === "leased"}
                          />
                        </td>
                        <td className="num">{formatNumber(job.priority)}</td>
                        <td className="num">
                          {formatNumber(job.attempts)} / {formatNumber(job.max_attempts)}
                        </td>
                        <td className="text-[0.72rem] text-muted">{formatDateTime(job.created_at)}</td>
                        <td className="text-[0.72rem] text-muted">
                          {job.finished_at === null ? "—" : formatDateTime(job.finished_at)}
                        </td>
                        <td>
                          {job.last_error === null ? (
                            <span className="text-faint">—</span>
                          ) : (
                            <span className="text-danger" title={job.last_error}>
                              {truncate(job.last_error, 70)}
                            </span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>

          {jobs.length >= JOB_LIMIT ? (
            <p className="text-[0.72rem] text-faint">
              Sono mostrati i primi {formatNumber(JOB_LIMIT)} job del progetto: i totali possono
              essere incompleti.
            </p>
          ) : null}
        </div>

        <div className="section-stack">
          <ResourceGauge resources={metrics?.resources ?? null} loading={metrics === null} compact />

          {metricsError !== null ? (
            <div className="banner banner-error" role="alert">
              <span aria-hidden="true">⚠</span>
              <span>{metricsError}</span>
            </div>
          ) : null}

          <LogView projectId={projectId} limit={600} title="Log di esecuzione" heightClass="h-80" />

          <p className="text-[0.72rem] text-faint">
            Ultimo campione di risorse:{" "}
            {metrics === null ? "—" : formatRelative(metrics.updated_at, now)}.
            {activeJobs > 0 ? " L'ETA si aggiorna ogni secondo." : ""}
          </p>
        </div>
      </div>
    </div>
  );
}
