import assert from "node:assert/strict";
import { test } from "node:test";

import { chapterProgress, chunkTranslated, readableText } from "./chapters.ts";
import type { Chapter, Chunk } from "./types.ts";

function chapter(id: string, order_index: number, title: string): Chapter {
  return {
    id,
    document_id: "doc",
    order_index,
    title,
    level: 1,
    block_first: 0,
    block_last: 0,
    summary: null,
    summary_model: null,
    summary_hash: null,
    status: "pending",
  };
}

function chunk(
  id: string,
  chapter_id: string | null,
  order_index: number,
  status: string,
  source_md: string,
  target_md: string | null,
): Chunk {
  return {
    id,
    document_id: "doc",
    chapter_id,
    order_index,
    block_ids_json: "[]",
    source_md,
    token_estimate: 10,
    context_json: "{}",
    flags_json: "[]",
    status,
    prompt_hash: null,
    model_id: null,
    params_json: null,
    context_manifest_json: null,
    target_md,
    error: null,
    created_at: "",
    updated_at: "",
  };
}

test("chunkTranslated treats a missing or whitespace-only target as untranslated", () => {
  assert.equal(chunkTranslated(chunk("c1", "ch1", 0, "done", "a", "# Ciao")), true);
  assert.equal(chunkTranslated(chunk("c2", "ch1", 1, "done", "a", null)), false);
  assert.equal(chunkTranslated(chunk("c3", "ch1", 2, "done", "a", "   \n")), false);
});

test("chapterProgress groups chunks in document order and drops empty chapters", () => {
  const chapters = [chapter("ch2", 2, "Due"), chapter("ch1", 1, "Uno"), chapter("ch3", 3, "Tre")];
  const chunks = [
    chunk("c1", "ch1", 0, "done", "# Uno", "# One"),
    chunk("c2", "ch1", 1, "running", "testo", null),
    chunk("c3", "ch2", 2, "failed", "testo", null),
    // Preamble and unknown chapters are not part of the outline.
    chunk("c4", null, 3, "done", "front", "front tradotto"),
    chunk("c5", "missing", 4, "done", "x", "y"),
  ];

  const rows = chapterProgress(chapters, chunks);

  assert.deepEqual(
    rows.map((row) => row.id),
    ["ch1", "ch2"],
  );
  assert.deepEqual(rows[0], {
    id: "ch1",
    order_index: 1,
    title: "Uno",
    level: 1,
    total: 2,
    translated: 1,
    running: 1,
    failed: 0,
    needs_review: 0,
    tokens: 20,
  });
  assert.deepEqual(rows[1], {
    id: "ch2",
    order_index: 2,
    title: "Due",
    level: 1,
    total: 1,
    translated: 0,
    running: 0,
    failed: 1,
    needs_review: 0,
    tokens: 10,
  });
});

test("chapterProgress counts a needs_review chunk as translated when it carries a target", () => {
  const chunks = [
    chunk("c1", "ch1", 0, "needs_review", "a", "b"),
    chunk("c2", "ch1", 1, "needs_review", "a", null),
  ];

  const [row] = chapterProgress([chapter("ch1", 1, "Uno")], chunks);

  assert.ok(row);
  assert.equal(row.translated, 1);
  assert.equal(row.needs_review, 2);
});

test("readableText drops markdown markers for reading", () => {
  assert.equal(readableText("## Chapitre 21"), "Chapitre 21");
  assert.equal(readableText("\\- Sit down. **Now**, *please*."), "- Sit down. Now, please.");
  assert.equal(readableText("2 * 3 = 6"), "2 * 3 = 6");
});
