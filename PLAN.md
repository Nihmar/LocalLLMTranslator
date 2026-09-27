# LocalLLMTranslator — Architecture and Implementation Plan

> Status: **M0–M7 complete.** This document is the source of truth for the architecture; the code
> follows it milestone by milestone.

---

## 1. Decisions made

| Scope | Choice | Rationale |
|---|---|---|
| Desktop shell | **Tauri 2** (Rust) | ~10 MB binary, low idle RAM, native cross-platform packaging (deb/AppImage/dmg/msi) |
| Documents | **Python sidecar** (PyInstaller onedir) | PyMuPDF/ebooklib/lxml are the mature ecosystem for EPUB/PDF; rewriting it in Rust is weeks of work at lower quality |
| IPC | **stdio, NDJSON, JSON-RPC 2.0** | No port to allocate, no token to manage, no additional network surface; the sidecar is auto-restarted by the Rust supervisor |
| LLM engine | **External `llama-server`** | The user starts and tunes the servers; the app detects endpoints, health, models and slots |
| State | **SQLite** (sqlx + embedded migrations) | Checkpoints, resume, cache, audit, job queue |
| PDF | **Pluggable interface**: `pymupdf4llm` default, `marker` optional | Default without heavy dependencies; marker can be enabled where layout quality matters |
| Frontend | React + TS + Vite + Tailwind + **CodeMirror 6** | CodeMirror 6 + `@codemirror/merge` renders the review diff (a read-only merge view in a dark theme built from the design tokens); the app needs no diff implementation of its own. |
| Tests | pytest + ruff + pyright (Python), cargo test + clippy + rustfmt (Rust), **fake llama-server** | Deterministic, offline CI |

**Product constraints**: no telemetry, no network calls except the configured endpoints,
user-editable prompts and templates, everything working offline.

### Environment verified on the development machine

| Component | Status |
|---|---|
| Python | 3.12.13 available via `uv` (preferred; `marker`/`torch` have no wheels for 3.14) |
| Rust / Node | cargo 1.97.1, node v26.8.2 |
| Pandoc | 3.10.2 |
| Tauri Linux dependencies | webkit2gtk-4.1, gtk+-3.0, libsoup-3.0, javascriptcoregtk-4.1 → all present |
| GPU | AMD Radeon RX 9060 XT (Navi 44) — **no NVIDIA**, VRAM readable from `/sys/class/drm/card1/device/mem_info_vram_{used,total}`; `rocm-smi` present in `/opt/rocm/bin` |
| `just` | **not installed** → a `Makefile` is used |

Consequence: VRAM detection must try **sysfs → rocm-smi → nvidia-smi**, in this order, and
degrade to "unknown" without errors.

---

## 2. Architecture

```
┌────────────────────────────── Tauri 2 (Rust) ──────────────────────────────┐
│  UI Bridge (commands + events)                                              │
│                                                                             │
│  ┌──────────────┐  ┌───────────────┐  ┌────────────────┐  ┌─────────────┐  │
│  │  Scheduler   │  │ ResourceGov.  │  │ ContextBuilder │  │  QA Engine  │  │
│  │ queue+lease  │  │ VRAM/slot cap │  │ budget+prompt  │  │  checks     │  │
│  └──────┬───────┘  └───────┬───────┘  └────────┬───────┘  └──────┬──────┘  │
│         └──────────────────┴───────────┬───────┴─────────────────┘         │
│                                        │                                    │
│  ┌──────────────┐  ┌───────────────┐  ┌▼───────────────┐  ┌─────────────┐  │
│  │  SQLite repo │  │ Pandoc Driver │  │  LlamaClient   │  │ Sidecar Sup.│  │
│  │  (sqlx)      │  │  (subprocess) │  │ HTTP + SSE     │  │  (stdio)    │  │
│  └──────────────┘  └───────┬───────┘  └────────┬───────┘  └──────┬──────┘  │
└────────────────────────────┼───────────────────┼─────────────────┼─────────┘
                             │                   │                 │
                      pandoc binary        llama-server ×N     Python sidecar
                      (+ Lua filters)      (external)          (NDJSON RPC)
                                                                     │
                                    ┌────────────────────────────────┴──────┐
                                    │ extractors · markdown_ir · chunker    │
                                    │ placeholders · pandoc bridge · qa     │
                                    └───────────────────────────────────────┘
```

### Data flow

```
source (EPUB/PDF/MD)
  └─▶ [sidecar] extractor ─▶ canonical markdown + YAML front-matter
        └─▶ [sidecar] markdown_ir ─▶ Block[] with stable ids
              └─▶ [sidecar] chunker ─▶ Chunk[] (block_ids, token_estimate)
                    └─▶ [sidecar] placeholder split ─▶ llm_text + PlaceholderMap
                          └─▶ [Rust] ContextBuilder ─▶ prompt (stable prefix)
                                └─▶ [Rust] LlamaClient SSE ─▶ translated text
                                      └─▶ [sidecar] reinject + re-parse + alignment
                                            └─▶ block_translation[] (original↔translated map)
                                                  ├─▶ editor (bilingual)  ─▶ suggestions
                                                  ├─▶ proofreader (target) ─▶ polish
                                                  └─▶ QA checks ─▶ qa_finding[]
                                                        └─▶ [Rust] Pandoc Driver ─▶ PDF/EPUB/DOCX
```

### Why the Rust/Python boundary is where it is

- **Rust = control plane**: window, DB, queue, lease, resource budget, HTTP/SSE calls,
  orchestration of the phases, events towards the UI. It is the part with concurrency and
  state, where Rust provides guarantees.
- **Python = data plane**: file formats, Markdown IR, chunking, placeholders, Pandoc,
  QA heuristics. They are **pure** functions (`input → output`): if the sidecar dies, every
  request is repeatable with no side effects.
- Consequence: the sidecar never touches the DB and knows nothing about the queue. A sidecar
  crash does not compromise the state.

---

## 3. Repository structure

