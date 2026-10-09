import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  chunkList,
  jobCancel,
  jobList,
  projectGet,
  projectList,
  toErrorMessage,
} from "../lib/ipc";
import { onJobProgress } from "../lib/events";
import { countLabel, formatDuration, formatNumber, shortId, truncate } from "../lib/format";
import { useTicker } from "../lib/hooks";
import { isCancellableJobState, jobKindLabel, jobRows, sortForMonitor } from "../lib/jobs";
import type { Chapter, Chunk, JobView } from "../lib/types";
import { Dialog } from "./Dialog";
import { EmptyState } from "./EmptyState";
import { StatusBadge } from "./StatusBadge";

/**
 * Job monitor: what the LLM workers are doing, and the switch to stop one of them.
 *
 * It is a dialog rather than a page section because the question it answers — "what is running,
 * and can I stop it?" — is asked from several places: the translation page, a series
 * reconnaissance, an export. It takes the open project as the default scope and can widen to every
 * project, so the same component serves a view that only knows its own book.
 *
 * Interrupting does **not** stop the queue: `job_cancel` abandons the in-flight call of the jobs
 * the user picks and leaves the rest running. A cancelled `translate_chunk` returns its chunk to
 * `pending`, so it can be retried later; the hint under the table says so instead of leaving the
 * user to guess what "interrompi" did to the book.
 *
 * `job://progress` is the only source of change: the list is refetched (debounced, because a
 * parallel run emits one event per transition) instead of patched from the single row it carries.
 */

export interface JobMonitorProps {
  /** Project whose jobs are shown by default; `null` opens scoped to every project. */
  projectId: string | null;
  onClose: () => void;
}

/** How long a burst of `job://progress` events is collapsed before refetching. */
const RELOAD_DEBOUNCE_MS = 300;

const JOB_LIMIT = 500;

const STATE_FILTERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "incomplete", label: "Da fare e in corso" },
  { value: "failed", label: "Falliti" },
  { value: "all", label: "Tutti gli stati" },
];

function matchesStateFilter(state: string, filter: string): boolean {
  if (filter === "all") {
    return true;
  }
  if (filter === "failed") {
    return state === "failed";
  }
  return isCancellableJobState(state);
}

