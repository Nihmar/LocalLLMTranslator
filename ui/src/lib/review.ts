/**
 * Review presentation helpers (`PLAN.md` §11.4), shared by the review page and the
 * correction history.
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

/**
 * One line describing a proposal: the replaced fragment when the pass quoted one,
 * the rewritten block otherwise.
 */
export function suggestionSnippet(suggestion: Suggestion): string {
  const quote = suggestion.quote ?? "";
  const proposed = suggestion.proposed ?? "";
  if (quote.length > 0) {
    return `«${quote}» → «${proposed}»`;
  }
  if (proposed.length === 0) {
    return "proposta senza testo";
  }
  return suggestion.pass === "proofreader"
    ? "blocco riscritto dal proofreader"
    : `blocco riscritto: «${proposed}»`;
}
