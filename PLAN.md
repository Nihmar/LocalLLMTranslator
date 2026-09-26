# LocalLLMTranslator — Piano di architettura e implementazione

> Stato: **piano approvato, implementazione non iniziata.**
> Il `README.md` presente alla radice è residuo di una implementazione precedente, eliminata
> volontariamente, e va riscritto da zero insieme al codice.

---

## 1. Decisioni prese

| Ambito | Scelta | Motivo |
|---|---|---|
| Shell desktop | **Tauri 2** (Rust) | Binario ~10 MB, RAM idle bassa, packaging cross-platform nativo (deb/AppImage/dmg/msi) |
| Documenti | **Sidecar Python** (PyInstaller onedir) | PyMuPDF/ebooklib/lxml sono l'ecosistema maturo per EPUB/PDF; riscriverlo in Rust è settimane di lavoro a qualità inferiore |
| IPC | **stdio, NDJSON, JSON-RPC 2.0** | Nessuna porta da allocare, nessun token da gestire, nessuna superficie di rete aggiuntiva; il sidecar è auto-riavviato dal supervisore Rust |
| Motore LLM | **`llama-server` esterni** | L'utente avvia e tunna i server; l'app rileva endpoint, salute, modelli e slot |
| Stato | **SQLite** (sqlx + migrazioni embedded) | Checkpoint, resume, cache, audit, coda job |
| PDF | **Interfaccia pluggable**: `pymupdf4llm` default, `marker` opzionale | Default senza dipendenze pesanti; marker attivabile dove serve qualità layout |
| Frontend | React + TS + Vite + Tailwind + CodeMirror 6 | CodeMirror 6 ha `@codemirror/merge` per il diff side-by-side richiesto |
| Test | pytest + ruff + pyright (Python), cargo test + clippy + rustfmt (Rust), **fake llama-server** | CI deterministica e offline |

**Vincoli di prodotto**: nessuna telemetria, nessuna chiamata di rete eccetto gli endpoint
configurati, prompt e template editabili dall'utente, tutto funzionante offline.

### Ambiente verificato sulla macchina di sviluppo

| Componente | Stato |
|---|---|
| Python | 3.12.13 disponibile via `uv` (da preferire; `marker`/`torch` non hanno wheel per 3.14) |
| Rust / Node | cargo 1.97.1, node v26.8.2 |
| Pandoc | 3.10.2 |
| Dipendenze Tauri Linux | webkit2gtk-4.1, gtk+-3.0, libsoup-3.0, javascriptcoregtk-4.1 → tutte presenti |
| GPU | AMD Radeon RX 9060 XT (Navi 44) — **niente NVIDIA**, VRAM leggibile da `/sys/class/drm/card1/device/mem_info_vram_{used,total}`; `rocm-smi` presente in `/opt/rocm/bin` |
| `just` | **non installato** → si usa un `Makefile` |

Conseguenza: la rilevazione VRAM deve provare **sysfs → rocm-smi → nvidia-smi**, in
quest'ordine, e degradare a "sconosciuta" senza errori.

---

## 2. Architettura

```
┌────────────────────────────── Tauri 2 (Rust) ──────────────────────────────┐
│  UI Bridge (commands + events)                                              │
│                                                                             │
│  ┌──────────────┐  ┌───────────────┐  ┌────────────────┐  ┌─────────────┐  │
│  │  Scheduler   │  │ ResourceGov.  │  │ ContextBuilder │  │  QA Engine  │  │
│  │ coda+lease   │  │ VRAM/slot cap │  │ budget+prompt  │  │  checks     │  │
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
                      (+ Lua filters)      (esterni)           (NDJSON RPC)
                                                                     │
                                    ┌────────────────────────────────┴──────┐
                                    │ extractors · markdown_ir · chunker    │
                                    │ placeholders · pandoc bridge · qa     │
                                    └───────────────────────────────────────┘
```

### Flusso dati

