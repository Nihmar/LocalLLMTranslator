-- Proofreader suggestions from before the pass answered span-level JSON are whole-block
-- rewrites with no severity and no reason: hundreds per book, unreviewable. Retire the
-- undecided ones; running the review again proposes them in the new form. Decided rows
-- keep their status, so the correction history is untouched.
UPDATE suggestion
SET status = 'superseded'
WHERE pass = 'proofreader'
  AND status = 'pending'
  AND severity IS NULL;