```
LocalLLMTranslator/
├── README.md                      # rewritten from scratch
├── PLAN.md                        # this document
├── AGENTS.md                      # conventions for agents
├── Makefile                       # dev, test, lint, build, bundle (just not installed)
├── .github/workflows/ci.yml
├── crates/app/                    # Tauri 2
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   ├── icons/
│   ├── migrations/                # 0001_init.sql, ...
│   └── src/
│       ├── main.rs / lib.rs
│       ├── commands/              # IPC surface towards the UI
│       ├── db/{mod,repo}.rs
│       ├── llm/{client,models,health,slots}.rs
│       ├── scheduler/{queue,lease,worker}.rs
│       ├── resources/{vram,gpu}.rs
│       ├── context/{builder,glossary,memory,budget}.rs
│       ├── pipeline/{ingest,translate,review,export,qa}.rs
│       ├── sidecar/{supervisor,rpc}.rs
│       └── pandoc.rs
├── ui/                            # React + TS + Vite
│   └── src/
│       ├── routes/{wizard,jobs,review,export,settings}
│       ├── components/{ChunkTable,DiffEditor,LogView,ResourceGauge}
│       └── lib/{ipc,events,queries}
├── sidecar/
│   ├── pyproject.toml
│   ├── llmtranslator_sidecar/
│   │   ├── __main__.py            # JSON-RPC server over stdio
│   │   ├── rpc.py                 # transport, dispatch, error codes
│   │   ├── errors.py              # shared domain errors (1001/1002/1003)
│   │   ├── extractors/{__init__,base,epub,pdf_pymupdf,pdf_marker,markdown_src}.py
│   │   ├── blocks.py              # Block, Chapter, Chunk
│   │   ├── parse.py               # source-preserving segmentation
│   │   ├── serialize.py           # byte-exact reconstruction and render
│   │   ├── placeholders.py
│   │   ├── chunker.py
│   │   ├── pandoc.py
│   │   └── qa.py
│   └── tests/
├── prompts/                       # shipped defaults, copied and made editable
│   ├── translator.md translator.table.md
│   ├── editor.md proofreader.md
│   ├── summarizer.md orchestrator.md
│   └── analyze_book.md            # book reconnaissance (§9.4, M3)
├── pandoc/
│   ├── templates/{book.tex,book.html}
│   ├── filters/{footnotes.lua,tables.lua,epub_cleanup.lua}
│   └── styles/{book.css,book.tex}
└── tools/
    ├── fake_llama_server.py       # deterministic OpenAI-compatible server for the tests
    └── make_fixtures.py           # generates test EPUBs/PDFs
```

---

## 4. Document model: blocks, chunks, placeholders

This is the part that decides whether the structure survives three LLM passes.
**Rule: flat text never crosses the pipeline; it always crosses an indexed structure.**

### 4.1 `Block` — atomic unit with a stable ID

```python
@dataclass
class Block:
    id: str            # "b000417" — deterministic sequence number from the document order
    chapter_id: str
    order: int
    kind: str          # heading|para|list|blockquote|table|code|figure|footnote_def|frontmatter|hr|html
    level: int         # heading level or list nesting
    source_md: str     # exact Markdown slice
    source_text: str   # text actually sent to the model (block markers removed where needed)
    translatable: bool
    attrs: dict        # {"text_prefix": "## ", "ordered": true, "align": [...], "lang": "sql", "ref": "^3"}
    content_hash: str  # sha256(canonical source_text) — for cache and change detection
```

- **Stable IDs**: `b{order:06d}`. Deterministic, they survive resume, export/import and
  re-extraction of the same file. No random UUIDs.
- **Non-translatable**: `code`, `hr`, `html`, `figure`. They stay in the flow; the chunker
  passes through them without sending them.
- **Tables**: a single `Block` with `kind="table"`, full `source_md`, dedicated prompt.

**Source-guided segmentation, not AST-guided.** The parser is a line-scanner that preserves
the exact slices of the original Markdown, so `serialize(parse(md)) == md` is a testable
invariant. A parser that produces an AST and re-serializes it would normalize the Markdown,
breaking fidelity on non-translated blocks.

Segmentation units: fenced code block (with info string), ATX heading, GFM table
(cell row + delimiter row), contiguous list (all items, markers preserved),
contiguous blockquote, footnote definition, HTML block, horizontal rule, paragraph.

Granularity choices for the MVP, with rationale:

- **Whole list = one block** (not one block per item): translating the list as a unit keeps the
  number of items and the markers stable and leaves the cross-references between items to the
  model. The chunker will be able to split lists that are too large by item in a later milestone.
- **Whole blockquote = one block**, same reasoning.
- **Heading**: the `#` are removed from the text sent and rebuilt from `attrs.text_prefix`,
  so the model cannot change the heading level.

**An extractor must not hand the IR a paragraph that reads as another block.** Publishers mark
dialogue with a leading dash inside a paragraph (`<p><span>-</span> <span>…</span></p>`), which
becomes `- Bonjour` in Markdown — and this parser, correctly, reads that as a *list*. The chunk
then declares list blocks, and a translation that renders the same dialogue as prose comes back
with a different block count and is refused for ever. The converters escape the marker
(`\- Bonjour`), the standard Markdown escape, so the paragraph stays a paragraph while the dash
still renders as the dash the book printed. The same rule covers a leading `+`, `*`, `#`, `>`,
`|` and an ordered item, whose delimiter is what gets escaped (`1\.`).

Two distinct functions:

- `serialize(blocks) -> str` — **identical** reconstruction of the source (invariant + documents
  without translation). It requires preserving, for each block, the number of blank lines that
  separated it from the next one.
- `render(blocks, translations) -> str` — construction of the output: for each translatable block
  with an available translation it emits `attrs.text_prefix + translation`, otherwise it emits
  the unchanged `source_md`.

### 4.2 Inline placeholders — why the Markdown does not get corrupted

Before sending text to the model, inline elements are replaced with opaque tokens `⟦n⟧`:

| Source | Text sent | Map |
|---|---|---|
| `**The city**` | `⟦1⟧The city⟦2⟧` | 1=`**`, 2=`**` (delimiters) |
| `[the king](https://x.org/a)` | `⟦3⟧the king⟦4⟧` | 3=`[`, 4=`](https://x.org/a)` — **the URL never enters the prompt** |
| `` `x = 1` `` | `⟦5⟧` | 5=opaque span, text unchanged |
| `[^3]` | `⟦6⟧` | 6=footnote reference |
| `$E=mc^2$` | `⟦7⟧` | 7=opaque math |
| `\- Bonjour` (escaped dialogue dash, §4.1) | `⟦8⟧ Bonjour` | 8=`\-` — a line-initial marker must not reach the model |

Advantages: URLs, inline code, math and notes cannot be translated or corrupted; the model
sees only prose. The map is a **pure function** of `source_text`, so it does not need to be
persisted — it is regenerated identically. The escaped dialogue dash belongs here for the same
reason: shown `\- Bonjour` the model answers `- Bonjour`, which Markdown reads as a list, and
that is not the paragraph the chunk declared — measured on a real book, the answer came back with
66 blocks against the 47 the chunk held.

Post-translation validation: every placeholder must appear **exactly once**. If it is missing
or duplicated → retry with a message listing the missing tokens → then fallback to
"raw Markdown, preserve the formatting" → then `qa_finding` for manual review. If the set
is complete but the order is altered, automatic repair by reordering by index.

### 4.3 `Chunk` — unit of work and of checkpoint

```python
@dataclass
class Chunk:
    id: str                 # "c000123"
    chapter_id: str
    order: int
    block_ids: list[str]    # contained blocks, in order
    source_md: str          # recomposition of the blocks
    token_estimate: int
    context_carrier: dict   # current heading chain, to orient the model
    flags: list[str]        # ["table", "table_part:2/3", "continues", "oversized"]
```

A chunk is **never** a cut at a fixed number of characters: it is always a list of whole blocks.

---

## 5. SQLite schema