```
sorgente (EPUB/PDF/MD)
  └─▶ [sidecar] extractor ─▶ markdown canonico + front-matter YAML
        └─▶ [sidecar] markdown_ir ─▶ Block[] con id stabili
              └─▶ [sidecar] chunker ─▶ Chunk[] (block_ids, token_estimate)
                    └─▶ [sidecar] placeholder split ─▶ llm_text + PlaceholderMap
                          └─▶ [Rust] ContextBuilder ─▶ prompt (prefix stabile)
                                └─▶ [Rust] LlamaClient SSE ─▶ testo tradotto
                                      └─▶ [sidecar] reinject + re-parse + allineamento
                                            └─▶ block_translation[] (mappa originale↔tradotto)
                                                  ├─▶ editor (bilingue)  ─▶ suggerimenti
                                                  ├─▶ proofreader (target) ─▶ rifinitura
                                                  └─▶ QA checks ─▶ qa_finding[]
                                                        └─▶ [Rust] Pandoc Driver ─▶ PDF/EPUB/DOCX
```

### Perché il confine Rust/Python è dove è

- **Rust = control plane**: finestra, DB, coda, lease, budget risorse, chiamate HTTP/SSE,
  orchestrazione delle fasi, eventi verso la UI. È la parte con concorrenza e stato, dove
  Rust dà garanzie.
- **Python = data plane**: formati di file, Markdown IR, chunking, placeholder, Pandoc,
  euristiche QA. Sono funzioni **pure** (`input → output`): se il sidecar muore, ogni
  richiesta è ripetibile senza effetti collaterali.
- Conseguenza: il sidecar non tocca mai il DB e non conosce la coda. Un crash del sidecar
  non compromette lo stato.

---

## 3. Struttura del repository

```
LocalLLMTranslator/
├── README.md                      # riscritto da zero
├── PLAN.md                        # questo documento
├── AGENTS.md                      # convenzioni per agenti
├── Makefile                       # dev, test, lint, build, bundle (just non installato)
├── .github/workflows/ci.yml
├── crates/app/                    # Tauri 2
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   ├── icons/
│   ├── migrations/                # 0001_init.sql, ...
│   └── src/
│       ├── main.rs / lib.rs
│       ├── commands/              # superficie IPC verso la UI
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
│   │   ├── __main__.py            # server JSON-RPC su stdio
│   │   ├── rpc.py
│   │   ├── extractors/{base,epub,pdf_pymupdf,pdf_marker,markdown_src}.py
│   │   ├── markdown_ir/{parse,serialize,blocks}.py
│   │   ├── placeholders.py
│   │   ├── chunker.py
│   │   ├── pandoc.py
│   │   ├── qa.py
│   │   └── langs.py
│   └── tests/
├── prompts/                       # default spediti, copiati e resi editabili
│   ├── translator.md translator.table.md
│   ├── editor.md proofreader.md
│   └── summarizer.md orchestrator.md
├── pandoc/
│   ├── templates/{book.tex,book.html}
│   ├── filters/{footnotes.lua,tables.lua,epub_cleanup.lua}
│   └── styles/{book.css,book.tex}
└── tools/
    ├── fake_llama_server.py       # server OpenAI-compat deterministico per i test
    └── make_fixtures.py           # genera EPUB/PDF di prova
```

---

## 4. Modello documentale: blocchi, chunk, placeholder

Questa è la parte che decide se la struttura sopravvive a tre passaggi LLM.
**Regola: il testo piatto non attraversa mai la pipeline; attraversa sempre una struttura
indicizzata.**

### 4.1 `Block` — unità atomica con ID stabile

```python
@dataclass
class Block:
    id: str            # "b000417" — progressivo deterministico dall'ordine nel documento
    chapter_id: str
    order: int
    kind: str          # heading|para|list|blockquote|table|code|figure|footnote_def|hr|html
    level: int         # livello heading o annidamento lista
    source_md: str     # slice Markdown esatta
    source_text: str   # testo effettivamente inviato al modello (marker di blocco rimossi dove serve)
    translatable: bool
    attrs: dict        # {"text_prefix": "## ", "ordered": true, "align": [...], "lang": "sql", "ref": "^3"}
    content_hash: str  # sha256(source_text canonico) — per cache e rilevamento modifiche
```

