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
