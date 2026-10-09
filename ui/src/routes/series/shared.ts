/**
 * Pieces shared by the series panels (`PLAN.md` §9.5): labels, the parsers of the JSON the
 * findings and the candidate profile carry, and the `run` contract every panel acts through.
 */

import type { JsonValue, SeriesGlossaryTerm, SeriesProfile } from "../../lib/types";

/**
 * Runs one panel action: the parent marks it busy, clears the previous message and shows the
 * returned notice, or the error the action threw. Panels never own the page-level banners.
 */
export type Run = (action: string, work: () => Promise<string | null>) => Promise<void>;

/** What every panel of a selected series receives. */
export interface SeriesPanelProps {
  /** An action is running somewhere on the page: buttons stay disabled. */
  busy: boolean;
  run: Run;
  /** Shows a message without running an action (a cancelled picker, nothing selected). */
  notify: (message: string) => void;
}

export const KINDS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "term", label: "Termine" },
  { value: "proper_noun", label: "Nome proprio" },
  { value: "do_not_translate", label: "Non tradurre" },
];

export const STATUSES: ReadonlyArray<{ value: string; label: string }> = [
  { value: "approved", label: "Approvato" },
  { value: "candidate", label: "Candidato" },
  { value: "conflict", label: "Conflitto" },
  { value: "rejected", label: "Rifiutato" },
];

export function statusLabel(status: string): string {
  return STATUSES.find((entry) => entry.value === status)?.label ?? status;
}

export function kindLabel(kind: string): string {
  return KINDS.find((entry) => entry.value === kind)?.label ?? kind;
}

export interface TermDraft {
  target: string;
  kind: string;
  status: string;
  note: string;
}

export function draftFrom(term: SeriesGlossaryTerm): TermDraft {
  return {
    target: term.target,
    kind: term.kind,
    status: term.status,
    note: term.note ?? "",
  };
}

/**
 * The pieces of a `glossary_conflict` finding the view acts on. Both the series flagger
 * (`series_target`/`project_target`) and the project-level proposal merge
 * (`existing_target`/`proposed_target`) are understood.
 */
export interface ConflictDetails {
  source: string | null;
  scope: string | null;
  seriesTarget: string | null;
  projectTarget: string | null;
}

export function parseConflictDetails(details: JsonValue | null | undefined): ConflictDetails {
  let record: Record<string, JsonValue> = {};
  if (details !== null && details !== undefined && typeof details === "object" && !Array.isArray(details)) {
    record = details;
  }
  const text = (key: string): string | null => {
    const value = record[key];
    return typeof value === "string" && value.length > 0 ? value : null;
  };
  return {
    source: text("source"),
    scope: text("scope"),
    seriesTarget: text("series_target") ?? text("existing_target"),
    projectTarget: text("project_target") ?? text("proposed_target"),
  };
}

/**
 * Parse the stored `series_profile` JSON. The candidate may predate fields, so the parser
 * degrades to the parts it understands instead of failing the whole view.
 */
export function parseSeriesProfile(json: string): SeriesProfile | null {
  try {
    const value: unknown = JSON.parse(json);
    if (value === null || typeof value !== "object") {
      return null;
    }
    const record = value as Record<string, unknown>;
    const synopsis = typeof record["synopsis"] === "string" ? record["synopsis"] : "";
    const styleNotes = Array.isArray(record["style_notes"])
      ? record["style_notes"].filter((note): note is string => typeof note === "string")
      : [];
    const characters = Array.isArray(record["characters"])
      ? record["characters"].flatMap((entry) => {
          if (entry === null || typeof entry !== "object") {
            return [];
          }
          const raw = entry as Record<string, unknown>;
          const source = typeof raw["source"] === "string" ? raw["source"] : "";
          if (source.length === 0) {
            return [];
          }
          return [
            {
              source,
              target: typeof raw["target"] === "string" ? raw["target"] : "",
              note: typeof raw["note"] === "string" ? raw["note"] : "",
            },
          ];
        })
      : [];
    const rejected = Array.isArray(record["rejected"])
      ? record["rejected"].filter((source): source is string => typeof source === "string")
      : [];
    return {
      synopsis,
      style_notes: styleNotes,
      characters,
      rejected,
      // Older candidates predate the provenance field; an empty one renders as absent.
      provenance: {
        generated_at: "",
        model: "",
        prompt_hash: "",
        books: [],
        sources: [],
        glossary_hash: "",
      },
    };
  } catch {
    return null;
  }
}
