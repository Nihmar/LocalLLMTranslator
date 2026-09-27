import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { toErrorMessage } from "./ipc.ts";

// Rust's `AppError` serialises as `{ code, message, retryable }` (`crates/app/src/error.rs`), so a
// rejected `invoke()` carries an object, not a string: the banner must show the human message,
// never the raw JSON. The object shape is handled first, ahead of `Error` and `string`.
test("toErrorMessage unwraps the serialized AppError object", () => {
  const error = { code: "not_found", message: "not found: project 42", retryable: false };

  assert.equal(toErrorMessage(error), "not found: project 42 (not_found)");
});

test("toErrorMessage never renders the raw AppError JSON", () => {
  const error = { code: "sidecar_timeout", message: "sidecar request timed out", retryable: true };

  assert.equal(toErrorMessage(error).includes('"code"'), false);
});

test("toErrorMessage keeps the plain Error and string forms", () => {
  assert.equal(toErrorMessage(new Error("boom")), "boom");
  assert.equal(toErrorMessage("plain text"), "plain text");
});

/**
 * Tauri maps a Rust command parameter to a **camelCase** key (`project_id` → `projectId`) unless
 * the command opts out with `rename_all`. A snake_case key in a flat argument object therefore
 * fails argument validation before the command ever runs, and the mistake stays invisible until
 * the view that calls it is opened — `recon_get`, `glossary_list`, the series glossary, the series
 * QA scan and the export history all shipped with it.
 *
 * Keys *inside* `{ req: { ... } }` are struct fields, which serde deserialises exactly as
 * written, so only the flat form is checked.
 */
const SOURCE = readFileSync(fileURLToPath(new URL("./ipc.ts", import.meta.url)), "utf8");
const FLAT_CALL = /call<[^>]*>\(COMMANDS\.(\w+), \{(.*)\}\)/g;

test("a flat command argument uses Tauri's camelCase key", () => {
  const offenders: string[] = [];
  for (const match of SOURCE.matchAll(FLAT_CALL)) {
    const command = match[1] ?? "";
    const args = (match[2] ?? "").trim();
    if (args.startsWith("req:") || !args.includes("_")) {
      continue;
    }
    offenders.push(`${command} sends { ${args} }`);
  }

  assert.deepEqual(offenders, [], `flat argument keys must be camelCase: ${offenders.join("; ")}`);
});

test("the scan actually reaches the calls", () => {
  // A regex that matched nothing would make the test above pass for ever.
  assert.ok([...SOURCE.matchAll(FLAT_CALL)].length > 20, "the wrapper calls are not being scanned");
});