```sql
CREATE TABLE project (
  id TEXT PRIMARY KEY, name TEXT NOT NULL,
  source_path TEXT NOT NULL, source_hash TEXT NOT NULL,
  source_format TEXT NOT NULL, source_lang TEXT, target_lang TEXT NOT NULL,
  doc_title TEXT, doc_author TEXT,
  series_id TEXT REFERENCES series(id) ON DELETE SET NULL,
  series_order INTEGER,
  prompts_snapshot_dir TEXT,            -- copy of the prompts used → reproducibility
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

CREATE TABLE block_translation (            -- original ↔ translation map
  block_id TEXT NOT NULL REFERENCES block(id) ON DELETE CASCADE,
  chunk_id TEXT NOT NULL REFERENCES chunk(id) ON DELETE CASCADE,
  text_md TEXT NOT NULL, placeholders_ok INTEGER NOT NULL DEFAULT 1,
  origin TEXT NOT NULL,                     -- translator|editor|proofreader|user
  edited_by_user INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL,
  PRIMARY KEY (block_id, origin)
);

CREATE TABLE suggestion (                   -- editor/proofreader proposals
  id TEXT PRIMARY KEY, chunk_id TEXT NOT NULL REFERENCES chunk(id) ON DELETE CASCADE,
  pass TEXT NOT NULL,                       -- editor|proofreader
  block_id TEXT, field TEXT, original TEXT, proposed TEXT,
  reason TEXT, severity TEXT, quote TEXT,
  status TEXT NOT NULL DEFAULT 'pending',   -- pending|accepted|rejected|superseded
  created_at TEXT NOT NULL,                 -- when the pass proposed the change
  decided_at TEXT                           -- when the user decided; NULL while pending
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

-- ---- series: the shared canon (PLAN.md §9.5) --------------------------------

CREATE TABLE series (
  id TEXT PRIMARY KEY, name TEXT NOT NULL,
  source_lang TEXT, target_lang TEXT,       -- pinned pair every member book shares
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

CREATE TABLE series_glossary_variant (      -- surface forms: "the Keeper", "Keeper's"
  id TEXT PRIMARY KEY,
  term_id TEXT NOT NULL REFERENCES series_glossary_term(id) ON DELETE CASCADE,
  text TEXT NOT NULL,
  UNIQUE(term_id, text)
);

CREATE TABLE series_memory (
  series_id TEXT NOT NULL REFERENCES series(id) ON DELETE CASCADE,
  key TEXT NOT NULL, value TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1, updated_at TEXT NOT NULL,
  PRIMARY KEY (series_id, key)              -- style_guide, synopsis, canon
);

CREATE TABLE project_memory (
  project_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1, updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, key)             -- synopsis, style_guide, rolling_summary, decisions
);

CREATE TABLE llm_endpoint (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, base_url TEXT NOT NULL,
  api_key_ref TEXT,                         -- keyring name, never the secret in plaintext
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
  kind TEXT NOT NULL,                       -- ingest|translate_chunk|book_recon|series_recon|
                                            -- summarize|edit_chunk|proofread_chunk|qa_scan|export_unit
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
  glossary_hash TEXT NOT NULL DEFAULT '',   -- effective glossary the block was made with
  text_md TEXT NOT NULL, hits INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL,
  PRIMARY KEY (content_hash, model, target_lang, glossary_hash)
);
```

Two-level cache: `translation_cache` is exact on the full prompt (copy-paste on
retry/resume), `translation_memory` reuses identical blocks already accepted (running heads,
repeated paragraphs, title pages) without even calling the model.

---

## 6. Jobs, checkpoints, idempotency

- **Claim**: `UPDATE job SET state='leased', lease_owner=?, lease_expires_at=now+90s WHERE id =
  (SELECT id FROM job WHERE state='pending' AND run_after<=now ORDER BY priority, created_at
  LIMIT 1) RETURNING *`. Lease renewed with a heartbeat; expired → the reaper puts it back to
  `pending` with `attempts+1`.
- **Idempotency**: every result write is an upsert keyed on the work identity
  (`chunk_id` + `prompt_hash`). Re-running an already completed job has no effect.
- **A refused attempt never destroys an accepted one**: when the answer cannot be aligned with
  the source the *new* answer is dropped, the chunk is flagged `needs_review` with the reason,
  but a translation already validated for that chunk stays in `target_md` (the column is only
  left empty when there was nothing to keep). The rejected answer is preserved in
  `llm_call.response_text`, so the review shows both what failed and what is on record.
- **No duplicate work**: `translation_start` skips the chunks that already carry an unfinished
  `translate_chunk` job, so "Avvia / Riprendi" resumes a queue instead of queueing the same chunk
  a second time (a duplicate would translate a chunk that is `done` by then). `job_cancel` is the
  narrow counterpart: a queued job's chunk is left alone, only the `running` one goes back to
  `pending`.
- **Crash recovery**: on startup the reaper frees the expired leases; ingestion and export write
  to temporary files and do an atomic `rename`; the sidecar is restarted and the in-flight
  requests (pure) are re-sent.
- **Selective resumption**: you can re-run a single chunk, a chapter, or "all chunks
  `failed`/`needs_review`".
- **Project export/import**: `.llmtz` is a ZIP with `manifest.json` (format version, app
  version, exported-at), `project.sqlite` (a `VACUUM INTO` copy of the app database), the
  project's `work/` directory (Markdown + assets), its `output/` directory and the `prompts/`
  snapshot. `project_export` writes it; `project_import` extracts it under the project data
  directory, copies the project-owned rows out of the attached archive database and rewrites the
  absolute paths (`document.markdown_path`, `project.prompts_snapshot_dir`) to the local
  locations. An id that already exists is rejected and no other project's rows are touched.

---

## 7. LLM layer

### 7.1 Client

`LlamaClient` (Rust) on `reqwest`:

- `health()` → `GET /health`
- `props()` → `GET /props` → `total_slots`, `n_ctx`, `default_generation_settings`, `model_path`
- `models()` → `GET /v1/models`
- `tokenize(text)` → `POST /tokenize` (**exact token count**, not heuristic), with a heuristic
  fallback `len/3.5` when the endpoint does not expose it
- `chat_stream(req) -> Stream<Delta>` → `POST /v1/chat/completions` with `stream: true`, SSE
  parsing, support for `response_format: json_schema` and `grammar` (GBNF) for editor and proofreader

A `Delta` carries the answer (`content`) and the **thinking** (`reasoning_content`) separately: a
reasoning model streams its reasoning first, and mixing the two would feed the thinking to the
translator as if it were text. The reasoning is never parsed as the answer; it is logged as a size
and stored in `llm_call.reasoning_text`, which is what makes an empty answer explainable.

`chat_call` is the single entry point for a role call. It forwards the sampling parameters of the
binding plus `chat_template_kwargs` verbatim, the per-request channel for chat-template variables:
that is how a role turns the thinking off (`{"enable_thinking": false}`) without touching the
server. A **structured** pass (`response_format: json_schema`) goes through `run_structured_call`,
which retries once, with an explicit instruction, when the answer carries no JSON object at all —
and doubles the app's own token default for that retry when the first attempt ran out of budget,
never overriding a `max_tokens` the binding set. A reply that was merely truncated (a `{` without
its closing brace) is not retried: the same call would truncate again. The retry's prompt hash
differs, so both attempts stay in `llm_call`.

