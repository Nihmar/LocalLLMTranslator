import assert from "node:assert/strict";
import { test } from "node:test";

import { nextAction, type BookState } from "./overview.ts";

const book = (overrides: Partial<BookState>): BookState => ({
  chapters: 39,
  parts: 41,
  translated: 0,
  running: 0,
  failed: 0,
  openDecisions: 0,
  importantDecisions: 0,
  candidateTerms: 0,
  ...overrides,
});

test("nextAction follows the book through its steps", () => {
  assert.equal(nextAction(book({ chapters: 0 })).step, "ingest");
  assert.equal(nextAction(book({ candidateTerms: 8 })).step, "glossary");
  assert.equal(nextAction(book({})).step, "translate");
  assert.equal(nextAction(book({ translated: 23, failed: 1 })).step, "translate");
  assert.equal(nextAction(book({ translated: 23, running: 1, importantDecisions: 4 })).step, "review");
  assert.equal(nextAction(book({ translated: 41, openDecisions: 3 })).step, "review");
  assert.equal(nextAction(book({ translated: 41 })).step, "export");
});
