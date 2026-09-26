import assert from "node:assert/strict";
import { test } from "node:test";

import { diffWords, MAX_TOKENS } from "./diff.ts";
import type { DiffSegment } from "./diff.ts";

function joined(segments: readonly DiffSegment[], kinds: readonly string[]): string {
  return segments
    .filter((segment) => kinds.includes(segment.kind))
    .map((segment) => segment.text)
    .join("");
}

test("identical text is a single equal segment", () => {
  const segments = diffWords("Il porto era tranquillo.", "Il porto era tranquillo.");
  assert.deepEqual(segments, [{ kind: "equal", text: "Il porto era tranquillo." }]);
});

test("a replaced word is removed on one side and added on the other", () => {
  const segments = diffWords("Il vecchio porto", "Il nuovo porto");
  assert.equal(joined(segments, ["equal", "removed"]), "Il vecchio porto");
  assert.equal(joined(segments, ["equal", "added"]), "Il nuovo porto");
  assert.ok(segments.some((segment) => segment.kind === "removed" && segment.text.includes("vecchio")));
  assert.ok(segments.some((segment) => segment.kind === "added" && segment.text.includes("nuovo")));
});

test("an insertion keeps the surrounding text equal", () => {
  const segments = diffWords("La nave va", "La nave va veloce");
  assert.deepEqual(segments, [
    { kind: "equal", text: "La nave va" },
    { kind: "added", text: " veloce" },
  ]);
});

test("a deletion keeps the surrounding text equal", () => {
  const segments = diffWords("La nave va veloce", "La nave va");
  assert.deepEqual(segments, [
    { kind: "equal", text: "La nave va" },
    { kind: "removed", text: " veloce" },
  ]);
});

test("empty sides are handled", () => {
  assert.deepEqual(diffWords("", ""), []);
  assert.deepEqual(diffWords("text", ""), [{ kind: "removed", text: "text" }]);
  assert.deepEqual(diffWords("", "text"), [{ kind: "added", text: "text" }]);
});

test("a pathological block falls back to a coarse replacement", () => {
  const before = Array.from({ length: MAX_TOKENS + 10 }, (_, index) => `w${index}`).join(" ");
  const segments = diffWords(before, `${before} extra`);
  assert.deepEqual(
    segments.map((segment) => segment.kind),
    ["removed", "added"],
  );
});