A rejected answer gains the outcome of the call — stop reason, answer and reasoning sizes, budget —
which is safe in the log file and in the UI because it never quotes the text.

Concurrency limit per endpoint = `min(role_binding.max_concurrency, props.total_slots)`.

### 7.2 Prefix stability (important for performance)

`llama-server` reuses the KV cache of the common prefix between requests. The prompt is
therefore structured **on purpose**:

- **System message byte-identical for the whole book**: role, rules, `style_guide`, book
  metadata → guaranteed cache hit.
- **Volatile user message**: `chapter_title`, `heading_chain`, `chapter_summary_so_far`,
  `previous_context`, the filtered glossary, the synopsis and the chunk flags (`text` is the
  passage itself) → it is the only part that invalidates the cache.

The glossary lives in the user message, not in the system one. §9.2 requires it to be filtered
to the terms present in each chunk, and a per-chunk glossary would make the system message
differ between chunks, destroying the stable prefix §7.2 exists to protect. The trade-off is
deliberate: the KV-cache prefix is preserved, while the glossary costs only the few terms a
chunk actually contains.

Effect: from the second chunk onwards the prefill costs almost nothing. Practical consequence:
**never** put timestamps, `chunk_id` or counters in the system message.

### 7.3 Recommended server configuration (documented in the README)

```sh
# Translator — large context, 4 parallel slots
llama-server -m models/qwen2.5-32b-instruct-q5_k_m.gguf \
  --host 127.0.0.1 --port 8080 -c 32768 --parallel 4 --cont-batching \
  --cache-reuse 256 --jinja --n-gpu-layers 999 --flash-attn --metrics

# Editor / proofreader — smaller model, same server or second endpoint
llama-server -m models/qwen2.5-14b-instruct-q6_k.gguf \
  --host 127.0.0.1 --port 8081 -c 16384 --parallel 2 --cont-batching --jinja
```

---

## 8. Prompt templates

**Jinja2** format (rendered in Rust with `minijinja`, so Python and Rust use the same files).
Defaults in `prompts/`, copied into the project snapshot and made editable from the UI.

### `prompts/translator.md` (system — stable)

```jinja
You are a professional literary translator. You translate from {{ source_language }} into {{ target_language }}.

HARD RULES
1. Output ONLY the translation of the passage in the user message. No commentary, no notes, no preface, no code fences around the result.
2. Preserve Markdown structure exactly: same number of lines and paragraphs, same list markers, same heading levels, same blank-line separation.
3. Tokens like ⟦12⟧ are placeholders. Copy each one verbatim, exactly once, in a position that is grammatical in {{ target_language }}. Never translate, split, merge, renumber or reorder them.
4. Never translate: fenced code blocks, inline code, URLs, DOIs, file paths, email addresses.
5. Use the GLOSSARY exactly as given whenever the source term occurs.
6. Do not summarise, do not omit sentences, do not merge or split paragraphs, do not add sentences that are not in the source.
7. Keep the source's paragraph rhythm and register; translate idioms into natural {{ target_language }}, not word-for-word.

STYLE GUIDE
{{ style_guide }}

BOOK
Title: {{ book_title }}
Author: {{ book_author }}
```

### `prompts/translator.md` (user — volatile)

The glossary and the synopsis are here, not in the system half: the glossary is filtered to the
terms present in each chunk, so it changes from chunk to chunk (see §7.2 and §9.2).

```jinja
CHAPTER: {{ chapter_title }}
SECTION: {{ heading_chain }}

CHAPTER SUMMARY SO FAR
{{ chapter_summary_so_far }}

GLOSSARY (source => target)
{{ glossary }}

SYNOPSIS
{{ synopsis }}

PREVIOUS PASSAGE (already translated — for continuity of tone, pronouns and terminology only; do NOT translate it):
{{ previous_context }}

NOTE: {{ chunk_flags }}

PASSAGE TO TRANSLATE:
{{ text }}
```

### `prompts/translator.table.md`

A default for a dedicated table prompt (`TABLE RULES` below). **Not wired yet**: the chunker
flags split tables (`table_part:i/n`) and the translator prompt states that in `chunk_flags`,
but every chunk is still rendered with `translator.md`. Wiring the dedicated prompt is future
work.

```
TABLE RULES
- Translate only the cell contents. Keep the header separator row (|---|---|) and its alignment colons exactly as they are.
- Keep the same number of rows and of pipe characters per row.
- Escape any literal pipe inside a cell as \|.
- Output the table only.
```

### `prompts/editor.md` (system)

```jinja
You are a bilingual revision editor for a {{ source_language }} → {{ target_language }} book translation.
You compare SOURCE and TRANSLATION and report only real defects: mistranslation, omission, addition,
terminology violation, register break, broken Markdown, broken or moved placeholder.
You do not rewrite for taste. You propose the smallest correction that fixes the defect.
Report an issue only if you are confident; an empty issue list is a valid answer.
Reply with JSON only.
```

Response schema (`response_format: json_schema`):

```json
{"type":"object","properties":{
  "verdict":{"enum":["ok","needs_fix"]},
  "issues":{"type":"array","items":{"type":"object","properties":{
    "block_index":{"type":"integer"},
    "severity":{"enum":["critical","major","minor"]},
    "kind":{"enum":["meaning","omission","addition","terminology","register","markup","placeholder"]},
    "quote":{"type":"string"},"suggested":{"type":"string"},"reason":{"type":"string"}
  },"required":["block_index","kind","quote","suggested","reason"]}}},
 "required":["verdict","issues"]}
```

### `prompts/proofreader.md` (target language only)

```jinja
You are a monolingual proofreader for {{ target_language }}.
The text was translated from {{ source_language }} and reads slightly foreign.
Fix grammar, agreement, punctuation, calques, false friends and unnatural collocations.
Do NOT change meaning. Do NOT add or remove content. Do NOT touch placeholders ⟦n⟧.
Do NOT alter Markdown structure, code spans, URLs or table pipes.
Output only the corrected text, with no commentary and no code fences.
```

### `prompts/summarizer.md` (rolling memory — orchestrator role)

```jinja
You maintain the memory of a translation project ({{ source_language }} → {{ target_language }}).
From the chapter excerpt below produce JSON only:
{"summary": "3-5 sentences in {{ target_language }}",
 "new_terms": [{"source":"","target":"","kind":"term|proper_noun|do_not_translate","note":""}],
 "style_notes": ["short observations about register, recurring constructions, forms of address"]}
Output at most 8 new_terms, only terms that recur or matter.
```

The `new_terms` enter `glossary_term` as `status='candidate'`: confirmation is up to the user
(or automatic, if configured). It is the mechanism that satisfies "memory of the terminological
choices already made".