- **ID stabili**: `b{order:06d}`. Deterministici, sopravvivono a resume, export/import e
  ri-estrazione dello stesso file. Nessun UUID casuale.
- **Non traducibili**: `code`, `hr`, `html`, `figure`. Restano nel flusso, il chunker li
  attraversa senza inviarli.
- **Tabelle**: un solo `Block` con `kind="table"`, `source_md` completo, prompt dedicato.

**Segmentazione guidata dal sorgente, non da un AST.** Il parser è un line-scanner che conserva
le slice esatte del Markdown originale, così `serialize(parse(md)) == md` è un'invariante
testabile. Un parser che produce un AST e lo ri-serializza normalizzerebbe il Markdown,
rompendo la fedeltà sui blocchi non tradotti.

Unità di segmentazione: fenced code block (con info string), heading ATX, tabella GFM
(riga di celle + riga delimitatrice), lista contigua (tutti gli item, marker preservati),
blockquote contiguo, definizione di nota, blocco HTML, horizontal rule, paragrafo.

Scelte di granularità per l'MVP, con motivazione:

- **Lista intera = un blocco** (non un blocco per item): tradurre la lista come unità mantiene
  numero di item e marker stabili e lascia al modello i riferimenti incrociati tra item.
  Il chunker potrà spezzare liste troppo grandi per item in una milestone successiva.
- **Blockquote intero = un blocco**, stesso ragionamento.
- **Heading**: i `#` vengono rimossi dal testo inviato e ricostruiti da `attrs.text_prefix`,
  così il modello non può cambiare il livello del titolo.

Due funzioni distinte:

- `serialize(blocks) -> str` — ricostruzione **identica** del sorgente (invariante + documenti
  senza traduzione). Richiede di conservare, per ogni blocco, il numero di righe vuote che lo
  separavano dal successivo.
- `render(blocks, translations) -> str` — costruzione dell'output: per ogni blocco traducibile
  con traduzione disponibile emette `attrs.text_prefix + traduzione`, altrimenti emette
  `source_md` invariato.

### 4.2 Placeholder inline — perché il Markdown non si corrompe

Prima di inviare testo al modello, gli elementi inline vengono sostituiti con token opachi `⟦n⟧`:

| Sorgente | Testo inviato | Mappa |
|---|---|---|
| `**La città**` | `⟦1⟧La città⟦2⟧` | 1=`**`, 2=`**` (delimitatori) |
| `[il re](https://x.org/a)` | `⟦3⟧il re⟦4⟧` | 3=`[`, 4=`](https://x.org/a)` — **l'URL non entra mai nel prompt** |
| `` `x = 1` `` | `⟦5⟧` | 5=span opaco, testo invariato |
| `[^3]` | `⟦6⟧` | 6=riferimento nota |
| `$E=mc^2$` | `⟦7⟧` | 7=math opaco |

Vantaggi: URL, codice inline, math e note non possono essere tradotti né corrotti; il modello
vede solo prosa. La mappa è una **funzione pura** di `source_text`, quindi non va persistita —
si rigenera identica.

Validazione post-traduzione: ogni placeholder deve comparire **esattamente una volta**. Se
manca o è duplicato → retry con messaggio che elenca i token mancanti → poi fallback a
"Markdown grezzo, preserva la formattazione" → poi `qa_finding` per revisione manuale. Se
l'insieme è completo ma l'ordine è alterato, riparazione automatica riordinando per indice.

### 4.3 `Chunk` — unità di lavoro e di checkpoint

```python
@dataclass
class Chunk:
    id: str                 # "c000123"
    chapter_id: str
    order: int
    block_ids: list[str]    # blocchi contenuti, in ordine
    source_md: str          # ricomposizione dei blocchi
    token_estimate: int
    context_carrier: dict   # catena di heading corrente, per orientare il modello
    flags: list[str]        # ["table", "table_part:2/3", "continues", "oversized"]
```

Il chunk **non** è mai un taglio a numero fisso di caratteri: è sempre una lista di blocchi interi.

---

## 5. Schema SQLite

