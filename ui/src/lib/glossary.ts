/**
 * Project-glossary helpers (PLAN.md §9.2).
 *
 * Kept out of the view so the ordering and the filtering can be tested without a DOM: a term
 * that waits for a decision has to come first, whatever order the backend returned, and the
 * order has to be stable between runs and machines.
 */

import type { GlossaryTerm } from "./types";

/**
 * Statuses in the order they need attention: a candidate waits for a decision, a conflict for
 * arbitration, an approved term is settled and a rejected one is kept as a record only.
 */
export const GLOSSARY_STATUS_ORDER: readonly string[] = [
  "candidate",
  "conflict",
  "approved",
  "rejected",
];

export const GLOSSARY_KINDS: readonly { value: string; label: string }[] = [
  { value: "term", label: "Termine" },
  { value: "proper_noun", label: "Nome proprio" },
  { value: "do_not_translate", label: "Non tradurre" },
];

/** What the ordering and the filtering need of a row (a persisted term or an unsaved draft). */
export interface GlossaryFilterable {
  source: string;
  target: string;
  note: string | null;
  status: string;
}

/** A term of kind `do_not_translate` keeps the source text as its rendering. */
export function needsTarget(kind: string): boolean {
  return kind !== "do_not_translate";
}

function statusRank(status: string): number {
  const index = GLOSSARY_STATUS_ORDER.indexOf(status);
  // An unknown status is never dropped, it just sorts after the known ones.
  return index === -1 ? GLOSSARY_STATUS_ORDER.length : index;
}

/**
 * Case-folded code-point order. `localeCompare` would order the same list differently on another
 * machine; equal keys compare equal, so the sort keeps the backend order.
 */
function compareText(left: string, right: string): number {
  const foldedLeft = left.toLowerCase();
  const foldedRight = right.toLowerCase();
  if (foldedLeft === foldedRight) {
    return 0;
  }
  return foldedLeft < foldedRight ? -1 : 1;
}

/** The load order of the table: urgency first, then source. */
export function compareGlossaryRows(left: GlossaryFilterable, right: GlossaryFilterable): number {
  const rank = statusRank(left.status) - statusRank(right.status);
  return rank !== 0 ? rank : compareText(left.source, right.source);
}

/** The rows in load order. */
export function glossaryOrder<T extends GlossaryFilterable>(rows: readonly T[]): T[] {
  return [...rows].sort(compareGlossaryRows);
}

/** Does any of the fields contain the needle? A blank needle matches everything. */
export function matchesNeedle(fields: readonly string[], query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") {
    return true;
  }
  return fields.some((field) => field.toLowerCase().includes(needle));
}

/** Does the row pass the status filter (`""` for every status) and the query? */
export function glossaryMatches(row: GlossaryFilterable, status: string, query: string): boolean {
  if (status !== "" && row.status !== status) {
    return false;
  }
  return matchesNeedle([row.source, row.target, row.note ?? ""], query);
}

/** Terms per status, for the header counters. An unknown status counts under its own name. */
export function glossaryCounts(terms: readonly GlossaryTerm[]): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const term of terms) {
    counts[term.status] = (counts[term.status] ?? 0) + 1;
  }
  return counts;
}

/** How many terms still wait for a decision: candidates and conflicts. */
export function pendingDecisionCount(terms: readonly GlossaryTerm[]): number {
  return terms.filter((term) => term.status === "candidate" || term.status === "conflict")
    .length;
}
