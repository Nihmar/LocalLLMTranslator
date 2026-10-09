/**
 * Review inbox helpers (`PLAN.md` §11.4): ordering, the proposal applied to its paragraph and
 * the text currently in effect, pure so they are unit-tested.
 */

import type { Suggestion } from "./types.ts";

/** Badge classes for a suggestion severity; a missing severity reads as a plain note. */
export function severityClass(severity: string | null): string {
  switch (severity) {
    case "critical":
      return "badge badge-danger";
    case "major":
      return "badge badge-warning";
    case "minor":
      return "badge badge-info";
    default:
      return "badge badge-neutral";
  }
}

/** `critical` and `major`: what the inbox shows by default. */
export function isImportant(suggestion: Suggestion): boolean {
  return suggestion.severity === "critical" || suggestion.severity === "major";
}

const SEVERITY_RANK: Record<string, number> = { critical: 0, major: 1, minor: 2 };

/**
 * Inbox order: the most serious first, then the reading order of the book (`chunkOrder` maps a
 * chunk id to its position), then creation time.
 */
export function inboxOrder(
  suggestions: readonly Suggestion[],
  chunkOrder: ReadonlyMap<string, number>,
): Suggestion[] {
  const rank = (suggestion: Suggestion): number => SEVERITY_RANK[suggestion.severity ?? ""] ?? 3;
  return [...suggestions].sort(
    (a, b) =>
      rank(a) - rank(b) ||
      (chunkOrder.get(a.chunk_id) ?? 0) - (chunkOrder.get(b.chunk_id) ?? 0) ||
      a.created_at.localeCompare(b.created_at),
  );
}

/**
 * Mirrors `pipeline::review::accept_suggestion` for every pass: the quote is replaced once when
 * the current text contains it, otherwise the proposal is the corrected block.
 */
export function applyProposal(current: string, suggestion: Suggestion): string {
  const proposed = suggestion.proposed ?? "";
  const quote = suggestion.quote ?? "";
  if (quote.length > 0 && current.includes(quote)) {
    return current.replace(quote, proposed);
  }
  return proposed.trim().length > 0 ? proposed : current;
}

export interface DiffSegment {
  text: string;
  kind: "same" | "removed" | "added";
}

/**
 * The correction inside its paragraph: the text around the quote unchanged, the quote struck,
 * the proposal inserted. Without a usable quote the whole block is replaced.
 */
export function proposalSegments(current: string, suggestion: Suggestion): DiffSegment[] {
  const proposed = suggestion.proposed ?? "";
  const quote = suggestion.quote ?? "";
  const at = quote.length > 0 ? current.indexOf(quote) : -1;
  if (at < 0) {
    return [
      { text: current, kind: "removed" },
      { text: proposed, kind: "added" },
    ].filter((segment) => segment.text.length > 0) as DiffSegment[];
  }
  const segments: DiffSegment[] = [
    { text: current.slice(0, at), kind: "same" },
    { text: quote, kind: "removed" },
    { text: proposed, kind: "added" },
    { text: current.slice(at + quote.length), kind: "same" },
  ];
  return segments.filter((segment) => segment.text.length > 0);
}

/** Origin precedence when two translations of a block share a timestamp. */
function originRank(origin: string): number {
  switch (origin) {
    case "user":
      return 3;
    case "proofreader":
      return 2;
    case "editor":
      return 1;
    default:
      return 0;
  }
}

/** The text currently in effect per block: newest wins, pass order breaks ties. */
export function currentTextByBlock(
  translations: readonly { block_id: string; text_md: string; origin: string; updated_at: string }[],
): Map<string, string> {
  const best = new Map<string, { text: string; updatedAt: string; rank: number }>();
  for (const row of translations) {
    if (row.text_md.trim().length === 0) {
      continue;
    }
    const candidate = { text: row.text_md, updatedAt: row.updated_at, rank: originRank(row.origin) };
    const current = best.get(row.block_id);
    if (
      current === undefined ||
      candidate.updatedAt > current.updatedAt ||
      (candidate.updatedAt === current.updatedAt && candidate.rank > current.rank)
    ) {
      best.set(row.block_id, candidate);
    }
  }
  return new Map([...best].map(([blockId, value]) => [blockId, value.text]));
}
