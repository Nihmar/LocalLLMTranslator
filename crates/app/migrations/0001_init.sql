-- LocalLLMTranslator initial schema.
-- Transcribed verbatim from PLAN.md section 5.
-- `PRAGMA foreign_keys = ON` is applied by the connection layer (see db::connect),
-- never inside a migration.

CREATE TABLE project (
  id TEXT PRIMARY KEY, name TEXT NOT NULL,
  source_path TEXT NOT NULL, source_hash TEXT NOT NULL,
  source_format TEXT NOT NULL, source_lang TEXT, target_lang TEXT NOT NULL,
  doc_title TEXT, doc_author TEXT,
  prompts_snapshot_dir TEXT,            -- copy of the prompts used -> reproducibility
  settings_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE document (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  markdown_path TEXT NOT NULL, front_matter_json TEXT NOT NULL,
  extractor TEXT NOT NULL, extractor_version TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE chapter (
  id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES document(id) ON DELETE CASCADE,
  order_index INTEGER NOT NULL, title TEXT NOT NULL, level INTEGER NOT NULL,
  block_first INTEGER NOT NULL, block_last INTEGER NOT NULL,
  summary TEXT, summary_model TEXT, summary_hash TEXT,
  status TEXT NOT NULL DEFAULT 'pending'
);

CREATE TABLE block (
  id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES document(id) ON DELETE CASCADE,
  chapter_id TEXT REFERENCES chapter(id), order_index INTEGER NOT NULL,
  kind TEXT NOT NULL, level INTEGER NOT NULL DEFAULT 0,
  source_md TEXT NOT NULL, source_text TEXT NOT NULL DEFAULT '',
  translatable INTEGER NOT NULL DEFAULT 1, attrs_json TEXT NOT NULL DEFAULT '{}',
  content_hash TEXT NOT NULL
);
CREATE INDEX idx_block_doc_order ON block(document_id, order_index);

CREATE TABLE chunk (
  id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES document(id) ON DELETE CASCADE,
  chapter_id TEXT REFERENCES chapter(id), order_index INTEGER NOT NULL,
  block_ids_json TEXT NOT NULL, source_md TEXT NOT NULL,
  token_estimate INTEGER NOT NULL, context_json TEXT NOT NULL DEFAULT '{}',
  flags_json TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL DEFAULT 'pending',   -- pending|running|done|failed|needs_review
  prompt_hash TEXT, model_id TEXT, params_json TEXT,
  context_manifest_json TEXT,               -- hash of the injected context pieces
  target_md TEXT, error TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX idx_chunk_status ON chunk(status, order_index);

CREATE TABLE block_translation (            -- source <-> translation map
  block_id TEXT NOT NULL REFERENCES block(id) ON DELETE CASCADE,
  chunk_id TEXT NOT NULL REFERENCES chunk(id) ON DELETE CASCADE,
  text_md TEXT NOT NULL, placeholders_ok INTEGER NOT NULL DEFAULT 1,
  origin TEXT NOT NULL,                     -- translator|editor|proofreader|user
  edited_by_user INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL,
  PRIMARY KEY (block_id, origin)
);

CREATE TABLE suggestion (                   -- proposals from editor/proofreader
  id TEXT PRIMARY KEY, chunk_id TEXT NOT NULL REFERENCES chunk(id) ON DELETE CASCADE,
  pass TEXT NOT NULL,                       -- editor|proofreader
  block_id TEXT, field TEXT, original TEXT, proposed TEXT,
  reason TEXT, severity TEXT, quote TEXT,
  status TEXT NOT NULL DEFAULT 'pending',   -- pending|accepted|rejected|superseded
  created_at TEXT NOT NULL
);

CREATE TABLE qa_finding (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL, chunk_id TEXT, block_id TEXT,
  kind TEXT NOT NULL,
  -- untranslated|glossary_mismatch|placeholder_broken|markdown_malformed|length_anomaly|
  -- duplicate|empty|latin_leftover|glossary_conflict
  severity TEXT NOT NULL, details_json TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'open', created_at TEXT NOT NULL
);

CREATE TABLE glossary_term (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL, source_lang TEXT, target_lang TEXT,
  source TEXT NOT NULL, target TEXT NOT NULL, note TEXT,
  kind TEXT NOT NULL DEFAULT 'term',        -- term|proper_noun|do_not_translate
  origin TEXT NOT NULL DEFAULT 'manual',    -- manual|proposed|imported
  revision INTEGER NOT NULL DEFAULT 1,      -- optimistic lock
  status TEXT NOT NULL DEFAULT 'approved',  -- approved|candidate|conflict|rejected
  UNIQUE(project_id, source_lang, target_lang, source)
);

CREATE TABLE project_memory (
  project_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1, updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, key)             -- synopsis, style_guide, rolling_summary, decisions
);

CREATE TABLE llm_endpoint (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, base_url TEXT NOT NULL,
  api_key_ref TEXT,                         -- name in the keyring, never the secret in clear text
  max_concurrency INTEGER, notes TEXT,
  last_health_at TEXT, last_health_ok INTEGER, props_json TEXT
);

CREATE TABLE role_binding (
  id TEXT PRIMARY KEY, endpoint_id TEXT NOT NULL REFERENCES llm_endpoint(id) ON DELETE CASCADE,
  role TEXT NOT NULL,                       -- translator|editor|proofreader|orchestrator
  model TEXT NOT NULL, params_json TEXT NOT NULL DEFAULT '{}',
  priority INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE job (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL,
  kind TEXT NOT NULL,                       -- ingest|translate_chunk|summarize|edit_chunk|
                                            -- proofread_chunk|qa_scan|export_unit
  payload_json TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 100,
  state TEXT NOT NULL DEFAULT 'pending',    -- pending|leased|running|done|failed|cancelled
  attempts INTEGER NOT NULL DEFAULT 0, max_attempts INTEGER NOT NULL DEFAULT 3,
  lease_owner TEXT, lease_expires_at TEXT,
  run_after TEXT, last_error TEXT,
  created_at TEXT NOT NULL, started_at TEXT, finished_at TEXT
);
CREATE INDEX idx_job_claim ON job(state, priority, created_at);

CREATE TABLE llm_call (                     -- audit + reproducibility
  id TEXT PRIMARY KEY, job_id TEXT, chunk_id TEXT, role TEXT NOT NULL,
  endpoint_id TEXT, model TEXT NOT NULL, params_json TEXT NOT NULL,
  seed INTEGER, prompt_hash TEXT NOT NULL, prompt_text TEXT, prompt_compressed INTEGER DEFAULT 0,
  response_text TEXT, finish_reason TEXT,
  prompt_tokens INTEGER, completion_tokens INTEGER,
  latency_ms INTEGER, attempt INTEGER NOT NULL DEFAULT 1,
  error TEXT, created_at TEXT NOT NULL
);
CREATE INDEX idx_llm_call_prompt ON llm_call(prompt_hash, model);

CREATE TABLE translation_cache (
  prompt_hash TEXT NOT NULL, model TEXT NOT NULL, params_hash TEXT NOT NULL,
  target_lang TEXT NOT NULL, response_text TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (prompt_hash, model, params_hash, target_lang)
);

CREATE TABLE translation_memory (           -- reuse of identical blocks
  content_hash TEXT NOT NULL, model TEXT NOT NULL, target_lang TEXT NOT NULL,
  text_md TEXT NOT NULL, hits INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL,
  PRIMARY KEY (content_hash, model, target_lang)
);
