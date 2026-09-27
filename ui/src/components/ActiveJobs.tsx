import { useMemo } from "react";
import { countLabel, formatDuration, formatNumber, shortId, truncate } from "../lib/format";
import { useTicker } from "../lib/hooks";
import { activeJobRows, elapsedSince } from "../lib/jobs";
import type { Chunk, Job } from "../lib/types";
import { StatusBadge } from "./StatusBadge";

/**
 * What the workers are doing right now, on the translation page.
 *
 * The queue counters above say *how many* jobs are running; this list says *which*: the chunk,
 * its chapter, how long the claim has been held and whether the job is on its second attempt.
 * It reads the job rows the page already fetched (`job_list`, refreshed on every
 * `job://progress`) and resolves the payload against the loaded chunks, so it costs no extra
 * command; only the elapsed time ticks locally, and `useTicker` stops the clock when nothing
 * is in flight.
 */

export interface ActiveJobsProps {
  jobs: readonly Job[];
  chunks: readonly Chunk[];
  chapterTitles: ReadonlyMap<string, string>;
  /** True while the first read of the queue is still in progress. */
  loading?: boolean | undefined;
}

export function ActiveJobs({ jobs, chunks, chapterTitles, loading = false }: ActiveJobsProps) {
  const rows = useMemo(
    () => activeJobRows(jobs, chunks, chapterTitles),
    [jobs, chunks, chapterTitles],
  );
  const now = useTicker(rows.length > 0);

  const pending = jobs.filter((job) => job.state === "pending");
  const pendingTranslations = pending.filter((job) => job.kind === "translate_chunk").length;

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Job in corso</span>
        <span className="flex flex-wrap items-center justify-end gap-2">
          <span className="mono-chip">{countLabel(rows.length, "attivo", "attivi")}</span>
        </span>
      </div>

      <div className="panel-pad section-stack">
        {loading && jobs.length === 0 ? (
          <p className="flex items-center gap-2 text-xs text-muted">
            <span className="spinner" aria-hidden="true" />
            Lettura della coda…
          </p>
        ) : rows.length === 0 ? (
          <>
            <p className="text-xs text-muted">Nessun job in corso.</p>
            {pending.length > 0 ? (
              <p className="field-hint">
                {countLabel(pending.length, "job in attesa", "job in attesa")} in coda
                {pendingTranslations > 0
                  ? `, di cui ${formatNumber(pendingTranslations)} di traduzione`
                  : ""}
                . L&apos;elenco si popola quando l&apos;esecutore assegna un chunk.
              </p>
            ) : null}
          </>
        ) : (
          <ul className="space-y-2 overflow-auto" style={{ maxHeight: "15rem" }}>
            {rows.map((row) => {
              const elapsed = elapsedSince(row.started_at, now);
              return (
                <li
                  key={row.id}
                  className="rounded-md border border-line bg-canvas px-2.5 py-2"
                  title={row.id}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="mono-chip">{row.kind_label}</span>
                    <StatusBadge status={row.state} pulse />
                  </div>

                  <div
                    className="mt-1 truncate text-xs text-ink-soft"
                    title={row.chapter_title ?? undefined}
                  >
                    {row.chunk_id === null
                      ? "Nessun chunk associato"
                      : `chunk ${shortId(row.chunk_id, 8)}`}
                    {row.chapter_title !== null ? ` · ${row.chapter_title}` : ""}
                  </div>

                  <div className="mt-1 flex items-center justify-between gap-2 text-[0.68rem] text-faint">
                    <span>
                      {elapsed === null ? "in attesa di avvio" : `da ${formatDuration(elapsed)}`}
                    </span>
                    <span>
                      tentativo {formatNumber(row.attempts)}/{formatNumber(row.max_attempts)}
                    </span>
                  </div>

                  {row.last_error !== null ? (
                    <p className="mt-1 truncate text-[0.68rem] text-danger" title={row.last_error}>
                      {truncate(row.last_error, 90)}
                    </p>
                  ) : null}
                </li>
              );
            })}
          </ul>
        )}

        {rows.length > 0 && pending.length > 0 ? (
          <p className="field-hint">
            {countLabel(pending.length, "job in attesa", "job in attesa")} dopo questi
            {pendingTranslations > 0
              ? `, di cui ${formatNumber(pendingTranslations)} di traduzione`
              : ""}
            .
          </p>
        ) : null}
      </div>
    </div>
  );
}