Mechanics: a `summarize` job runs on the `orchestrator` role after every 5 completed chunks of a
chapter and once when the chapter has no unfinished chunk left. A rolling run writes
`project_memory['rolling_summary']`; the final run writes `chapter.summary` (the value the
context assembler reads for the next chapters) and clears the rolling one. `style_notes` are
stored as candidates under `project_memory['style_notes']` and only reach the style guide when
the user adds them. With no orchestrator binding the jobs are not enqueued at all and
translation continues exactly as before.

---

## 9. Chunking and context management

### 9.1 Chunking algorithm

1. Group blocks by chapter.
2. Accumulate blocks until `token_estimate ≤ budget` (budget = `n_ctx − prompt_reserve −
   output_reserve`, default 60% of `n_ctx` — a value read from `/props`, not from config).
3. **Never split** a table, a code block or a blockquote with its footnote; if the block
   straddles the limit, close the chunk first.
4. Table larger than the budget → split by rows with the **header repeated** in every part and
   flag `table_part:i/n`; the prompt states this explicitly.
5. Paragraph larger than the budget → split at sentence boundaries, flag `continues:true`, and the
   next chunk receives the tail of the previous one as context.
6. Each chunk carries `context_carrier`: the chain of headings (H1 → H2 → H3) it sits in.

**Example** (budget 6000 tokens):

| Block | Type | Token | Outcome |
|---|---|---|---|
| b000101 | h2 "Chapter 3 — The Siege" | 12 | chunk c000041 |
| b000102 | para | 480 | chunk c000041 |
| b000103 | blockquote + `[^3]` | 320 | chunk c000041 |
| b000104 | table 4×3 | 380 | chunk c000041 |
| b000105 | para | 1450 | chunk c000041 (total 2642) |
| b000106 | code ```sql | 210 | skipped, **not sent** |
| b000107 | para | 3900 | chunk c000042 (`continues` if split) |
| b000108 | para | 600 | chunk c000043 |

### 9.2 ContextAssembler — budget with priorities

With a given number of available tokens, the context is filled in this order and truncated from
the last:

| Priority | Component | Source | Persistence |
|---|---|---|---|
| 1 | System prompt + rules + style guide | `prompts/translator.md`, `project_memory.style_guide` | stable for the book |
| 2 | Chunk flags (table part, continuation, oversized) | `chunk.flags_json` | per chunk |
| 3 | Heading chain of the chunk | `chunk.context_json` (`context_carrier`) | per chunk |
| 4 | **Relevant** glossary (only the terms present in the text of this chunk) | `glossary_term` | stable, ordered |
| 5 | Book synopsis | `project_memory.synopsis` | stable |
| 6 | Summary of the previous chapters (window 3) | `chapter.summary` | per chapter |
| 7 | Summary of the current chapter up to here | generated every N chunks | growing |
| 8 | Tail of the last translated passage | runtime | volatile |

The two chunk-local pieces sit right after the required prefix: they are tiny and they orient
the model on the passage at hand (a table part must repeat its header; a continuation must read
as one sentence across the split).

Injecting the entire glossary would be a mistake: on a book with 400 terms it devours the
context. The "terms present in this chunk" filter is what makes the glossary scalable.

### 9.3 Context and cache

The `prompt_hash` covers the full prompt (system + user): the volatile context **is part of**
the hash, so two identical chunks in different contexts do not share the exact cache, and that
is correct. The cheap reuse happens at the level of `translation_memory` (identical block → same
translation, zero calls) and of the server's KV cache (identical system prefix).

### 9.4 Book reconnaissance — acquiring the style guide and the synopsis (M3)

An empty "style guide" box is a bad interface: the user does not know what to write, yet the
quality of a literary translation depends mostly on knowing the register, the period and the
audience of the book. M3 therefore opens with a *reconnaissance* step that produces a
**candidate** book profile, which the user confirms before any translation runs. The confirmed
profile is prefix 1 of the context budget: it is the stable head of the system message, so it
costs nothing in KV-cache terms and improves every chunk.

Sources, in order of preference — **all of them local**:

1. the book itself: the incipit and a few paragraphs per chapter, which is where register,
   narrative voice and recurring constructions actually live;
2. the metadata the extractor already produces (title/author/language from the EPUB OPF or the
   PDF document info) — the first consumer of `document.front_matter_json`, written since M1 and
   never read;
3. optionally, text the user **pastes** from a page they found themselves. The app never fetches
   it: "no network calls other than the configured `llama-server` endpoints" stays true.

The call runs on the `orchestrator` role with a user-editable template
(`prompts/analyze_book.md`) and a JSON schema — finally using the `response_format: json_schema`
support the LLM client has had since M1 — so the answer is structured:

```json
{"source_language": "", "genre": "", "audience": "", "era": "", "narrative_voice": "",
 "register": "", "style_notes": [""], "themes": [""], "synopsis": "3-5 sentences",
 "proper_nouns": [{"source": "", "kind": "proper_noun|do_not_translate", "note": ""}],
 "field_basis": {"genre": "from_text|inferred"}}
```

Rules that make it safe to inject into every prompt:

- **Everything is a candidate.** The profile is reviewed field by field; a field marked
  `inferred` never silently becomes a fact in the system prompt.
- **The length caps live in the schema** (synopsis ≤ 120 words, style guide ≤ 200): the profile
  is paid on every single chunk, so it cannot grow unbound.
- The injected block carries an explicit "context only — do not add content from it" clause,
  alongside rule 6 of the translator prompt.
- Confirmed values land where the builder already reads them: `style_guide` and `synopsis` in
  `project_memory`, the rest as a `book_meta` JSON document, accepted proper nouns as approved
  `glossary_term` rows (or `do_not_translate` entries). The provenance travels with the value.
- Reproducible like everything else: seeded, audited in `llm_call`, and snapshotted into the
  project.
- Degrades cleanly: with no orchestrator model bound the step is skipped and the fields stay
  editable by hand.
- IPC: `recon_start` enqueues a `book_recon` job and returns its id; `recon_get` returns the
  candidate profile, the confirmed `style_guide`/`synopsis`/`book_meta` values and the glossary;
  `recon_confirm` receives the edited profile plus the confirmed field keys and performs the writes.
  The candidate is stored under `project_memory['book_profile']` and nothing reaches the translator
  prompts until `recon_confirm` runs. Progress and failures travel on the existing `job://progress`
  and `log://line` events — no new sidecar method and no new event.

### 9.5 Series — a shared canon across books

A series (a fantasy saga, a trilogy, a collection with recurring characters) needs one
terminology that outlives a single book. The project glossary stays authoritative for its own
book, and a series adds a shared layer that **evolves after the books are translated**:

- **`series`** owns the pinned language pair (`source_lang`, `target_lang`) every member book
  shares, a name and free-form settings. `project.series_id` + `project.series_order` place a
  book in the saga; the series is optional (a standalone book is exactly what exists today).
- **Glossary resolution, no copies.** The prompt sees the *effective* glossary: project terms
  first, then series terms, with a project term **overriding** the series rendering for the same
  source. Nothing is copied at creation time, so editing a series term immediately reaches every
  book, and an override stays an explicit, per-book decision.
