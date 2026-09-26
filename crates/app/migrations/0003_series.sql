-- Series: a shared canon across books (PLAN.md §9.5).
--
-- A series owns the language pair its books share, a glossary layer the projects
-- inherit (a project term overrides the series rendering for the same source) and
-- memory values (style guide, synopsis, canon). Projects keep their own glossary
-- and memory unchanged, so a standalone book behaves exactly as before.

CREATE TABLE series (
  id TEXT PRIMARY KEY, name TEXT NOT NULL,
  source_lang TEXT, target_lang TEXT,
  settings_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE series_glossary_term (
  id TEXT PRIMARY KEY, series_id TEXT NOT NULL REFERENCES series(id) ON DELETE CASCADE,
  source_lang TEXT, target_lang TEXT,
  source TEXT NOT NULL, target TEXT NOT NULL, note TEXT,
  kind TEXT NOT NULL DEFAULT 'term',        -- term|proper_noun|do_not_translate
  origin TEXT NOT NULL DEFAULT 'manual',    -- manual|proposed|imported|promoted
  revision INTEGER NOT NULL DEFAULT 1,      -- optimistic lock
  status TEXT NOT NULL DEFAULT 'approved',  -- approved|candidate|conflict|rejected
  UNIQUE(series_id, source_lang, target_lang, source)
);

CREATE TABLE series_glossary_variant (
  id TEXT PRIMARY KEY,
  term_id TEXT NOT NULL REFERENCES series_glossary_term(id) ON DELETE CASCADE,
  text TEXT NOT NULL,
  UNIQUE(term_id, text)
);

CREATE TABLE series_memory (
  series_id TEXT NOT NULL REFERENCES series(id) ON DELETE CASCADE,
  key TEXT NOT NULL, value TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1, updated_at TEXT NOT NULL,
  PRIMARY KEY (series_id, key)
);

ALTER TABLE project ADD COLUMN series_id TEXT REFERENCES series(id) ON DELETE SET NULL;
ALTER TABLE project ADD COLUMN series_order INTEGER;
CREATE INDEX idx_project_series ON project(series_id, series_order);

-- `translation_memory` reuses a block translation only under the same effective
-- glossary: a canon change must not keep serving renderings made before it. The
-- primary key cannot be altered in place, so the table is rebuilt; the old rows
-- carry no glossary identity and are dropped rather than reused under an unknown
-- canon (the exact `translation_cache` is unaffected: its key already includes the
-- prompt, hence the glossary text).
ALTER TABLE translation_memory RENAME TO translation_memory_v1;

CREATE TABLE translation_memory (
  content_hash TEXT NOT NULL, model TEXT NOT NULL, target_lang TEXT NOT NULL,
  glossary_hash TEXT NOT NULL DEFAULT '',
  text_md TEXT NOT NULL, hits INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL,
  PRIMARY KEY (content_hash, model, target_lang, glossary_hash)
);

DROP TABLE translation_memory_v1;
