/** Pieces shared by the review page panels (`PLAN.md` §11.4). */

export type Tab = "open" | "decided" | "qa";
export type SeverityFilter = "important" | "all" | "minor";
export type PassFilter = "all" | "editor" | "proofreader";

export const PASSES: ReadonlyArray<{ value: string; label: string }> = [
  { value: "both", label: "Editor + proofreader" },
  { value: "editor", label: "Solo editor" },
  { value: "proofreader", label: "Solo proofreader" },
];

export const SEVERITY_LABEL: Record<string, string> = {
  critical: "Critica",
  major: "Importante",
  minor: "Minore",
};

export const QA_KIND_LABEL: Record<string, string> = {
  untranslated: "Non tradotto",
  glossary_mismatch: "Glossario non rispettato",
  glossary_conflict: "Conflitto di glossario",
  placeholder_broken: "Segnaposto rotti",
  markdown_malformed: "Struttura alterata",
  length_anomaly: "Lunghezza anomala",
  duplicate: "Duplicato",
  empty: "Vuoto",
  latin_leftover: "Testo non tradotto rimasto",
};

export function passLabel(pass: string): string {
  return pass === "proofreader" ? "proofreader" : "editor";
}

export function isTypingTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement
  );
}
