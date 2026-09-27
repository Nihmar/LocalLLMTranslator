import assert from "node:assert/strict";
import { test } from "node:test";

import { isActiveJobState, jobKindLabel, payloadChunkId } from "./jobs.ts";

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
