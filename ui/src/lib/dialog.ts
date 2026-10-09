/**
 * OS file-picker bridge.
 *
 * `@tauri-apps/plugin-dialog` is the only way to obtain an absolute path from a native dialog;
 * the Rust plugin is registered in `crates/app` and the `dialog:default` permission is granted in
 * `crates/app/capabilities/default.json`. Outside the Tauri webview (a plain browser tab) the
 * dialog is unavailable, so the picked file is uploaded to the headless server
 * (`PLAN.md` §12.3) and the *server* path is returned: the commands stay path-based, and the
 * caller does not care which shell it runs in.
 */

import { open } from "@tauri-apps/plugin-dialog";
import { uploadFile } from "./http.ts";
import { isTauriRuntime } from "./ipc.ts";

/** Extensions the ingestion accepts, without the `.` prefix. */
export const DOCUMENT_EXTENSIONS: readonly string[] = ["epub", "pdf", "md", "markdown", "mkd"];

/** Extension of the project bundle archives. */
export const BUNDLE_EXTENSION = "llmtz";

/** Extension of the series canon bundles (`PLAN.md` §9.5). */
export const SERIES_BUNDLE_EXTENSION = "llmtsz";

/**
 * Opens the document picker and returns the chosen absolute path (desktop) or the path the
 * server stored the uploaded copy under (browser). `null` only when the user cancels; a broken
 * native dialog is logged and reported as `null` so the manual input stays usable.
 */
export async function pickDocumentFile(): Promise<string | null> {
  return pickFilePath("Scegli il documento", [...DOCUMENT_EXTENSIONS], "Documenti");
}

/** Opens the OS picker for a `.llmtz` project bundle, or uploads one in a browser. */
export async function pickBundleFile(): Promise<string | null> {
  return pickFilePath("Importa progetto (.llmtz)", [BUNDLE_EXTENSION], "Bundle progetto");
}

/** Opens the OS picker for a `.llmtsz` series bundle, or uploads one in a browser. */
export async function pickSeriesBundleFile(): Promise<string | null> {
  return pickFilePath("Importa serie (.llmtsz)", [SERIES_BUNDLE_EXTENSION], "Bundle serie");
}

/** The path of a picked file, uploading it first when there is no native picker. */
async function pickFilePath(
  title: string,
  extensions: string[],
  filterName: string,
): Promise<string | null> {
  if (isTauriRuntime()) {
    return pickFile(title, extensions, filterName);
  }
  const file = await pickBrowserFile(extensions);
  if (file === null) {
    return null;
  }
  const uploaded = await uploadFile(file);
  return uploaded.path;
}

/** The native picker; only called inside the Tauri webview. */
async function pickFile(
  title: string,
  extensions: string[],
  filterName: string,
): Promise<string | null> {
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

/** A hidden `<input type=file>`; `null` when the user cancels. */
function pickBrowserFile(extensions: readonly string[]): Promise<File | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = extensions.map((extension) => `.${extension}`).join(",");
    input.style.display = "none";
    document.body.append(input);
    const settle = (file: File | null): void => {
      input.remove();
      resolve(file);
    };
    input.addEventListener(
      "change",
      () => {
        settle(input.files?.[0] ?? null);
      },
      { once: true },
    );
    // Fired by modern browsers when the picker is dismissed without a choice.
    input.addEventListener("cancel", () => settle(null), { once: true });
    input.click();
  });
}