export function JobMonitor({ projectId, onClose }: JobMonitorProps) {
  const [jobs, setJobs] = useState<JobView[]>([]);
  const [chunks, setChunks] = useState<Chunk[]>([]);
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [projectNames, setProjectNames] = useState<ReadonlyMap<string, string>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [scope, setScope] = useState<"project" | "all">(projectId === null ? "all" : "project");
  const [stateFilter, setStateFilter] = useState("incomplete");
  const [kindFilter, setKindFilter] = useState("all");
  const [selection, setSelection] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);

  const scopedProjectId = scope === "project" ? projectId : null;

  const loadJobs = useCallback(async () => {
    setError(null);
    try {
      const rows = await jobList({
        project_id: scopedProjectId,
        state: null,
        limit: JOB_LIMIT,
      });
      setJobs(rows);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setJobs([]);
    } finally {
      setLoading(false);
    }
  }, [scopedProjectId]);

  // Names for the rows: the open project's chunks and chapters, or every project's name when the
  // scope is global. A failure is not fatal — the monitor falls back to ids and job kinds.
  const loadContext = useCallback(async () => {
    try {
      if (scopedProjectId === null) {
        setChunks([]);
        setChapters([]);
        const projects = await projectList();
        setProjectNames(new Map(projects.map((project) => [project.id, project.name])));
        return;
      }
      const [detail, rows] = await Promise.all([
        projectGet(scopedProjectId),
        chunkList({ project_id: scopedProjectId, status: null }),
      ]);
      setChapters(detail.chapters);
      setChunks(rows);
    } catch {
      setChunks([]);
      setChapters([]);
    }
  }, [scopedProjectId]);

  useEffect(() => {
    setLoading(true);
    void loadJobs();
    void loadContext();
  }, [loadJobs, loadContext]);

  // Bursts of events during a parallel run would otherwise refetch the whole queue per chunk.
  const reloadTimer = useRef<number | null>(null);
  useEffect(() => {
    return onJobProgress(() => {
      if (reloadTimer.current !== null) {
        window.clearTimeout(reloadTimer.current);
      }
      reloadTimer.current = window.setTimeout(() => {
        reloadTimer.current = null;
        void loadJobs();
      }, RELOAD_DEBOUNCE_MS);
    });
  }, [loadJobs]);

  useEffect(() => {
    return () => {
      if (reloadTimer.current !== null) {
        window.clearTimeout(reloadTimer.current);
      }
    };
  }, []);

  const chapterTitles = useMemo(() => {
    const map = new Map<string, string>();
    for (const chapter of chapters) {
      map.set(chapter.id, chapter.title);
    }
    return map;
  }, [chapters]);

  const running = jobs.some((job) => job.state === "running" || job.state === "leased");
  const now = useTicker(running);

  const rows = useMemo(() => {
    const all = jobRows(jobs, chunks, chapterTitles, now);
    return sortForMonitor(
      all.filter(
        (row) =>
          matchesStateFilter(row.job.state, stateFilter) &&
          (kindFilter === "all" || row.job.kind === kindFilter),
      ),
    );
  }, [jobs, chunks, chapterTitles, now, stateFilter, kindFilter]);

  const kinds = useMemo(() => {
    const seen = new Set<string>();
    for (const job of jobs) {
      seen.add(job.kind);
    }
    return [...seen]
      .sort()
      .map((kind) => ({ value: kind, label: jobKindLabel(kind) }));
  }, [jobs]);

  const cancellableRows = rows.filter((row) => row.cancellable);
  const selected = rows.filter((row) => selection.has(row.job.id) && row.cancellable);
  const allVisibleSelected = cancellableRows.length > 0 && selected.length === cancellableRows.length;

  async function interrupt(ids: readonly string[]) {
    if (ids.length === 0) {
      return;
    }
    setBusy(true);
    setNotice(null);
    setError(null);
    try {
      const result = await jobCancel(ids);
      const parts: string[] = [];
      if (result.cancelled.length > 0) {
        parts.push(countLabel(result.cancelled.length, "job interrotto", "job interrotti"));
      }
      if (result.skipped.length > 0) {
        parts.push(`${formatNumber(result.skipped.length)} già conclusi`);
      }
      setNotice(`${parts.length > 0 ? parts.join(", ") : "Nessun job da interrompere"}.`);
      setSelection(new Set());
      await loadJobs();
    } catch (cancelError) {
      setError(toErrorMessage(cancelError));
    } finally {
      setBusy(false);
    }
  }

  function toggle(id: string, checked: boolean) {
    setSelection((current) => {
      const next = new Set(current);
      if (checked) {
        next.add(id);
      } else {
        next.delete(id);
      }
      return next;
    });
  }

  return (
    <Dialog
      title="Lavori in corso"
      size={expanded ? "full" : "wide"}
      onClose={onClose}
      headerActions={
        <>
          <button
            type="button"
            className="btn btn-sm"
            disabled={loading}
            onClick={() => {
              void loadJobs();
              void loadContext();
            }}
          >
            Aggiorna
          </button>
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => {
              setExpanded((current) => !current);
            }}
            title={expanded ? "Torna alla finestra ridotta" : "Ingrandisci a tutta finestra"}
          >
            {expanded ? "Riduci" : "Ingrandisci"}
          </button>
        </>
      }
    >
      <div className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col">
          <span className="field-label">Ambito</span>
          <select
            className="select"
            style={{ width: "auto" }}
            value={scope}
            onChange={(event) => {
              setScope(event.target.value === "all" ? "all" : "project");
              setSelection(new Set());
            }}
          >
            {projectId !== null ? <option value="project">Progetto aperto</option> : null}
            <option value="all">Tutti i progetti</option>
          </select>
        </label>

        <label className="flex flex-col">
          <span className="field-label">Stato</span>
          <select
            className="select"
            style={{ width: "auto" }}
            value={stateFilter}
            onChange={(event) => {
              setStateFilter(event.target.value);
              setSelection(new Set());
            }}
          >
            {STATE_FILTERS.map((filter) => (
              <option key={filter.value} value={filter.value}>
                {filter.label}
              </option>
            ))}
          </select>
        </label>

        <label className="flex flex-col">
          <span className="field-label">Tipo</span>
          <select
            className="select"
            style={{ width: "auto" }}
            value={kindFilter}
            onChange={(event) => {
              setKindFilter(event.target.value);
              setSelection(new Set());
            }}
          >
            <option value="all">Tutti i tipi</option>
            {kinds.map((kind) => (
              <option key={kind.value} value={kind.value}>
                {kind.label}
              </option>
            ))}
          </select>
        </label>

        <span className="field-hint mb-1">
          {countLabel(rows.length, "riga", "righe")} · {formatNumber(cancellableRows.length)} da
          interrompere
        </span>

        <span className="ml-auto flex flex-wrap items-center gap-2">
          <button
            type="button"
            className="btn btn-sm"
            disabled={busy || selected.length === 0}
            onClick={() => {
              void interrupt(selected.map((row) => row.job.id));
            }}
          >
            {busy ? <span className="spinner" aria-hidden="true" /> : null}
            Interrompi selezionati ({formatNumber(selected.length)})
          </button>
          <button
            type="button"
            className="btn btn-sm btn-danger"
            disabled={busy || cancellableRows.length === 0}
            onClick={() => {
              void interrupt(cancellableRows.map((row) => row.job.id));
            }}
            title="Interrompe ogni lavoro che non è ancora concluso; la coda resta attiva"
          >
            Interrompi tutti ({formatNumber(cancellableRows.length)})
          </button>
        </span>
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

      <p className="field-hint">
        Interrompere non ferma la coda: gli altri lavori proseguono. Un chunk di traduzione
        interrotto torna «in attesa» e verrà ripreso al prossimo «Avvia / Riprendi».
      </p>

      {loading ? (
        <EmptyState tone="loading" compact title="Lettura della coda…" />
      ) : rows.length === 0 ? (
        <EmptyState
          compact
          title="Nessun lavoro da mostrare"
          description={
            stateFilter === "incomplete"
              ? "Non c'è nulla in attesa o in esecuzione con questi filtri."
              : "Cambia stato o tipo per vedere gli altri job."
          }
        />
      ) : (
        <div className="table-scroll" style={{ maxHeight: expanded ? "none" : "26rem" }}>
          <table className="data-table">
            <thead>
              <tr>
                <th style={{ width: "2.2rem" }}>
                  <input
                    type="checkbox"
                    checked={allVisibleSelected}
                    aria-label="Seleziona i lavori interrompibili"
                    onChange={(event) => {
                      setSelection(
                        event.target.checked
                          ? new Set(cancellableRows.map((row) => row.job.id))
                          : new Set(),
                      );
                    }}
                  />
                </th>
                <th style={{ width: "10rem" }}>Tipo</th>
                <th>Chunk / capitolo</th>
                {scope === "all" ? <th style={{ width: "10rem" }}>Progetto</th> : null}
                <th style={{ width: "8rem" }}>Stato</th>
                <th style={{ width: "6.5rem" }} className="num">
                  Durata
                </th>
                <th style={{ width: "5rem" }} className="num">
                  Tent.
                </th>
                <th style={{ width: "3.5rem" }} className="num">
                  Prio
                </th>
                <th>Ultimo errore</th>
                <th style={{ width: "6.5rem" }} />
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.job.id} data-selected={selection.has(row.job.id)}>
                  <td>
                    <input
                      type="checkbox"
                      checked={selection.has(row.job.id)}
                      disabled={!row.cancellable}
                      aria-label={`Seleziona il job ${shortId(row.job.id, 8)}`}
                      onChange={(event) => {
                        toggle(row.job.id, event.target.checked);
                      }}
                    />
                  </td>
                  <td className="text-ink-soft">{row.kind_label}</td>
                  <td>
                    {row.chunk_id === null ? (
                      <span className="text-faint">—</span>
                    ) : (
                      <>
                        <span className="mono-chip">{shortId(row.chunk_id, 10)}</span>
                        {row.chapter_title !== null ? (
                          <span className="ml-2 text-ink-soft" title={row.chapter_title}>
                            {truncate(row.chapter_title, 60)}
                          </span>
                        ) : null}
                      </>
                    )}
                  </td>
                  {scope === "all" ? (
                    <td className="text-ink-soft" title={row.job.project_id}>
                      {projectNames.get(row.job.project_id) ?? shortId(row.job.project_id, 8)}
                    </td>
                  ) : null}
                  <td>
                    <StatusBadge
                      status={row.job.state}
                      pulse={row.job.state === "running" || row.job.state === "leased"}
                    />
                  </td>
                  <td className="num text-muted tabular-nums">
                    {row.elapsed_ms === null ? "—" : formatDuration(row.elapsed_ms)}
                  </td>
                  <td className="num">
                    {formatNumber(row.job.attempts)} / {formatNumber(row.job.max_attempts)}
                  </td>
                  <td className="num">{formatNumber(row.job.priority)}</td>
                  <td>
                    {row.job.last_error === null ? (
                      <span className="text-faint">—</span>
                    ) : (
                      <span className="text-danger" title={row.job.last_error}>
                        {truncate(row.job.last_error, 70)}
                      </span>
                    )}
                  </td>
                  <td className="text-right">
                    <button
                      type="button"
                      className="btn btn-sm"
                      disabled={busy || !row.cancellable}
                      onClick={() => {
                        void interrupt([row.job.id]);
                      }}
                      title={
                        row.cancellable
                          ? "Interrompe questo lavoro; la coda resta attiva"
                          : "Il lavoro è già concluso"
                      }
                    >
                      Interrompi
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {jobs.length >= JOB_LIMIT ? (
        <p className="field-hint">
          Mostrati i primi {formatNumber(JOB_LIMIT)} job del progetto: restringi i filtri per
          vederne altri.
        </p>
      ) : null}
    </Dialog>
  );
}