```sql
CREATE TABLE project (
  id TEXT PRIMARY KEY, name TEXT NOT NULL,
  source_path TEXT NOT NULL, source_hash TEXT NOT NULL,
  source_format TEXT NOT NULL, source_lang TEXT, target_lang TEXT NOT NULL,
  doc_title TEXT, doc_author TEXT,
  prompts_snapshot_dir TEXT,            -- copia dei prompt usati → riproducibilità
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
  context_manifest_json TEXT,               -- hash dei pezzi di contesto iniettati
  target_md TEXT, error TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX idx_chunk_status ON chunk(status, order_index);

CREATE TABLE block_translation (            -- mappa originale ↔ traduzione
  block_id TEXT NOT NULL REFERENCES block(id) ON DELETE CASCADE,
  chunk_id TEXT NOT NULL REFERENCES chunk(id) ON DELETE CASCADE,
  text_md TEXT NOT NULL, placeholders_ok INTEGER NOT NULL DEFAULT 1,
  origin TEXT NOT NULL,                     -- translator|editor|proofreader|user
  edited_by_user INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL,
  PRIMARY KEY (block_id, origin)
);

CREATE TABLE suggestion (                   -- proposte di editor/proofreader
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
  revision INTEGER NOT NULL DEFAULT 1,      -- lock ottimistico
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
  api_key_ref TEXT,                         -- nome nel keyring, mai il segreto in chiaro
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

CREATE TABLE llm_call (                     -- audit + riproducibilità
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

CREATE TABLE translation_memory (           -- riuso di blocchi identici
  content_hash TEXT NOT NULL, model TEXT NOT NULL, target_lang TEXT NOT NULL,
  text_md TEXT NOT NULL, hits INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL,
  PRIMARY KEY (content_hash, model, target_lang)
);
```

Cache a due livelli: `translation_cache` è esatta sul prompt completo (copia-incolla su
retry/resume), `translation_memory` riusa blocchi identici già accettati (testatine, paragrafi
ripetuti, frontespizi) senza nemmeno chiamare il modello.

---

## 6. Job, checkpoint, idempotenza

- **Claim**: `UPDATE job SET state='leased', lease_owner=?, lease_expires_at=now+90s WHERE id =
  (SELECT id FROM job WHERE state='pending' AND run_after<=now ORDER BY priority, created_at
  LIMIT 1) RETURNING *`. Lease rinnovato con heartbeat; scaduto → reaper lo riporta a `pending`
  con `attempts+1`.
- **Idempotenza**: ogni scrittura di risultato è un upsert chiavato sull'identità del lavoro
  (`chunk_id` + `prompt_hash`). Rieseguire un job già completato non produce effetti.
- **Crash recovery**: all'avvio il reaper libera i lease scaduti; ingestione ed export scrivono
  su file temporanei e fanno `rename` atomico; il sidecar viene riavviato e le richieste in volo
  (pure) sono re-inviate.
- **Ripresa selettiva**: si può rilanciare un singolo chunk, un capitolo, o "tutti i chunk
  `failed`/`needs_review`".
- **Export/import progetto**: `.llmtz` = zip con `project.sqlite` (copia via `VACUUM INTO`),
  `markdown/`, `output/`, `prompts/` snapshot.

---

## 7. Layer LLM

### 7.1 Client

`LlamaClient` (Rust) su `reqwest`:

- `health()` → `GET /health`
- `props()` → `GET /props` → `total_slots`, `n_ctx`, `default_generation_settings`, `model_path`
- `models()` → `GET /v1/models`
- `tokenize(text)` → `POST /tokenize` (**conteggio token esatto**, non euristico), con fallback
  euristico `len/3.5` quando l'endpoint non lo espone
- `chat_stream(req) -> Stream<Delta>` → `POST /v1/chat/completions` con `stream: true`, parsing
  SSE, supporto `response_format: json_schema` e `grammar` (GBNF) per editor e proofreader

Limite di concorrenza per endpoint = `min(role_binding.max_concurrency, props.total_slots)`.

### 7.2 Stabilità del prefisso (importante per le prestazioni)

