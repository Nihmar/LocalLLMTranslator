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
 * Every command is declared with `rename_all = "snake_case"`, so an argument key is the Rust
 * parameter name as written (`project_id`). Tauri's default would expect `projectId`, and a key in
 * the wrong spelling fails validation before the command runs, invisibly until the view that
 * calls it is opened — `recon_get` shipped that way 319 times in one day of use. One spelling,
 * checked here, instead of a convention remembered in comments.
 *
 * Keys *inside* `{ req: { ... } }` are struct fields, which serde deserialises exactly as
 * written, so only the flat form is checked.
 */
const SOURCE = readFileSync(fileURLToPath(new URL("./ipc.ts", import.meta.url)), "utf8");
const FLAT_CALL = /call<[^>]*>\(COMMANDS\.(\w+), \{(.*)\}\)/g;

test("a flat command argument uses the Rust parameter name", () => {
  const offenders: string[] = [];
  for (const match of SOURCE.matchAll(FLAT_CALL)) {
    const command = match[1] ?? "";
    const args = (match[2] ?? "").trim();
    if (args.startsWith("req:")) {
      continue;
    }
    const keys = args.split(",").map((part) => (part.split(":")[0] ?? "").trim());
    if (keys.some((key) => /[A-Z]/.test(key))) {
      offenders.push(`${command} sends { ${args} }`);
    }
  }

  assert.deepEqual(offenders, [], `flat argument keys must be snake_case: ${offenders.join("; ")}`);
});

test("the scan actually reaches the calls", () => {
  // A regex that matched nothing would make the test above pass for ever.
  assert.ok([...SOURCE.matchAll(FLAT_CALL)].length > 20, "the wrapper calls are not being scanned");
});