- **Aliases and variants.** `series_glossary_variant` lists the surface forms of a term
  (`Keeper`, `the Keeper`, `Keeper's`). Filtering for "the terms present in this chunk" matches
  the main source **or any variant** with the same word-boundary rule; the prompt still renders
  the canonical `source => target` pair, so the model is not asked to infer anything.
- **Series memory.** `series_memory` holds `style_guide`, `synopsis` and any canon document.
  They are budgeted below the book-level ones (a book's own style guide wins; the series
  synopsis sits under the book synopsis) and are visible to the reconnaissance as evidence to
  promote into.
- **Consistency when the canon changes.** Adding or editing a series term opens a
  `qa_finding(kind='glossary_conflict')` for every member book whose own glossary renders the
  same source differently, deduplicated per term; the book override keeps winning until the
  user resolves it. `translation_memory` rows carry the hash of the effective glossary they
  were produced with, so a glossary change stops reusing blocks translated under the old
  canon (the exact `translation_cache` already keys on `prompt_hash`, which includes the
  glossary text).
- **Bundles.** A `.llmtz` stays self-contained. Series export/import
  (`series_export`/`series_import`) moves the canon between machines: the archive carries
  `manifest.json`, `series.json` and — when the series has member books — **one** database
  snapshot plus a `projects/<id>/` tree per book, so the bundle stays proportional to the
  canon, not to the number of books. Import **merges** by revision: a missing term is added,
  the same rendering only updates its note/kind, a different rendering is kept with
  `status='conflict'` (and flags the member books) and is never silently dropped; memory
  values are taken only when newer. A book whose id already exists locally is skipped, so
  importing the same bundle twice is safe. Version 1 bundles (canon only) still import.
- No sidecar change: the series is control-plane state (DB + prompt assembly), and the sidecar
  keeps receiving the already-filtered glossary.

IPC: `series_list`, `series_create`, `series_get`, `series_update`, `series_delete`,
`project_set_series`, `series_glossary_list`, `series_glossary_upsert`,
`series_glossary_delete`, `series_variant_upsert`, `series_variant_delete`,
`series_promote_term`, `series_export`, `series_import`, `series_qa_scan`,
`series_recon_start` (`{req: {series_id, force?}}`), `series_recon_confirm`. Events: none new —
the existing `job://progress` and `log://line` cover the work, and conflicts are read
through `qa_report`.

Milestones (S1–S5) are in §13. All of them are implemented: the Series view authors the
canon, the bundle merge and the cross-book QA scan exist, and `series_recon` produces a
candidate profile from the member books' confirmed profiles (it is never injected into a
prompt until the user confirms it). `series_recon_confirm` applies the user's decisions
field by field: accepted values land in the series memory, accepted characters become
approved canon terms and rejected sources are remembered so a later run skips them. The run
is **incremental**: a book whose confirmed profile and the canon hash are unchanged does not
reach the model, and the previous candidate is fed back as context so an update keeps what
is still valid; `force` re-synthesizes everything. The job runs on the `orchestrator` role
and is attached to the first member book — the queue is project-scoped — with the
`series_id` in its payload.

---

## 10. Concurrency and sub-agents

- **Unit of parallelism**: the chunk. The queue is global, concurrency is per endpoint.
- **ResourceGovernor** (read-only, no driver dependency):
  - VRAM: sysfs (`/sys/class/drm/card*/device/mem_info_vram_{used,total}`) → `rocm-smi` →
    `nvidia-smi` → "unknown". *(on the development machine: AMD, therefore sysfs/rocm)*
  - Slots: `/props.total_slots`; fallback `llm_endpoint.max_concurrency`.
  - `max_parallel = min(free_slots, floor(VRAM_headroom / estimated_cost_per_slot), user_limit)`.
  - If `max_parallel < 2` → **degradation to serial without error**, with a UI event and an explicit reason.
  - **Per endpoint**: the pool claims a job only when the role's endpoint has a free slot. The
    limit is `min(max_concurrency, /props.total_slots)`, and `1` when the endpoint reports
    neither (serial, without error). The plan is built at boot from the role bindings with a
    live `/props` probe and the persisted props as fallback; `Local` jobs (sidecar/pandoc: ingest,
    export, QA scan) use the global cap. `metrics_get` and `metrics://tick` report the per-role
    limit, the in-flight count and the reason, so the UI can show why the sub-agents are capped.
- **Ordering**: priority = chapter order (sequential translation makes sense for
  coherence), the user's jobs at the front, the retries at the back (a requeued attempt takes a
  priority penalty so a failing chunk cannot starve the queue).
- **Concurrent glossary**: the sub-agents **propose** terms, they do not impose them.
  `glossary_term.revision` + `status='candidate'`; if two agents propose different renderings for
  the same term → both saved as `candidate` and a `qa_finding(kind='glossary_conflict')`
  to be resolved in the UI. No blocking lock, no lost writes.
  Merge rule: a proposal never overwrites a different existing rendering — the row becomes
  `status='conflict'` when it was a candidate (an approved row stays approved) and an open
  `qa_finding(kind='glossary_conflict')` records both renderings, deduplicated per source term.
  Edits coming from the UI carry `expected_revision` and fail loudly when another writer changed
  the row in the meantime, instead of silently overwriting it.
- **Determinism**: the seeding derives from `hash(chunk_id, role)` → stable across serial and
  parallel runs.

---

## 11. User interface

The numbered subsections below are stable anchors, not the on-screen order: the shell groups
destinations by **scope**. The **project steps** — Ingestion, Translation, Review and Export —
are the translation pipeline; the sidebar shows them only while a project is open, in reading
order, and each one is a freely visitable route (not a constraint). The **application
destinations** — Projects, Models, Series and the Job dashboard — are always reachable,
independent of the open project; they come first in the sidebar, so opening or closing a book
never moves the entries above. A project also reaches, beside its four steps, the **Glossario**:
the terms the translator prompt reads, reviewed as a destination of its own instead of inside a
collapsed panel. The open project is pinned in the header (name, language pair, source path), so
the current book is unambiguous on every page.

1. **Ingestion** — drag&drop, format detection, chapter and block preview, PDF backend
   choice, extraction result with warnings.
2. **Models** — endpoint CRUD (URL, health-check, model list from `/v1/models`, `props`),
   role assignment, savable profiles, VRAM/slot indicator.
   A binding is one row per (role, endpoint): assigning the same pair again updates that row
   instead of adding a twin, and `role_binding_delete` removes it, so a model can be unassigned
   from a role it was given earlier. A role may keep several bindings, ordered by priority
   (the highest wins in `role_binding_for`).
3. **Translation** — book profile panel (the reconnaissance result of §9.4, confirmed field by
   field) plus chunk table (`pending/running/done/failed/needs_review`) with tokens,
   attempts, model; start/pause/resume; live log; resource gauge; actions on multiple
   selection (retry, skip, re-translate with another model).
   Above the chunk table sits a chapter outline with the same aggregated statuses; double-clicking
   a chapter (or pressing Enter on its row, or its «Anteprima» button) opens a live preview
   composed client-side from the chunk rows already loaded — translated chunks show their target,
   the rest their source in grey — so a run can be followed chapter by chapter without waiting
   for the export. The status filter applies to the table only: counters, outline and preview
   always describe the whole project. The translation page also summarises what is running and
   opens the **job monitor** described below.
