import assert from "node:assert/strict";
import { test } from "node:test";

import {
  elapsedSince,
  isActiveJobState,
  isCancellableJobState,
  jobKindLabel,
  payloadChunkId,
} from "./jobs.ts";

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
