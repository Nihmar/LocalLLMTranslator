/**
 * Queue vocabulary shared by the job dashboard and the job monitor: the Italian label of every
 * job kind, the states in which a job occupies a worker or the queue, and the chunk a job works
 * on — which lives in `payload_json`, not on the job row.
 *
 * The monitor projects the rows against the chunks and chapters a project already has in memory,
 * so naming what runs costs no extra command.
 */

import type { Chunk, JobView, JsonValue } from "./types";

/** Italian label per job kind. Unknown kinds are shown verbatim, never hidden. */
export const KIND_LABELS: Readonly<Record<string, string>> = {
  ingest: "Ingestione",
  translate_chunk: "Traduzione chunk",
  summarize: "Riassunto",
  book_recon: "Ricognizione",
  edit_chunk: "Revisione editor",
  proofread_chunk: "Proofread",
  qa_scan: "Scansione QA",
  export_unit: "Export",
};

export function jobKindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind;
}

/** States in which a job occupies a worker: it has been claimed and has not finished yet. */
const ACTIVE_STATES: ReadonlySet<string> = new Set(["running", "leased"]);

export function isActiveJobState(state: string): boolean {
  return ACTIVE_STATES.has(state);
}

/** States a job can still be interrupted from; a finished job has nothing left to stop. */
const CANCELLABLE_STATES: ReadonlySet<string> = new Set(["pending", "leased", "running"]);

export function isCancellableJobState(state: string): boolean {
  return CANCELLABLE_STATES.has(state);
}

/** The chunk a job belongs to lives in its decoded payload, not on the job row. */
export function payloadChunkId(payload: JsonValue): string | null {
  if (typeof payload === "object" && payload !== null && !Array.isArray(payload)) {
    const value = payload["chunk_id"];
    return typeof value === "string" ? value : null;
  }
  return null;
}

/** Milliseconds since the claim, or `null` when the job never started or the stamp is unreadable. */
export function elapsedSince(startedAt: string | null, now: number): number | null {
  if (startedAt === null) {
    return null;
  }
  const started = Date.parse(startedAt);
  if (Number.isNaN(started)) {
    return null;
  }
  return Math.max(0, now - started);
}

/** One job as the monitor shows it, with its chunk and chapter resolved for display. */
export interface JobRow {
  job: JobView;
  /** Italian label of `job.kind`. */
  kind_label: string;
  chunk_id: string | null;
  /** Title of the chapter the chunk belongs to, when both resolved. */
  chapter_title: string | null;
  cancellable: boolean;
  /** How long the job ran, or has been running; `null` when it never started. */
  elapsed_ms: number | null;
}
/**
 * Project every job against the chunks and chapters of the open project.
 *
 * A job whose chunk cannot be resolved keeps a `null` chapter instead of being dropped: the
 * monitor exists to show work, including the odd row that cannot be named. `now` decides the
 * elapsed time of the jobs that are still running.
 */
export function jobRows(
  jobs: readonly JobView[],
  chunks: readonly Chunk[],
  chapterTitles: ReadonlyMap<string, string>,
  now: number,
): JobRow[] {
  const chapterByChunk = new Map<string, string>();
  for (const chunk of chunks) {
    if (chunk.chapter_id !== null) {
      chapterByChunk.set(chunk.id, chunk.chapter_id);
    }
  }

  return jobs.map((job) => {
    const chunkId = payloadChunkId(job.payload);
    const chapterId = chunkId === null ? null : (chapterByChunk.get(chunkId) ?? null);
    const finished = job.finished_at === null ? null : Date.parse(job.finished_at);
    const until = finished === null || Number.isNaN(finished) ? now : finished;
    return {
      job,
      kind_label: jobKindLabel(job.kind),
      chunk_id: chunkId,
      chapter_title: chapterId === null ? null : (chapterTitles.get(chapterId) ?? null),
      cancellable: isCancellableJobState(job.state),
      elapsed_ms: elapsedSince(job.started_at, until),
    };
  });
}

/** Rank of a state in the monitor: what is moving first, what already finished last. */
const STATE_RANK: Readonly<Record<string, number>> = {
  running: 0,
  leased: 1,
  pending: 2,
};

/**
 * Monitor order: running, assigned, queued, then everything already concluded.
 *
 * Inside a group the most recent activity wins, so the row that just changed is at the top of its
 * group and a long queue is read from the part that is actually moving.
 */
export function sortForMonitor(rows: readonly JobRow[]): JobRow[] {
  const rank = (row: JobRow): number => STATE_RANK[row.job.state] ?? 3;
  const activity = (row: JobRow): string =>
    row.job.started_at ?? row.job.finished_at ?? row.job.created_at;
  return [...rows].sort((left, right) => {
    const byState = rank(left) - rank(right);
    if (byState !== 0) {
      return byState;
    }
    const byActivity = activity(right).localeCompare(activity(left));
    if (byActivity !== 0) {
      return byActivity;
    }
    return left.job.id.localeCompare(right.job.id);
  });
}
