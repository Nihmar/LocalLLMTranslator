/** Pieces shared by the translation page panels (`PLAN.md` §11.3). */

import type { Chunk } from "../../lib/types";

export const STATUS_FILTERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "all", label: "Tutti gli stati" },
  { value: "pending", label: "In attesa" },
  { value: "running", label: "In corso" },
  { value: "done", label: "Completati" },
  { value: "failed", label: "Falliti" },
  { value: "needs_review", label: "Da rivedere" },
];

export const PAGE_SIZE = 200;

export interface Counts {
  pending: number;
  running: number;
  done: number;
  failed: number;
  needs_review: number;
}

export function countStatuses(chunks: readonly Chunk[]): Counts {
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
export function parseStringArray(json: string): string[] {
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
export function parseChunkId(payloadJson: string): string | null {
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

export function blockOriginLabel(origin: string): string {
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
