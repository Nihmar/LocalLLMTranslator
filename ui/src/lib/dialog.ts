/**
 * OS file-picker bridge.
 *
 * `@tauri-apps/plugin-dialog` is the only way to obtain an absolute path from a native dialog;
 * the Rust plugin is registered in `crates/app` and the `dialog:default` permission is granted in
 * `crates/app/capabilities/default.json`. Outside the Tauri webview (a plain browser tab) the
 * dialog is unavailable, so `pickDocumentFile` degrades to `null` and the caller keeps its manual
 * path field usable.
 */

import { open } from "@tauri-apps/plugin-dialog";
import { isTauriRuntime } from "./ipc";

/** Extensions the ingestion accepts, without the `.` prefix. */
export const DOCUMENT_EXTENSIONS: readonly string[] = ["epub", "pdf", "md", "markdown", "mkd"];

/** Extension of the project bundle archives. */
export const BUNDLE_EXTENSION = "llmtz";

/** Extension of the series canon bundles (`PLAN.md` §9.5). */
export const SERIES_BUNDLE_EXTENSION = "llmtsz";

/**
 * Opens the OS document picker and returns the chosen absolute path, or `null` when the user
 * cancels or the dialog is unavailable. Failures are logged and reported as `null` so a broken
 * dialog never blocks the manual input.
 */
export async function pickDocumentFile(): Promise<string | null> {
  return pickFile("Scegli il documento", [...DOCUMENT_EXTENSIONS], "Documenti");
}

/** Opens the OS picker for a `.llmtz` project bundle. */
export async function pickBundleFile(): Promise<string | null> {
  return pickFile("Importa progetto (.llmtz)", [BUNDLE_EXTENSION], "Bundle progetto");
}

/** Opens the OS picker for a `.llmtsz` series bundle. */
export async function pickSeriesBundleFile(): Promise<string | null> {
  return pickFile("Importa serie (.llmtsz)", [SERIES_BUNDLE_EXTENSION], "Bundle serie");
}

async function pickFile(
  title: string,
  extensions: string[],
  filterName: string,
): Promise<string | null> {
  if (!isTauriRuntime()) {
    return null;
  }
  try {
    const selected = await open({
      multiple: false,
      directory: false,
      title,
      filters: [{ name: filterName, extensions }],
    });
    return typeof selected === "string" ? selected : null;
  } catch (error) {
    console.error("[dialog] file picker unavailable", error);
    return null;
  }
}