4. **Review** — 3-column side-by-side editor (original / translated / corrected) with block-level
   and character-level diff, navigation by suggestion, accept/reject per individual
   change, and a filterable QA report. The diff is a read-only CodeMirror 6 merge view
   (`@codemirror/merge`) with a dark theme built from the design tokens: the source block is a
   read-only markdown editor, the current translation and the selected proposal are the two
   sides of the merge view. Accept/reject is a control-plane operation that rewrites a block
   translation and recomposes the chunk, so the editor never mutates the text locally.
   Passes run as `edit_chunk` / `proofread_chunk` jobs; the QA heuristics run inline on every
   validated translation and can be re-run per chunk with `qa_scan` (for example after a glossary
   change). Accepting a suggestion rewrites the block with the pass as its origin and recomposes
   the chunk's `target_md`, so the exporter sees the reviewed text.
5. **Export** — per-chapter unit, `metadata.yaml`, template/CSS/LaTeX choice, preview,
   selective rebuild of only the modified chapter, build history.
   Mechanics: the composed units are hashed (content, metadata and the template/CSS/filters in
   use) and the last successful build is kept in `project_memory['export_state']`; an unchanged
   build is skipped and reported as such, `changed_units`/`reused_units` say what moved, and
   `chapter_id` builds one chapter standalone. The preview returns the composed markdown and the
   `metadata.yaml` without invoking Pandoc; the build history is the last ten records.
   `export_build` currently runs the build inline in the command; the `export_unit` job kind is
   dispatched by the worker but nothing enqueues it yet, so pausing the queue does not pause an
   export. Quoting the build through the queue is future work.

Plus: **Job dashboard** (per-chunk progress, ETA computed from the real throughput, log, resources)
and **Projects** (multiple, resume, export/import `.llmtz`).

The **Glossario** destination edits the same `glossary_term` rows as the book profile panel, with
room for the job: the terms that wait for a decision (candidates and conflicts) come first, a
status filter and a free-text search narrow the table, and the header counts what is pending. The
order and the filters read the **persisted** row, so approving a candidate does not move it while
the cursor is still in the row; the source identifies the row and is therefore not editable — a
rename is a new term plus a removal. One "Salva" writes every changed row with the revision it was
loaded from and reports a stale row instead of overwriting it (§9.2, §12.2).

The **job monitor** is a dialog, reachable from the header of every page and from the translation
page: it lists the jobs of the open project (or of every project, when the scope is widened), names
the chunk, its chapter, the elapsed time and the attempts behind each row, filters by state and
kind, and lets the user interrupt one job, a selection, or every unfinished one. Interrupting is
`job_cancel` (§12.2) and does **not** stop the queue; the dialog is deliberately not a page section
because the same question — what is running, and can I stop it — is asked from several views.

---

## 12. IPC contract

### 12.1 Sidecar (NDJSON over stdio, JSON-RPC 2.0)

Requests `{"jsonrpc":"2.0","id":N,"method":"...","params":{...}}`; responses `result` or
`error`; server→client notifications for progress. Planned methods:

| Method | Return |
|---|---|
| `ping` | `{pong, version, python, platform}` |
| `detect_format` | `{format, backends[]}` |
| `ingest` | `{markdown_path, metadata, chapters[], warnings[], assets_dir, assets[]}` |
| `parse_document` | `{blocks[], chapters[]}` |
| `build_chunks` | `{chunks[]}` |
| `prepare_text` | `{llm_text, placeholders[]}` |
| `reinject` | `{blocks_md[], placeholders_ok, missing[], duplicated[]}` |
| `qa_check` | `{findings[]}` |
| `pandoc_build` | `{units[{path,title}], metadata{}, output_path, output_format, template?, css?, resource_path[], toc?, lua_filters?, top_level_division?}` | `{output_path, log, duration_ms}` |
| `estimate_tokens` | `{counts[]}` (heuristic fallback, used if `/tokenize` is not available) |

The sidecar is **stateless** and does not touch the DB: every method is a pure function. This is
what makes it safe to restart it and re-send the in-flight requests.

### 12.2 Tauri (commands + events)

- Commands: `project_*`, `endpoint_*`, `role_binding_list`, `role_binding_set`, `role_binding_delete`,
  `ingest_start`, `translation_start/pause/resume/cancel`,
  `recon_start`, `recon_get`, `recon_confirm`, `glossary_list`, `glossary_upsert`, `glossary_delete`,
  `series_list`, `series_create`, `series_get`, `series_update`, `series_delete`, `project_set_series`,
  `series_glossary_list`, `series_glossary_upsert`, `series_glossary_delete`,
  `series_variant_upsert`, `series_variant_delete`, `series_promote_term`,
  `series_export`, `series_import`, `series_qa_scan`, `series_recon_start`, `series_recon_confirm`,
  `job_list`, `job_cancel`, `chunk_get`, `review_start`, `suggestion_list/accept/reject`, `qa_report`,
  `qa_finding_set_status`, `log_frontend_error`, `diagnostics_paths`, `diagnostics_export`,
  `export_build`, `export_preview`, `export_history`, `metrics_get`.
- Events: `job://progress`, `log://line`, `metrics://tick`, `sidecar://status`,
  `sidecar://progress`, `export://progress`. Findings are not pushed: the UI reads them
  through `qa_report` and refetches when a `job://progress` transition says a chunk moved.

`job://progress` carries the serialized `job` row at every transition the control plane owns;
views treat it as an invalidation trigger and refetch through commands. `job_cancel` takes
`{job_ids[]}` and interrupts those jobs **without stopping the queue**: a running job has its
in-flight call abandoned (the worker polls a per-job cancellation flag and re-reads the row before
writing an outcome, so a cancelled row is never revived by `complete` or `retry_or_fail`), a queued
one is cancelled in the database, and an interrupted `translate_chunk` returns its chunk to
`pending` — only when that chunk is `running`, so a merely queued job never touches the chunk it
pointed at — so a later `translation_start` picks it up again. The result reports `cancelled[]` and
`skipped[]` (ids that had already finished). This is deliberately narrower than
`translation_cancel`, which pauses the pool and aborts every worker. `sidecar://progress`
forwards the sidecar's out-of-band `progress` notifications unchanged, and `log://line`
(`{ts, level, source, message}`) carries the sidecar's stderr and the worker's job transitions
(start, completion) and failures, so the live log pane shows activity during a run.

API keys: `llm_endpoint.api_key_ref` stores only the *name* of the keyring entry and
`LlamaClient` accepts a bearer key, but no code reads the OS keyring yet, so an endpoint that
requires authentication is not usable today. Wiring the keyring lookup is future work; the
no-secrets-in-the-database rule already holds.