`llama-server` riusa il KV cache del prefisso comune tra richieste. Il prompt è quindi
strutturato **apposta**:

- **System message byte-identico per tutto il libro**: ruolo, regole, `style_guide`, glossario
  (ordinato deterministicamente per `source`), metadati libro → cache hit garantito.
- **User message volatile**: `chapter_title`, `chapter_summary_so_far`, `previous_context`,
  `text` → è l'unica parte che invalida il cache.

Effetto: dal secondo chunk in poi il prefill costa quasi nulla. Conseguenza pratica: **mai**
inserire timestamp, `chunk_id` o contatori nel system message.

### 7.3 Configurazione consigliata dei server (documentata nel README)

```sh
# Traduttore — contesto ampio, 4 slot paralleli
llama-server -m models/qwen2.5-32b-instruct-q5_k_m.gguf \
  --host 127.0.0.1 --port 8080 -c 32768 --parallel 4 --cont-batching \
  --cache-reuse 256 --jinja --n-gpu-layers 999 --flash-attn --metrics

# Editor / proofreader — modello più piccolo, stesso server o secondo endpoint
llama-server -m models/qwen2.5-14b-instruct-q6_k.gguf \
  --host 127.0.0.1 --port 8081 -c 16384 --parallel 2 --cont-batching --jinja
```

---

## 8. Prompt template

Formato **Jinja2** (resi in Rust con `minijinja`, così Python e Rust usano gli stessi file).
Default in `prompts/`, copiati nello snapshot di progetto e resi editabili dalla UI.

### `prompts/translator.md` (system — stabile)

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

Stessa intestazione, più:

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

Schema di risposta (`response_format: json_schema`):

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

### `prompts/proofreader.md` (solo lingua target)

```jinja
You are a monolingual proofreader for {{ target_language }}.
The text was translated from {{ source_language }} and reads slightly foreign.
Fix grammar, agreement, punctuation, calques, false friends and unnatural collocations.
Do NOT change meaning. Do NOT add or remove content. Do NOT touch placeholders ⟦n⟧.
Do NOT alter Markdown structure, code spans, URLs or table pipes.
Output only the corrected text, with no commentary and no code fences.
```

### `prompts/summarizer.md` (memoria rolling — ruolo orchestrator)

```jinja
You maintain the memory of a translation project ({{ source_language }} → {{ target_language }}).
From the chapter excerpt below produce JSON only:
{"summary": "3-5 sentences in {{ target_language }}",
 "new_terms": [{"source":"","target":"","kind":"term|proper_noun|do_not_translate","note":""}],
 "style_notes": ["short observations about register, recurring constructions, forms of address"]}
Output at most 8 new_terms, only terms that recur or matter.
```

Le `new_terms` entrano in `glossary_term` come `status='candidate'`: la conferma è dell'utente
(o automatica, se configurata). È il meccanismo che soddisfa "memoria delle scelte terminologiche
già fatte".

---

## 9. Chunking e gestione del contesto

### 9.1 Algoritmo di chunking

1. Raggruppa i blocchi per capitolo.
2. Accumula blocchi finché `token_estimate ≤ budget` (budget = `n_ctx − riserva_prompt −
   riserva_output`, default 60% di `n_ctx` — valore letto da `/props`, non da config).
3. **Mai spezzare** una tabella, un code block o un blockquote con la sua nota; se il blocco è
   a cavallo del limite, chiudi il chunk prima.
4. Tabella più grande del budget → split per righe con **header ripetuto** in ogni parte e flag
   `table_part:i/n`; il prompt lo dichiara esplicitamente.
5. Paragrafo più grande del budget → split a confini di frase, flag `continues:true`, e il chunk
   successivo riceve la coda del precedente come contesto.
6. Ogni chunk porta con sé `context_carrier`: la catena di heading (H1 → H2 → H3) in cui si trova.

**Esempio** (budget 6000 token):

