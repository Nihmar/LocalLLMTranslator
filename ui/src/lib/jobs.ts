/**
 * Queue vocabulary shared by the job dashboard and the translation page: the Italian label of
 * every job kind, the states in which a job occupies a worker, and the chunk a job works on —
 * which lives in `payload_json`, not on the job row.
 *
 * The in-flight projection resolves those payloads against the chunk rows a page already holds,
 * so the "what is running now" list needs no extra command and cannot drift from the table.
 */

import type { Chunk, Job } from "./types";

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

/** The chunk a job belongs to lives in its payload, not on the job row. */
export function payloadChunkId(payloadJson: string): string | null {
  try {
    const parsed: unknown = JSON.parse(payloadJson);
    if (typeof parsed === "object" && parsed !== null) {
      const value = (parsed as { chunk_id?: unknown }).chunk_id;
      return typeof value === "string" ? value : null;
    }
  } catch {
    // A malformed payload must not break the progress trigger.
  }
  return null;
}

/** One in-flight job, with the chunk and chapter it belongs to resolved for display. */
export interface ActiveJobRow {
  id: string;
  kind: string;
  /** Italian label of `kind`. */
  kind_label: string;
  state: string;
  chunk_id: string | null;
  chapter_id: string | null;
  /** Chapter title, when the chunk resolved to a known chapter. */
  chapter_title: string | null;
  attempts: number;
  max_attempts: number;
  /** ISO timestamp of the claim, or `null` when the job has not started yet. */
  started_at: string | null;
  last_error: string | null;
}

/**
 * In-flight jobs, oldest claim first so the list does not reshuffle as new jobs start.
 *
 * The order is the point: a worker busy with a slow chunk stays where the reader last saw it,
 * and ties fall back to the id so jobs claimed in the same second keep a stable order.
 */
export function activeJobRows(
  jobs: readonly Job[],
  chunks: readonly Chunk[],
  chapterTitles: ReadonlyMap<string, string>,
): ActiveJobRow[] {
  const chapterByChunk = new Map<string, string>();
  for (const chunk of chunks) {
    if (chunk.chapter_id !== null) {
      chapterByChunk.set(chunk.id, chunk.chapter_id);
    }
  }

  return jobs
    .filter((job) => isActiveJobState(job.state))
    .map((job) => {
      const chunkId = payloadChunkId(job.payload_json);
      const chapterId = chunkId === null ? null : (chapterByChunk.get(chunkId) ?? null);
      return {
        id: job.id,
        kind: job.kind,
        kind_label: jobKindLabel(job.kind),
        state: job.state,
        chunk_id: chunkId,
        chapter_id: chapterId,
        chapter_title: chapterId === null ? null : (chapterTitles.get(chapterId) ?? null),
        attempts: job.attempts,
        max_attempts: job.max_attempts,
        started_at: job.started_at,
        last_error: job.last_error,
      };
    })
    .sort((left, right) => {
      const byStart = (left.started_at ?? "").localeCompare(right.started_at ?? "");
      return byStart !== 0 ? byStart : left.id.localeCompare(right.id);
    });
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