Diagnostics: the control plane writes structured `tracing` events to stdout and to a daily
file under the app data dir (`logs/llmtz.<date>.log`); the frontend reports every failed
command through `log_frontend_error`, so the file mirrors what the user saw. The log never
carries book text, prompts, responses or glossary values. `diagnostics_export` bundles the
newest logs and a `report.json` (versions, sidecar/worker state, queue, failed jobs,
model-call errors) and nothing else: no database, no project files.

---

## 13. Milestones

| # | Content | Acceptance criterion |
|---|---|---|
| **M0** | Repo, `Makefile`, CI (ruff/pyright/pytest + cargo fmt/clippy/test), Tauri window, sidecar with `ping`, SQLite migrations, `AGENTS.md`, rewritten README | `make check` green, empty app that opens on Linux |
| **M1** | **Walking skeleton end-to-end**: EPUB → Markdown → `Block[]` → `Chunk[]` → placeholder → translation with only the *translator* role → persistence + resume → Pandoc → EPUB/PDF. Minimal UI: create project, choose file, endpoint, start, progress, open output | Translated EPUB; interruption halfway and resume with no loss of structure; PDF and EPUB generated |
| **M2** | Robust ingestion: PDF `pymupdf4llm` behind the `PdfExtractor` interface, footnotes, plates, images, YAML front matter, stable anchors and IDs, optional `pdf_marker.py` | Footnotes and images present and intact in the output; stable IDs between two extractions |
| **M3** | Context and memory: glossary, synopsis, rolling summaries, style guide, ContextAssembler with budget, `/tokenize`, `translation_cache` + `translation_memory`, **book reconnaissance** (§9.4: a candidate book profile the user confirms before translating) | Repeated chunks do not call the model; the glossary appears in the prompt only for the terms present; the confirmed book profile is the stable head of every prompt |
| **M4** | Bilingual review and QA: JSON editor, proofreader, diff UI, accept/reject per change, QA report (untranslated, glossary inconsistencies, broken placeholders, anomalous lengths, malformed Markdown) | The report correctly flags untranslated text and inconsistencies on a controlled test |
| **M5** | Export and typesetting: per-chapter split, `metadata.yaml`, Pandoc/CSS/LaTeX templates, Lua filters for footnotes and tables, preview, selective rebuild | Readable PDF and EPUB with table of contents, footnotes and images |
| **M6** | Concurrency and sub-agents: queue with lease, ResourceGovernor, serial degradation, glossary with optimistic lock and merge | The sub-agents activate only with sufficient VRAM/slots and degrade without errors |
| **M7** | Packaging: PyInstaller onedir, Tauri bundle (deb/AppImage/dmg/msi), guided first launch, export/import `.llmtz`, docs | AppImage and bundle launchable on a clean machine |
| **S1** | Series entity + glossary resolution: `series`, `series_glossary_term`, variants, `project.series_id`, effective glossary (project overrides series), glossary hash in `translation_memory` | Two books of the same series translate a recurring term identically; a book override wins and is documented |
| **S2** | Series glossary lifecycle: promotion from a book, cross-book conflict findings, `series_glossary_*`/`series_promote_term` commands, Series UI (list, glossary, conflicts) | Changing a series term after two books are translated surfaces a resolvable conflict on the old book |
| **S3** | Series memory: `series_memory` style guide and synopsis injected below the book-level ones; merge from `book_recon` into the series | Both books of the series prompt with the shared profile without losing their own |
| **S4** | Consistency at scale: alias/variant matching in the filter, cross-book QA scan, series export/import with merge-by-revision | A term rename is auditable across every member book, and importing a series bundle never drops a differing rendering |
| **S5** | Optional `series_recon` job on the orchestrator role: series synopsis and canonical character sheet | — |

Dependencies: M1 unblocks everything; M3 comes before M4 (the editor uses the glossary and
context); M6 after M3 (concurrency requires the versioned glossary).

---

## 14. Verification

- **`tools/fake_llama_server.py`**: deterministic OpenAI-compatible server
  (`/v1/models`, `/v1/chat/completions` with SSE, `/props`, `/tokenize`, `/health`). Fake
  translations but structurally faithful; it can inject faults (placeholder drops, truncations,
  timeouts) to exercise the error branches. It is the basis of the offline CI.
- **Fixtures**: `tools/make_fixtures.py` generates test EPUBs and PDFs with tables, footnotes, images,
  code blocks and nested chapters; a "large" EPUB (~1M characters) for the robustness and
  memory tests.
- **Key invariant**: `serialize(parse(md)) == md` on all the fixtures — it is the test that
  guarantees the IR does not normalize the Markdown.
- **Python tests**: `test_markdown_ir` (round-trip), `test_placeholders` (round-trip and
  repair), `test_chunker` (no block lost, no table split, token sum),
  `test_pandoc`, `test_qa`.
- **Rust tests**: `test_queue` (claim, expired lease, retry, concurrency), `test_context_builder`
  (budget and priorities), `test_cache` (hit/miss), `test_resources` (serial degradation with a
  fake VRAM profile), `test_scheduler` (idempotency on re-run), `test_recon` (candidate profile →
  confirmation → what the context builder reads), `test_summarize` (rolling cadence, candidate
  terms, an approved term never demoted), `test_review` (editor/proofreader passes, accept,
  reject, recompose), `test_qa` (a controlled chunk reports glossary_mismatch, untranslated and
  empty).
- **End-to-end integration tests**: EPUB → translation (fake server) → export; interruption
  halfway via `SIGTERM` and resume; verification that the translated Markdown has the same sequence
  of block types as the original.
- **Manual**: translate a real ~300-page EPUB with a real `llama-server`, interrupt,
  resume, export PDF and EPUB.

---

## 15. Risks and mitigations

| Risk | Mitigation |
|---|---|
| The model loses placeholders or rewrites the structure | Structural validation per chunk + targeted retry + fallback to raw Markdown + `qa_finding` for manual review (M1, M4) |
| Original↔translation block alignment fails on irregular output | Compare the number of blocks; if different → `needs_review` instead of aligning by force (M1) |
| A reasoning model spends the whole budget thinking and answers nothing | The thinking is streamed into `reasoning_content`, which the client keeps out of the answer and stores for audit; a role can turn the thinking off with `chat_template_kwargs`; a structured pass retries once with a larger default budget; the failure names the stop reason, the sizes and the budget (M4) |
| Packaging of the Python sidecar on 3 OSes | `onedir` sidecar (not `onefile`: faster startup, fewer AV false positives), bundled as a Tauri resource; smoke test in CI on Linux, manual build on macOS/Windows (M0, M7) |
| Python 3.14 without PyInstaller/torch wheels | Pin **Python 3.12** in the sidecar (already available via uv) |
| Marker drags in torch (GB) | Optional extra, never in the default bundle; the `PdfExtractor` interface keeps it out of the core (M2) |
| Insufficient VRAM with different models per role | ResourceGovernor + serial degradation + UI that shows the reason (M6) |
| `n_ctx` of the server different from the expected one | The budget is computed from `/props`, not from config; warning if inconsistent (M3) |
| List/blockquote granularity (single block) penalizes very long lists | Accepted in the MVP; per-item split planned in M2 if the tests on real books require it |
