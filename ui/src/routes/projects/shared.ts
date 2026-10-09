/** Pieces shared by the library page panels (`PLAN.md` §11). */

import { basename, fileExtension } from "../../lib/format";
import type { SourceFormat } from "../../lib/types";

export interface FormState {
  name: string;
  source_path: string;
  source_format: SourceFormat;
  source_lang: string;
  target_lang: string;
}

export interface FormErrors {
  name?: string | undefined;
  source_path?: string | undefined;
  target_lang?: string | undefined;
}

/** Languages offered in the new-book form; any other ISO 639-1 code can still be typed. */
export const LANGUAGES: ReadonlyArray<{ code: string; name: string }> = [
  { code: "it", name: "Italiano" },
  { code: "en", name: "Inglese" },
  { code: "fr", name: "Francese" },
  { code: "de", name: "Tedesco" },
  { code: "es", name: "Spagnolo" },
  { code: "pt", name: "Portoghese" },
  { code: "nl", name: "Olandese" },
  { code: "ru", name: "Russo" },
];

export function languageName(code: string): string {
  return LANGUAGES.find((language) => language.code === code)?.name.toLowerCase() ?? code;
}

export function isSourceFormat(value: string): value is SourceFormat {
  return value === "epub" || value === "pdf" || value === "markdown";
}

export const FORMAT_OPTIONS: ReadonlyArray<{ value: SourceFormat; label: string }> = [
  { value: "epub", label: "EPUB" },
  { value: "pdf", label: "PDF" },
  { value: "markdown", label: "Markdown" },
];

export const EMPTY_FORM: FormState = {
  name: "",
  source_path: "",
  source_format: "epub",
  source_lang: "",
  target_lang: "it",
};

/** Maps a file extension to the format the sidecar will report (`PLAN.md` §12.1). */
export function detectFormatFromPath(path: string): SourceFormat | null {
  const extension = fileExtension(path);
  if (extension === null) {
    return null;
  }
  if (extension === "epub") {
    return "epub";
  }
  if (extension === "pdf") {
    return "pdf";
  }
  if (extension === "md" || extension === "markdown" || extension === "mkd") {
    return "markdown";
  }
  return null;
}

export function looksAbsolute(path: string): boolean {
  return path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\");
}

export function formatLabel(format: string): string {
  return FORMAT_OPTIONS.find((option) => option.value === format)?.label ?? format;
}

/** The suggested name for a file: its stem, unless the stem is the empty-name marker. */
export function suggestedName(path: string): string | null {
  const stem = basename(path).replace(/\.[^.]+$/, "");
  return stem.length > 0 && stem !== "—" ? stem : null;
}