| Blocco | Tipo | Token | Esito |
|---|---|---|---|
| b000101 | h2 "Chapter 3 — The Siege" | 12 | chunk c000041 |
| b000102 | para | 480 | chunk c000041 |
| b000103 | blockquote + `[^3]` | 320 | chunk c000041 |
| b000104 | table 4×3 | 380 | chunk c000041 |
| b000105 | para | 1450 | chunk c000041 (tot. 2642) |
| b000106 | code ```sql | 210 | attraversato, **non inviato** |
| b000107 | para | 3900 | chunk c000042 (`continues` se spezzato) |
| b000108 | para | 600 | chunk c000043 |

### 9.2 ContextAssembler — budget con priorità

A parità di token disponibili, il contesto si riempie in quest'ordine e si tronca dall'ultimo:

| Priorità | Componente | Fonte | Persistenza |
|---|---|---|---|
| 1 | System prompt + regole + style guide | `prompts/translator.md`, `project_memory.style_guide` | stabile per il libro |
| 2 | Glossario **pertinente** (solo i termini presenti nel testo di questo chunk) | `glossary_term` | stabile, ordinato |
| 3 | Sinossi libro | `project_memory.synopsis` | stabile |
| 4 | Riassunto dei capitoli precedenti (finestra 3) | `chapter.summary` | per capitolo |
| 5 | Riassunto del capitolo corrente fino a qui | generato ogni N chunk | crescente |
| 6 | Coda dell'ultimo passaggio tradotto | runtime | volatile |

Iniettare l'intero glossario sarebbe un errore: su un libro con 400 termini divora il contesto.
Il filtro "termini presenti in questo chunk" è ciò che rende il glossario scalabile.

### 9.3 Contesto e cache

Il `prompt_hash` copre il prompt completo (system + user): il contesto volatile **fa parte**
dell'hash, quindi due chunk identici in contesti diversi non condividono la cache esatta, ed è
corretto. Il riuso economico avviene a livello di `translation_memory` (blocco identico → stessa
traduzione, zero chiamate) e di KV cache del server (prefisso system identico).

---

## 10. Concorrenza e sub-agenti

- **Unità di parallelismo**: il chunk. La coda è globale, la concorrenza è per endpoint.
- **ResourceGovernor** (solo lettura, nessuna dipendenza da driver):
  - VRAM: sysfs (`/sys/class/drm/card*/device/mem_info_vram_{used,total}`) → `rocm-smi` →
    `nvidia-smi` → "sconosciuta". *(sulla macchina di sviluppo: AMD, quindi sysfs/rocm)*
  - Slot: `/props.total_slots`; fallback `llm_endpoint.max_concurrency`.
  - `max_parallel = min(slot_liberi, floor(headroom_VRAM / costo_stimato_per_slot), limite_utente)`.
  - Se `max_parallel < 2` → **degradazione a seriale senza errore**, con evento UI e motivo esplicito.
- **Ordinamento**: priorità = ordine capitolo (la traduzione sequenziale ha senso per la
  coerenza), i job dell'utente in testa, i retry in coda.
- **Glossario concorrente**: i sub-agenti **propongono** termini, non li impongono.
  `glossary_term.revision` + `status='candidate'`; se due agenti propongono rese diverse per lo
  stesso termine → entrambe salvate come `candidate` e una `qa_finding(kind='glossary_conflict')`
  da risolvere in UI. Nessun lock bloccante, nessuna scrittura persa.
- **Determinismo**: il seeding deriva da `hash(chunk_id, role)` → stabile tra esecuzioni seriali
  e parallele.

---

## 11. Interfaccia utente

Wizard a 5 step, ma ogni step è una rotta visitabile liberamente (non un vincolo):

1. **Ingestione** — drag&drop, rilevamento formato, anteprima capitoli e blocchi, scelta backend
   PDF, esito estrazione con avvisi.
2. **Modelli** — CRUD endpoint (URL, health-check, lista modelli da `/v1/models`, `props`),
   assegnazione ruoli, profili salvabili, indicatore VRAM/slot.
3. **Traduzione** — tabella chunk (`pending/running/done/failed/needs_review`) con token,
   tentativi, modello; avvio/pausa/riprendi; log live; gauge risorse; azioni su selezione
   multipla (retry, salta, ri-traduci con altro modello).
4. **Revisione** — editor side-by-side a 3 colonne (originale / tradotto / corretto) con diff a
   livello di blocco e di carattere, navigazione per suggerimento, accetta/rifiuta per singola
   modifica, e report QA filtrabile.
5. **Export** — unità per capitolo, `metadata.yaml`, scelta template/CSS/LaTeX, anteprima,
   rebuild selettivo del solo capitolo modificato, cronologia build.

Più: **Dashboard job** (progresso per chunk, ETA calcolata dal throughput reale, log, risorse) e
**Progetti** (multipli, riprendi, esporta/importa `.llmtz`).

---

## 12. Contratto IPC

### 12.1 Sidecar (NDJSON su stdio, JSON-RPC 2.0)

Richieste `{"jsonrpc":"2.0","id":N,"method":"...","params":{...}}`; risposte `result` o
`error`; notifiche server→client per il progresso. Metodi previsti:

| Metodo | Ritorno |
|---|---|
| `ping` | `{pong, version, python, platform}` |
| `detect_format` | `{format, backends[]}` |
| `ingest` | `{markdown_path, metadata, chapters[], warnings[]}` |
| `parse_document` | `{blocks[], chapters[]}` |
| `build_chunks` | `{chunks[]}` |
| `prepare_text` | `{llm_text, placeholders[]}` |
| `reinject` | `{blocks_md[], placeholders_ok, missing[], duplicated[]}` |
| `qa_check` | `{findings[]}` |
| `pandoc_build` | `{output_path, log}` |
| `estimate_tokens` | `{counts[]}` (fallback euristico, usato se `/tokenize` non c'è) |

Il sidecar è **senza stato** e non tocca il DB: ogni metodo è una funzione pura. Questo è ciò che
rende sicuro riavviarlo e ri-inviare le richieste in volo.

### 12.2 Tauri (commands + events)

- Commands: `project_*`, `endpoint_*`, `role_binding_*`, `ingest_start`, `translation_start/pause/resume/cancel`,
  `job_list`, `chunk_get`, `review_start`, `suggestion_list/accept/reject`, `qa_report`,
  `export_build`, `export_preview`, `glossary_*`, `metrics_get`.
- Events: `job://progress`, `log://line`, `metrics://tick`, `qa://finding`, `sidecar://status`,
  `export://progress`.

