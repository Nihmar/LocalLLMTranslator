import assert from "node:assert/strict";
import { test } from "node:test";

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
