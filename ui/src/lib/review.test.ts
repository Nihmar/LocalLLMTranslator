import assert from "node:assert/strict";
import { test } from "node:test";

import {
  applyProposal,
  currentTextByBlock,
  inboxOrder,
  isImportant,
  proposalSegments,
  severityClass,
} from "./review.ts";
import type { Suggestion } from "./types.ts";

function suggestion(overrides: Partial<Suggestion>): Suggestion {
  return {
    id: "s000001",
    chunk_id: "c000001",
    pass: "editor",
    block_id: "b000001",
    field: "text",
    original: "Il vecchio porto",
    proposed: "Il nuovo porto",
    reason: "the adjective is wrong",
    severity: "major",
    quote: "vecchio",
    status: "accepted",
    created_at: "2026-01-01T10:00:00.000Z",
    decided_at: "2026-01-02T10:00:00.000Z",
    ...overrides,
  };
}

test("severityClass maps every severity to its badge", () => {
  assert.equal(severityClass("critical"), "badge badge-danger");
  assert.equal(severityClass("major"), "badge badge-warning");
  assert.equal(severityClass("minor"), "badge badge-info");
  assert.equal(severityClass(null), "badge badge-neutral");
});

test("applyProposal replaces the quote for every pass", () => {
  const proofread = suggestion({ pass: "proofreader", quote: "porto", proposed: "porto." });
  assert.equal(applyProposal("Il vecchio porto", proofread), "Il vecchio porto.");
  // Without the quote in the text the proposal is the whole block.
  assert.equal(applyProposal("Altro testo", proofread), "porto.");
});

test("proposalSegments marks the quote inside its paragraph", () => {
  const segments = proposalSegments("Il vecchio porto", suggestion({}));
  assert.deepEqual(segments, [
    { text: "Il ", kind: "same" },
    { text: "vecchio", kind: "removed" },
    { text: "Il nuovo porto", kind: "added" },
    { text: " porto", kind: "same" },
  ]);
  const whole = proposalSegments("Testo", suggestion({ quote: null, proposed: "Nuovo" }));
  assert.deepEqual(whole, [
    { text: "Testo", kind: "removed" },
    { text: "Nuovo", kind: "added" },
  ]);
});

test("inboxOrder puts the serious issues first, then the book order", () => {
  const order = new Map([
    ["c1", 1],
    ["c2", 2],
  ]);
  const sorted = inboxOrder(
    [
      suggestion({ id: "minor-early", severity: "minor", chunk_id: "c1" }),
      suggestion({ id: "major-late", severity: "major", chunk_id: "c2" }),
      suggestion({ id: "major-early", severity: "major", chunk_id: "c1" }),
      suggestion({ id: "critical", severity: "critical", chunk_id: "c2" }),
    ],
    order,
  ).map((item) => item.id);
  assert.deepEqual(sorted, ["critical", "major-early", "major-late", "minor-early"]);
  assert.equal(isImportant(suggestion({ severity: "minor" })), false);
});

test("currentTextByBlock keeps the newest translation of each block", () => {
  const texts = currentTextByBlock([
    { block_id: "b1", text_md: "vecchio", origin: "translator", updated_at: "2026-01-01" },
    { block_id: "b1", text_md: "nuovo", origin: "editor", updated_at: "2026-01-02" },
    { block_id: "b2", text_md: " ", origin: "translator", updated_at: "2026-01-03" },
  ]);
  assert.equal(texts.get("b1"), "nuovo");
  assert.equal(texts.has("b2"), false);
});
