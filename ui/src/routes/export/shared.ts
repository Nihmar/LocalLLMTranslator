/** Pieces shared by the export page panels (`PLAN.md` §11.5). */

import type { ExportFormat, ExportProgressEvent } from "../../lib/types";

/** Output formats offered by the build form. `html` is the browser-preview choice. */
export const FORMATS: ReadonlyArray<{
  value: ExportFormat | "html";
  label: string;
  extension: string;
  templateHint: string;
  cssHint: string;
  note: string;
}> = [
  {
    value: "pdf",
    label: "PDF",
    extension: "pdf",
    templateHint: "pandoc/templates/book.tex",
    cssHint: "",
    note: "Impaginazione LaTeX con indice, filtro note e suddivisione in capitoli.",
  },
  {
    value: "epub",
    label: "EPUB",
    extension: "epub",
    templateHint: "pandoc/templates/book.html",
    cssHint: "pandoc/styles/book.css",
    note: "Indice, note e immagini generati da Pandoc.",
  },
  {
    value: "html",
    label: "HTML",
    extension: "html",
    templateHint: "pandoc/templates/book.html",
    cssHint: "pandoc/styles/book.css",
    note: "Utile per l'anteprima impaginata nel browser.",
  },
  {
    value: "docx",
    label: "DOCX",
    extension: "docx",
    templateHint: "",
    cssHint: "",
    note: "Nessun template: Pandoc usa il documento di riferimento predefinito.",
  },
];

export function parentDirectory(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (index <= 0) {
    return trimmed;
  }
  return trimmed.slice(0, index);
}

export function looksAbsolute(path: string): boolean {
  return path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\");
}

/** Italian description of the last `export://progress` state. */
export function progressLabel(event: ExportProgressEvent): string {
  if (event.state === "started") {
    return `Build avviata${event.format === undefined ? "" : ` (${event.format.toUpperCase()})`}.`;
  }
  if (event.state === "done") {
    return `Build conclusa${event.output_path === undefined ? "" : `: ${event.output_path}`}.`;
  }
  return `Stato: ${event.state}.`;
}

/** `2026-10-09T12:00:00Z` → `2026-10-09 12:00:00`, as the history table shows it. */
export function timestampLabel(value: string): string {
  return value.replace("T", " ").slice(0, 19);
}
