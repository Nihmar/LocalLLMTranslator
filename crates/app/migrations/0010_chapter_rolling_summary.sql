-- The rolling summary is per chapter now (`rolling_summary:<chapter_id>`), so the value the
-- translator injects is always the chapter it belongs to. The old global key is ambiguous
-- (it could hold another chapter's summary); drop it instead of migrating it into a
-- chapter it may not belong to.
DELETE FROM project_memory WHERE key = 'rolling_summary';
