import assert from "node:assert/strict";
import { test } from "node:test";

import {
  elapsedSince,
  isActiveJobState,
  isCancellableJobState,
  jobKindLabel,
  jobRows,
  payloadChunkId,
  sortForMonitor,
} from "./jobs.ts";
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

test("a job can be interrupted while it is queued or running", () => {
  assert.equal(isCancellableJobState("pending"), true);
  assert.equal(isCancellableJobState("leased"), true);
  assert.equal(isCancellableJobState("running"), true);
  assert.equal(isCancellableJobState("done"), false);
  assert.equal(isCancellableJobState("failed"), false);
  assert.equal(isCancellableJobState("cancelled"), false);
});

test("the chunk id is read from the payload", () => {
  assert.equal(payloadChunkId('{"chunk_id":"c1"}'), "c1");
  assert.equal(payloadChunkId('{"other":1}'), null);
  assert.equal(payloadChunkId("not json"), null);
  assert.equal(payloadChunkId("null"), null);
  assert.equal(payloadChunkId('{"chunk_id":7}'), null);
});

test("elapsedSince needs a readable claim and never goes negative", () => {
  const now = Date.parse("2026-01-01T00:01:00Z");
  assert.equal(elapsedSince(null, now), null);
  assert.equal(elapsedSince("not a date", now), null);
  assert.equal(elapsedSince("2026-01-01T00:00:30Z", now), 30_000);
  assert.equal(elapsedSince("2026-01-01T00:02:00Z", now), 0);
});

function job(
  id: string,
  kind: string,
  state: string,
  started_at: string | null,
  finished_at: string | null,
  payload_json = "{}",
): Job {
  return {
    id,
    project_id: "p1",
    kind,
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
    finished_at,
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

test("jobRows names the chunk and chapter and marks what can be interrupted", () => {
  const now = Date.parse("2026-01-01T00:10:00Z");
  const rows = jobRows(
    [
      job(
        "j1",
        "translate_chunk",
        "running",
        "2026-01-01T00:09:00Z",
        null,
        '{"chunk_id":"c1"}',
      ),
      job(
        "j2",
        "qa_scan",
        "running",
        "2026-01-01T00:09:30Z",
        null,
        '{"chunk_id":"c9"}',
      ),
      job("j3", "summarize", "done", "2026-01-01T00:00:00Z", "2026-01-01T00:02:00Z"),
    ],
    [chunk("c1", "ch1")],
    new Map([["ch1", "Un duello all'alba"]]),
    now,
  );

  const byId = new Map(rows.map((row) => [row.job.id, row]));
  assert.equal(byId.get("j1")?.chapter_title, "Un duello all'alba");
  assert.equal(byId.get("j1")?.elapsed_ms, 60_000);
  assert.equal(byId.get("j1")?.cancellable, true);
  // A payload naming an unknown chunk keeps the row, with the chapter left empty.
  assert.equal(byId.get("j2")?.chunk_id, "c9");
  assert.equal(byId.get("j2")?.chapter_title, null);
  // A finished job measures its own run and cannot be interrupted.
  assert.equal(byId.get("j3")?.elapsed_ms, 120_000);
  assert.equal(byId.get("j3")?.cancellable, false);
});

test("sortForMonitor puts what is moving first and finished work last", () => {
  const now = Date.parse("2026-01-01T01:00:00Z");
  const rows = jobRows(
    [
      job("done", "translate_chunk", "done", "2026-01-01T00:00:00Z", "2026-01-01T00:01:00Z"),
      job("pending", "translate_chunk", "pending", null, null),
      job("leased", "translate_chunk", "leased", null, null),
      job("running-old", "translate_chunk", "running", "2026-01-01T00:10:00Z", null),
      job("running-new", "translate_chunk", "running", "2026-01-01T00:20:00Z", null),
    ],
    [],
    new Map(),
    now,
  );

  assert.deepEqual(
    sortForMonitor(rows).map((row) => row.job.id),
    ["running-new", "running-old", "leased", "pending", "done"],
  );
});