---

## 13. Milestone

| # | Contenuto | Criterio di accettazione |
|---|---|---|
| **M0** | Repo, `Makefile`, CI (ruff/pyright/pytest + cargo fmt/clippy/test), finestra Tauri, sidecar con `ping`, migrazioni SQLite, `AGENTS.md`, README riscritto | `make check` verde, app vuota che si apre su Linux |
| **M1** | **Walking skeleton end-to-end**: EPUB → Markdown → `Block[]` → `Chunk[]` → placeholder → traduzione con il solo ruolo *translator* → persistenza + resume → Pandoc → EPUB/PDF. UI minima: crea progetto, scegli file, endpoint, avvia, progresso, apri output | EPUB tradotto; interruzione a metà e ripresa senza perdita di struttura; PDF ed EPUB generati |
| **M2** | Ingestione robusta: PDF `pymupdf4llm` dietro interfaccia `PdfExtractor`, note a piè di pagina, tavole, immagini, front-matter YAML, ancore e ID stabili, `pdf_marker.py` opzionale | Note e immagini presenti e integre nell'output; ID stabili tra due estrazioni |
| **M3** | Contesto e memoria: glossario, sinossi, riassunti rolling, style guide, ContextAssembler con budget, `/tokenize`, `translation_cache` + `translation_memory` | Chunk ripetuti non richiamano il modello; il glossario compare nel prompt solo per i termini presenti |
| **M4** | Revisione bilingue e QA: editor JSON, proofreader, UI diff, accetta/rifiuta per modifica, report QA (non tradotti, incoerenze glossario, placeholder rotti, lunghezze anomale, Markdown malformato) | Il report segnala correttamente non-tradotti e incoerenze su un test controllato |
| **M5** | Export e impaginazione: split per capitolo, `metadata.yaml`, template Pandoc/CSS/LaTeX, filtri Lua per note e tavole, anteprima, rebuild selettivo | PDF ed EPUB leggibili con indice, note e immagini |
| **M6** | Concorrenza e sub-agenti: coda con lease, ResourceGovernor, degradazione seriale, glossario con lock ottimistico e merge | I sub-agenti si attivano solo con VRAM/slot sufficienti e degradano senza errori |
| **M7** | Packaging: PyInstaller onedir, bundle Tauri (deb/AppImage/dmg/msi), primo avvio guidato, export/import `.llmtz`, docs | AppImage e bundle avviabili su macchina pulita |

