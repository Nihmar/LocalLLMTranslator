import assert from "node:assert/strict";
import { test } from "node:test";

import { severityClass, suggestionSnippet } from "./review.ts";
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

test("suggestionSnippet quotes the replaced fragment", () => {
  assert.equal(suggestionSnippet(suggestion({})), "«vecchio» → «Il nuovo porto»");
});

test("suggestionSnippet falls back to the rewritten block without a quote", () => {
  assert.equal(
    suggestionSnippet(suggestion({ quote: null, pass: "proofreader", proposed: "Il porto." })),
    "blocco riscritto dal proofreader",
  );
  assert.equal(
    suggestionSnippet(suggestion({ quote: null, proposed: "Il nuovo porto" })),
    "blocco riscritto: «Il nuovo porto»",
  );
});

test("suggestionSnippet survives a proposal with no text", () => {
  assert.equal(
    suggestionSnippet(suggestion({ quote: null, proposed: null })),
    "proposta senza testo",
  );
});

test("severityClass maps every severity to its badge", () => {
  assert.equal(severityClass("critical"), "badge badge-danger");
  assert.equal(severityClass("major"), "badge badge-warning");
  assert.equal(severityClass("minor"), "badge badge-info");
  assert.equal(severityClass(null), "badge badge-neutral");
});
