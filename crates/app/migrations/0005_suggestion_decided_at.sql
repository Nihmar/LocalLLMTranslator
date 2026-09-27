-- Keep the moment a proposal was decided, so the retained suggestion rows read back
-- as a per-project history of the corrections made (PLAN.md §11.4).
--
-- `created_at` is when the editor or proofreader pass proposed the change;
-- `decided_at` is when the user accepted or rejected it. It stays NULL while the
-- proposal is pending, and for a superseded row that was never decided at all.

ALTER TABLE suggestion ADD COLUMN decided_at TEXT;

-- Decisions taken before this column existed: the proposal time is the closest moment
-- on record, and without the backfill the correction history of every project reviewed
-- so far would read as empty.
UPDATE suggestion SET decided_at = created_at WHERE status IN ('accepted', 'rejected');
