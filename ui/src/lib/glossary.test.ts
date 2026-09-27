import assert from "node:assert/strict";
import { test } from "node:test";

import {
  glossaryCounts,
  glossaryMatches,
  glossaryOrder,
  matchesNeedle,
  needsTarget,
  pendingDecisionCount,
} from "./glossary.ts";
import type { GlossaryTerm } from "./types.ts";

function term(
  id: string,
  status: string,
  source: string,
  target = source,
  note: string | null = null,
): GlossaryTerm {
  return {
    id,
    project_id: "p1",
    source_lang: "English",
    target_lang: "Italian",
    source,
    target,
    note,
    kind: "term",
    origin: "manual",
    revision: 1,
    status,
  };
}

const TERMS: GlossaryTerm[] = [
  term("a", "approved", "harbour", "porto"),
  term("b", "candidate", "keeper", "guardiano", "proposed by the summarizer"),
  term("c", "rejected", "zephyr", "zefiro"),
  term("d", "conflict", "light", "luce"),
];

test("a candidate comes before a conflict, an approved and a rejected term", () => {
  assert.deepEqual(
    glossaryOrder(TERMS).map((row) => row.id),
    ["b", "d", "a", "c"],
  );
});

test("terms of the same status are ordered by source, ignoring case", () => {
  assert.deepEqual(
    glossaryOrder([term("x", "approved", "zeta"), term("y", "approved", "Alpha")]).map(
      (row) => row.source,
    ),
    ["Alpha", "zeta"],
  );
  // Equal keys keep the order they arrived in, so the table never reshuffles on its own.
  const sameFolded = [term("x", "candidate", "keeper"), term("y", "candidate", "Keeper")];
  assert.deepEqual(
    glossaryOrder(sameFolded).map((row) => row.id),
    ["x", "y"],
  );
});

test("an unknown status sorts after the known ones and is never dropped", () => {
  const rows = glossaryOrder([term("x", "future_status", "source"), term("y", "rejected", "other")]);
  assert.deepEqual(
    rows.map((row) => row.id),
    ["y", "x"],
  );
  assert.deepEqual(glossaryOrder(rows), rows, "the order is stable");
});

test("the status filter keeps one status only", () => {
  assert.equal(glossaryMatches(term("a", "approved", "harbour"), "approved", ""), true);
  assert.equal(glossaryMatches(term("a", "approved", "harbour"), "rejected", ""), false);
  // The empty status means every status.
  assert.equal(glossaryMatches(term("a", "rejected", "harbour"), "", ""), true);
});

test("the query searches source, rendering and note", () => {
  assert.equal(matchesNeedle(["harbour", "porto", ""], "porto"), true);
  assert.equal(matchesNeedle(["keeper", "guardiano", "proposed by the summarizer"], "summarizer"), true);
  assert.equal(matchesNeedle(["keeper", "guardiano", "note"], "GUARDIANO"), true);
  assert.equal(matchesNeedle(["keeper", "guardiano", "note"], "nothing"), false);
  assert.equal(matchesNeedle(["keeper", "guardiano", "note"], "   "), true);
  // An empty note is searched as an empty string, never as a null crash.
  assert.equal(matchesNeedle(["harbour", "porto", ""], "harbour"), true);
});

test("the counters agree with the rows", () => {
  const counts = glossaryCounts(TERMS);
  assert.equal(counts.candidate, 1);
  assert.equal(counts.approved, 1);
  assert.equal(counts.conflict, 1);
  assert.equal(counts.rejected, 1);
  assert.equal(pendingDecisionCount(TERMS), 2);
});

test("a do-not-translate term needs no rendering", () => {
  assert.equal(needsTarget("do_not_translate"), false);
  assert.equal(needsTarget("term"), true);
  assert.equal(needsTarget("proper_noun"), true);
});
