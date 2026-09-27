-- Keep a reasoning model's thinking next to its answer (PLAN.md §7.1).
--
-- llama-server streams the thinking of a reasoning model in `reasoning_content`,
-- separately from the answer in `content`. Without it a call that spent its whole
-- `max_tokens` budget thinking is indistinguishable from a server that answered
-- nothing at all, which is exactly the case the editor used to report as
-- "the editor answer contains no JSON object".

ALTER TABLE llm_call ADD COLUMN reasoning_text TEXT;
