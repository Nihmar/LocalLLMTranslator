# LocalLLMTranslator — Architecture and Implementation Plan

> Status: **M0 and M1 complete, M2 next.** This document is the source of truth for the
> architecture; the code follows it milestone by milestone.

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
| Frontend | React + TS + Vite + Tailwind + CodeMirror 6 | CodeMirror 6 has `@codemirror/merge` for the required side-by-side diff |
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

Advantages: URLs, inline code, math and notes cannot be translated or corrupted; the model
sees only prose. The map is a **pure function** of `source_text`, so it does not need to be
persisted — it is regenerated identically.

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
- **Crash recovery**: on startup the reaper frees the expired leases; ingestion and export write
  to temporary files and do an atomic `rename`; the sidecar is restarted and the in-flight
  requests (pure) are re-sent.
- **Selective resumption**: you can re-run a single chunk, a chapter, or "all chunks
  `failed`/`needs_review`".
- **Project export/import**: `.llmtz` = zip with `project.sqlite` (copy via `VACUUM INTO`),
  `markdown/`, `output/`, `prompts/` snapshot.

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

Concurrency limit per endpoint = `min(role_binding.max_concurrency, props.total_slots)`.

### 7.2 Prefix stability (important for performance)

`llama-server` reuses the KV cache of the common prefix between requests. The prompt is
therefore structured **on purpose**:

- **System message byte-identical for the whole book**: role, rules, `style_guide`, glossary
  (ordered deterministically by `source`), book metadata → guaranteed cache hit.
- **Volatile user message**: `chapter_title`, `chapter_summary_so_far`, `previous_context`,
  `text` → it is the only part that invalidates the cache.

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

GLOSSARY (source => target)
{{ glossary }}

BOOK
Title: {{ book_title }}
Author: {{ book_author }}
Synopsis: {{ synopsis }}
```

### `prompts/translator.md` (user — volatile)

```jinja
CHAPTER: {{ chapter_title }}
{{ chapter_summary_so_far }}

PREVIOUS PASSAGE (already translated — for continuity of tone, pronouns and terminology only; do NOT translate it):
{{ previous_context }}

PASSAGE TO TRANSLATE:
{{ text }}
```

### `prompts/translator.table.md`

Same header, plus:

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
| 2 | **Relevant** glossary (only the terms present in the text of this chunk) | `glossary_term` | stable, ordered |
| 3 | Book synopsis | `project_memory.synopsis` | stable |
| 4 | Summary of the previous chapters (window 3) | `chapter.summary` | per chapter |
| 5 | Summary of the current chapter up to here | generated every N chunks | growing |
| 6 | Tail of the last translated passage | runtime | volatile |

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
  `project_memory`, the rest as a `book_meta` JSON document, proper nouns as `glossary_term`
  candidates and `do_not_translate` entries. The provenance travels with the value.
- Reproducible like everything else: seeded, audited in `llm_call`, and snapshotted into the
  project.
- Degrades cleanly: with no orchestrator model bound the step is skipped and the fields stay
  editable by hand.

---

## 10. Concurrency and sub-agents

- **Unit of parallelism**: the chunk. The queue is global, concurrency is per endpoint.
- **ResourceGovernor** (read-only, no driver dependency):
  - VRAM: sysfs (`/sys/class/drm/card*/device/mem_info_vram_{used,total}`) → `rocm-smi` →
    `nvidia-smi` → "unknown". *(on the development machine: AMD, therefore sysfs/rocm)*
  - Slots: `/props.total_slots`; fallback `llm_endpoint.max_concurrency`.
  - `max_parallel = min(free_slots, floor(VRAM_headroom / estimated_cost_per_slot), user_limit)`.
  - If `max_parallel < 2` → **degradation to serial without error**, with a UI event and an explicit reason.
- **Ordering**: priority = chapter order (sequential translation makes sense for
  coherence), the user's jobs at the front, the retries at the back.
- **Concurrent glossary**: the sub-agents **propose** terms, they do not impose them.
  `glossary_term.revision` + `status='candidate'`; if two agents propose different renderings for
  the same term → both saved as `candidate` and a `qa_finding(kind='glossary_conflict')`
  to be resolved in the UI. No blocking lock, no lost writes.
- **Determinism**: the seeding derives from `hash(chunk_id, role)` → stable across serial and
  parallel runs.

---

## 11. User interface

A 5-step wizard, but each step is a freely visitable route (not a constraint):

1. **Ingestion** — drag&drop, format detection, chapter and block preview, PDF backend
   choice, extraction result with warnings.
2. **Models** — endpoint CRUD (URL, health-check, model list from `/v1/models`, `props`),
   role assignment, savable profiles, VRAM/slot indicator.
3. **Translation** — book profile panel (the reconnaissance result of §9.4, confirmed field by
   field) plus chunk table (`pending/running/done/failed/needs_review`) with tokens,
   attempts, model; start/pause/resume; live log; resource gauge; actions on multiple
   selection (retry, skip, re-translate with another model).
4. **Review** — 3-column side-by-side editor (original / translated / corrected) with block-level
   and character-level diff, navigation by suggestion, accept/reject per individual
   change, and a filterable QA report.
5. **Export** — per-chapter unit, `metadata.yaml`, template/CSS/LaTeX choice, preview,
   selective rebuild of only the modified chapter, build history.

Plus: **Job dashboard** (per-chunk progress, ETA computed from the real throughput, log, resources)
and **Projects** (multiple, resume, export/import `.llmtz`).

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
| `pandoc_build` | `{output_path, log, duration_ms}` |
| `estimate_tokens` | `{counts[]}` (heuristic fallback, used if `/tokenize` is not available) |

The sidecar is **stateless** and does not touch the DB: every method is a pure function. This is
what makes it safe to restart it and re-send the in-flight requests.

### 12.2 Tauri (commands + events)

- Commands: `project_*`, `endpoint_*`, `role_binding_*`, `ingest_start`, `translation_start/pause/resume/cancel`,
  `job_list`, `chunk_get`, `review_start`, `suggestion_list/accept/reject`, `qa_report`,
  `export_build`, `export_preview`, `glossary_*`, `metrics_get`.
- Events: `job://progress`, `log://line`, `metrics://tick`, `qa://finding`, `sidecar://status`,
  `sidecar://progress`, `export://progress`.

`job://progress` carries the serialized `job` row at every transition the control plane owns;
views treat it as an invalidation trigger and refetch through commands. `sidecar://progress`
forwards the sidecar's out-of-band `progress` notifications unchanged, and `log://line`
(`{ts, level, source, message}`) carries the sidecar's stderr and the worker's failures.

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
  fake VRAM profile), `test_scheduler` (idempotency on re-run).
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
| Packaging of the Python sidecar on 3 OSes | `onedir` sidecar (not `onefile`: faster startup, fewer AV false positives), bundled as a Tauri resource; smoke test in CI on Linux, manual build on macOS/Windows (M0, M7) |
| Python 3.14 without PyInstaller/torch wheels | Pin **Python 3.12** in the sidecar (already available via uv) |
| Marker drags in torch (GB) | Optional extra, never in the default bundle; the `PdfExtractor` interface keeps it out of the core (M2) |
| Insufficient VRAM with different models per role | ResourceGovernor + serial degradation + UI that shows the reason (M6) |
| `n_ctx` of the server different from the expected one | The budget is computed from `/props`, not from config; warning if inconsistent (M3) |
| List/blockquote granularity (single block) penalizes very long lists | Accepted in the MVP; per-item split planned in M2 if the tests on real books require it |
