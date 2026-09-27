import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { LogView } from "../components/LogView";
import { ProgressBar } from "../components/ProgressBar";
import { ResourceGauge } from "../components/ResourceGauge";
import { StatusBadge } from "../components/StatusBadge";
import { onJobProgress, onMetricsTick } from "../lib/events";
import { useTicker } from "../lib/hooks";
import { KIND_LABELS, jobKindLabel, payloadChunkId } from "../lib/jobs";
import {
  countLabel,
  estimateRemainingMs,
  formatDateTime,
  formatDuration,
  formatEta,
  formatNumber,
  formatThroughput,
  parentDirectory,
  shortId,
  truncate,
} from "../lib/format";
import { diagnosticsExport, diagnosticsPaths, jobList, metricsGet, openPath, toErrorMessage } from "../lib/ipc";
import type { DiagnosticsPaths, Job, Metrics, Project } from "../lib/types";
import type { ViewId } from "../App";

/**
 * Job dashboard: queue progress, ETA, resources and the live log (`PLAN.md` §11, "Dashboard job").
 *
 * The ETA is never a constant. It is derived from observed throughput only: distinct
 * `translate_chunk` chunks that this page has watched reach a terminal state, over the
 * wall-clock time since the first `job://progress` event. When nothing has completed yet, the
 * page says the estimate is not available instead of inventing a number.
 *
 * `job://progress` carries one job row, so it is still treated as an invalidation trigger: the
 * queue is refetched through `job_list` rather than patched from a single transition.
 */

export interface JobsViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

const JOB_LIMIT = 1000;
const TERMINAL_STATES: ReadonlySet<string> = new Set(["done", "failed", "cancelled"]);

const STATE_FILTERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "all", label: "Tutti gli stati" },
  { value: "pending", label: "In attesa" },
  { value: "leased", label: "Assegnati" },
  { value: "running", label: "In corso" },
  { value: "done", label: "Completati" },
  { value: "failed", label: "Falliti" },
  { value: "cancelled", label: "Annullati" },
];

const KIND_FILTERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "all", label: "Tutti i tipi" },
  ...Object.entries(KIND_LABELS).map(([value, label]) => ({ value, label })),
];

interface EtaView {
  etaMs: number | null;
  /** Italian sentence explaining where the estimate comes from. */
  source: string;
}

