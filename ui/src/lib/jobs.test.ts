import assert from "node:assert/strict";
import { test } from "node:test";

import { activeJobRows, elapsedSince, isActiveJobState, jobKindLabel, payloadChunkId } from "./jobs.ts";
import type { Chunk, Job } from "./types.ts";

test("known job kinds are labelled in Italian", () => {
  assert.equal(jobKindLabel("translate_chunk"), "Traduzione chunk");
  assert.equal(jobKindLabel("edit_chunk"), "Revisione editor");
});

test("an unknown kind is shown verbatim", () => {
  assert.equal(jobKindLabel("future_kind"), "future_kind");
});

test("only running and leased jobs occupy a worker", () => {
  assert.equal(isActiveJobState("running"), true);
  assert.equal(isActiveJobState("leased"), true);
  assert.equal(isActiveJobState("pending"), false);
  assert.equal(isActiveJobState("done"), false);
});

test("the chunk id is read from the payload", () => {
  assert.equal(payloadChunkId('{"chunk_id":"c1"}'), "c1");
  assert.equal(payloadChunkId('{"other":1}'), null);
  assert.equal(payloadChunkId("not json"), null);
  assert.equal(payloadChunkId("null"), null);
  assert.equal(payloadChunkId('{"chunk_id":7}'), null);
});

function job(
  id: string,
  state: string,
  payload_json: string,
  started_at: string | null,
): Job {
  return {
    id,
    project_id: "p1",
    kind: "translate_chunk",
    payload_json,
    priority: 0,
    state,
    attempts: 1,
    max_attempts: 3,
    lease_owner: null,
    lease_expires_at: null,
    run_after: null,
    last_error: null,
    created_at: "2026-01-01T00:00:00Z",
    started_at,
    finished_at: null,
  };
}

function chunk(id: string, chapter_id: string | null): Chunk {
  return {
    id,
    document_id: "doc",
    chapter_id,
    order_index: 0,
    block_ids_json: "[]",
    source_md: "",
    token_estimate: 10,
    context_json: "{}",
    flags_json: "[]",
    status: "running",
    prompt_hash: null,
    model_id: null,
    params_json: null,
    context_manifest_json: null,
    target_md: null,
    error: null,
    created_at: "",
    updated_at: "",
  };
}

test("activeJobRows keeps only the jobs holding a worker, oldest claim first", () => {
  const jobs = [
    job("j-late", "running", '{"chunk_id":"c2"}', "2026-01-01T00:02:00Z"),
    job("j-pending", "pending", '{"chunk_id":"c3"}', null),
    job("j-early", "leased", '{"chunk_id":"c1"}', "2026-01-01T00:01:00Z"),
    job("j-done", "done", '{"chunk_id":"c4"}', "2026-01-01T00:00:00Z"),
  ];

  const rows = activeJobRows(
    jobs,
    [chunk("c1", "ch1"), chunk("c2", "ch2")],
    new Map([
      ["ch1", "Uno"],
      ["ch2", "Due"],
    ]),
  );

  assert.deepEqual(
    rows.map((row) => row.id),
    ["j-early", "j-late"],
  );
  assert.equal(rows[0]?.kind_label, "Traduzione chunk");
  assert.equal(rows[0]?.chapter_title, "Uno");
  assert.equal(rows[1]?.chapter_title, "Due");
});

test("activeJobRows leaves the chapter empty when the chunk cannot be resolved", () => {
  const rows = activeJobRows(
    [job("j1", "running", '{"chunk_id":"missing"}', "2026-01-01T00:00:00Z")],
    [chunk("c1", "ch1")],
    new Map([["ch1", "Uno"]]),
  );

  assert.equal(rows[0]?.chunk_id, "missing");
  assert.equal(rows[0]?.chapter_id, null);
  assert.equal(rows[0]?.chapter_title, null);
});

test("elapsedSince needs a readable claim and never goes negative", () => {
  const now = Date.parse("2026-01-01T00:01:00Z");
  assert.equal(elapsedSince(null, now), null);
  assert.equal(elapsedSince("not a date", now), null);
  assert.equal(elapsedSince("2026-01-01T00:00:30Z", now), 30_000);
  assert.equal(elapsedSince("2026-01-01T00:02:00Z", now), 0);
});