Dipendenze: M1 sblocca tutto; M3 va prima di M4 (l'editor usa glossario e contesto); M6 dopo M3
(la concorrenza richiede il glossario versionato).

---

## 14. Verifica

- **`tools/fake_llama_server.py`**: server OpenAI-compatibile deterministico
  (`/v1/models`, `/v1/chat/completions` con SSE, `/props`, `/tokenize`, `/health`). Traduzioni
  finte ma strutturalmente fedeli; può iniettare guasti (drop di placeholder, troncamenti,
  timeout) per esercitare i rami di errore. È la base della CI offline.
- **Fixture**: `tools/make_fixtures.py` genera EPUB e PDF di prova con tabelle, note, immagini,
  blocchi di codice e capitoli annidati; un EPUB "grande" (~1M caratteri) per i test di
  robustezza e memoria.
- **Invariante chiave**: `serialize(parse(md)) == md` su tutti i fixture — è il test che
  garantisce che l'IR non normalizzi il Markdown.
- **Test Python**: `test_markdown_ir` (round-trip), `test_placeholders` (round-trip e
  riparazione), `test_chunker` (nessun blocco perso, nessuna tabella spezzata, somma token),
  `test_pandoc`, `test_qa`.
- **Test Rust**: `test_queue` (claim, lease scaduto, retry, concorrenza), `test_context_builder`
  (budget e priorità), `test_cache` (hit/miss), `test_resources` (degradazione seriale con
  profilo VRAM finto), `test_scheduler` (idempotenza su riesecuzione).
- **Test di integrazione end-to-end**: EPUB → traduzione (fake server) → export; interruzione a
  metà via `SIGTERM` e ripresa; verifica che il Markdown tradotto abbia la stessa sequenza di
  tipi di blocco dell'originale.
- **Manuale**: tradurre un EPUB reale di ~300 pagine con `llama-server` vero, interrompere,
  riprendere, esportare PDF ed EPUB.

---

## 15. Rischi e mitigazioni

| Rischio | Mitigazione |
|---|---|
| Il modello perde placeholder o riscrive la struttura | Validazione strutturale per chunk + retry mirato + fallback a Markdown grezzo + `qa_finding` per revisione manuale (M1, M4) |
| Allineamento blocchi originale↔traduzione fallisce su output irregolare | Confronto del numero di blocchi; se diverso → `needs_review` invece di allineare a forza (M1) |
| Packaging del sidecar Python su 3 OS | Sidecar `onedir` (non `onefile`: avvio più rapido, meno falsi positivi AV), bundlato come resource Tauri; smoke test in CI su Linux, build manuale su macOS/Windows (M0, M7) |
| Python 3.14 senza wheel PyInstaller/torch | Pinnare **Python 3.12** nel sidecar (già disponibile via uv) |
| Marker trascina torch (GB) | Extra opzionale, mai nel bundle di default; l'interfaccia `PdfExtractor` lo tiene fuori dal core (M2) |
| VRAM insufficiente con modelli diversi per ruolo | ResourceGovernor + degradazione seriale + UI che mostra il motivo (M6) |
| `n_ctx` del server diverso da quello atteso | Il budget si calcola da `/props`, non da config; avviso se incoerente (M3) |
| Granularità lista/blockquote (blocco unico) penalizza liste molto lunghe | Accettato nell'MVP; split per item previsto in M2 se i test su libri reali lo richiedono |