export function JobsView({ project, onNavigate }: JobsViewProps) {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [stateFilter, setStateFilter] = useState("all");
  const [kindFilter, setKindFilter] = useState("all");
  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [metricsError, setMetricsError] = useState<string | null>(null);
  // Diagnostics: where the logs are and the last exported bundle.
  const [diagPaths, setDiagPaths] = useState<DiagnosticsPaths | null>(null);
  const [diagExporting, setDiagExporting] = useState(false);
  const [diagNotice, setDiagNotice] = useState<string | null>(null);
  const [diagError, setDiagError] = useState<string | null>(null);
  const [exportedBundle, setExportedBundle] = useState<string | null>(null);

  // Observed-throughput fallback: distinct chunks seen finishing since the first event.
  const [sessionFinished, setSessionFinished] = useState(0);
  const [sessionStartedAt, setSessionStartedAt] = useState<number | null>(null);
  const finishedIds = useRef<Set<string>>(new Set());

  const projectId = project?.id ?? null;

  useEffect(() => {
    void diagnosticsPaths()
      .then(setDiagPaths)
      .catch(() => {
        // The folder is only a convenience: without it the export still works.
        setDiagPaths(null);
      });
  }, []);

  async function handleExportDiagnostics(): Promise<void> {
    setDiagExporting(true);
    setDiagError(null);
    setDiagNotice(null);
    try {
      const outcome = await diagnosticsExport();
      setExportedBundle(outcome.output_path);
      setDiagNotice(
        `Diagnostica esportata (${formatNumber(outcome.files)} file): ${outcome.output_path}`,
      );
    } catch (exportError) {
      setDiagError(toErrorMessage(exportError));
    } finally {
      setDiagExporting(false);
    }
  }

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
      setMetrics(await metricsGet());
    } catch (loadError) {
      setMetricsError(toErrorMessage(loadError));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    void loadMetrics();
  }, [loadMetrics]);

  useEffect(() => onMetricsTick(() => {
    void loadMetrics();
  }), [loadMetrics]);

  // Live queue state: the event is a trigger, so the queue is refetched rather than patched.
  useEffect(
    () =>
      onJobProgress((event) => {
        setSessionStartedAt((current) => current ?? Date.now());

        const chunkId = payloadChunkId(event.payload_json);
        if (
          event.kind === "translate_chunk" &&
          chunkId !== null &&
          TERMINAL_STATES.has(event.state) &&
          !finishedIds.current.has(chunkId)
        ) {
          finishedIds.current.add(chunkId);
          setSessionFinished(finishedIds.current.size);
        }

        void load();
        void loadMetrics();
      }),
    [load, loadMetrics],
  );

  // The observation window belongs to one project only.
  useEffect(() => {
    finishedIds.current = new Set();
    setSessionFinished(0);
    setSessionStartedAt(null);
  }, [projectId]);

  const counts = useMemo(() => {
    const result: Record<string, number> = {};
    for (const job of jobs) {
      result[job.state] = (result[job.state] ?? 0) + 1;
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

  const activeJobs =
    (counts.pending ?? 0) + (counts.leased ?? 0) + (counts.running ?? 0);
  const now = useTicker(activeJobs > 0);

  const sessionElapsedMs = sessionStartedAt === null ? 0 : Math.max(0, now - sessionStartedAt);

  const eta = useMemo((): EtaView => {
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
  }, [sessionFinished, sessionElapsedMs, translateTotal]);

  const observedThroughput =
    sessionFinished > 0 ? formatThroughput(sessionFinished, sessionElapsedMs) : "—";
  const observedElapsed = sessionElapsedMs > 0 ? formatDuration(sessionElapsedMs) : "—";

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
                <div className="stat-value">{formatNumber(counts.pending ?? 0)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">In corso</div>
                <div className="stat-value">
                  {formatNumber((counts.running ?? 0) + (counts.leased ?? 0))}
                </div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Completati</div>
                <div className="stat-value">{formatNumber(counts.done ?? 0)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Falliti</div>
                <div className="stat-value">{formatNumber(counts.failed ?? 0)}</div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Annullati</div>
                <div className="stat-value">{formatNumber(counts.cancelled ?? 0)}</div>
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
                <div className="stat-label">Parallelismo</div>
                <div className="stat-value" style={{ fontSize: "0.85rem" }}>
                  {metrics === null ? "—" : formatNumber(metrics.suggested_parallel)}
                </div>
              </div>
            </div>

            <p className="text-[0.72rem] text-faint">{eta.source}</p>
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
                      setStateFilter(event.target.value);
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
                      setKindFilter(event.target.value);
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
                        <td className="text-ink-soft">{jobKindLabel(job.kind)}</td>
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
          <ResourceGauge metrics={metrics} loading={metrics === null} compact />

          {metricsError !== null ? (
            <div className="banner banner-error" role="alert">
              <span aria-hidden="true">⚠</span>
              <span>{metricsError}</span>
            </div>
          ) : null}

          <LogView limit={600} title="Log di esecuzione" heightClass="h-80" />

          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Diagnostica</span>
            </div>
            <div className="panel-pad section-stack">
              <p className="field-hint">
                Ogni evento (job, chiamate al modello, stato del sidecar, errori mostrati
                nell&apos;interfaccia) finisce in un file di log giornaliero. Il bundle di
                diagnostica contiene i log più recenti e un report su versioni, coda e
                fallimenti: nessun testo del libro, prompt o database.
              </p>
              <div className="flex flex-wrap items-center gap-2">
                <button
                  type="button"
                  className="btn"
                  disabled={diagPaths === null}
                  onClick={() => {
                    if (diagPaths !== null) {
                      void openPath(diagPaths.log_dir);
                    }
                  }}
                >
                  Apri cartella log
                </button>
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={diagExporting}
                  onClick={() => void handleExportDiagnostics()}
                >
                  {diagExporting ? <span className="spinner" aria-hidden="true" /> : null}
                  Esporta diagnostica
                </button>
                {exportedBundle !== null ? (
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => void openPath(parentDirectory(exportedBundle))}
                  >
                    Apri cartella del bundle
                  </button>
                ) : null}
              </div>
              {diagPaths !== null ? (
                <p className="mono-chip" title={diagPaths.log_dir}>
                  {truncate(diagPaths.log_dir, 64)}
                </p>
              ) : null}
              {diagError !== null ? (
                <div className="banner banner-error" role="alert">
                  <span aria-hidden="true">⚠</span>
                  <span>{diagError}</span>
                </div>
              ) : null}
              {diagNotice !== null ? (
                <div className="banner banner-ok" role="status">
                  <span aria-hidden="true">✓</span>
                  <span>{diagNotice}</span>
                </div>
              ) : null}
            </div>
          </div>

          <p className="text-[0.72rem] text-faint">
            {activeJobs > 0 ? "L'ETA si aggiorna ogni secondo." : "Nessun job attivo."}
          </p>
        </div>
      </div>
    </div>
  );
}
