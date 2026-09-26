# AGENTS.md — convenzioni del progetto

## Cos'è questo progetto

`LocalLLMTranslator`: applicazione desktop per tradurre documenti lunghi (romanzi, saggi,
manuali) con LLM eseguiti **in locale** su `llama.cpp`. Pipeline completa:
ingestione → conversione → traduzione → revisione → impaginazione.

**Leggi `PLAN.md` prima di scrivere codice.** Contiene architettura, schema del database,
contratto IPC, prompt template, strategia di chunking e milestone. È la fonte di verità:
se il codice e `PLAN.md` divergono, fermati e segnala la divergenza invece di scegliere da solo.

---

## Regola numero uno: commit frequenti e significativi

**Fai quanti più commit significativi puoi.** Non accumulare lavoro in un commit gigante a fine
sessione. Un commit = una unità logica di lavoro, completa e coerente.

Linee guida operative:

- Committa **appena** un'unità è completa e verificata (`ruff`/`pyright`/`pytest` o
  `cargo fmt`/`clippy`/`test` passano). Non aspettare la fine del task.
- Un modulo nuovo = un commit. Un test nuovo per quel modulo = un altro commit. Una migrazione
  dello schema = un altro commit ancora.
- **Non mischiare** riformattazioni, rinomini e cambi di comportamento nello stesso commit.
  Se `ruff format` tocca file non correlati, committali separatamente.
- Non committare mai codice che non compila o test rossi. Se sei in un vicolo cieco, usa
  `git stash` o un branch, non un commit "WIP" sul ramo principale.
- Commit piccoli e frequenti battono commit grandi e rari. Nel dubbio, spezza.

Formato: **Conventional Commits**, messaggio in inglese, imperativo, con scope.

```
<type>(<scope>): <descrizione breve>

<corpo opzionale: perché, non cosa>

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>
```

Tipi: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `perf`, `build`, `ci`.
Scope: `sidecar`, `rust`, `ui`, `tools`, `prompts`, `pandoc`, `db`, `docs`, `ci`.

Esempi di commit **giusti**:

```
feat(sidecar): parse markdown into stable-id blocks
feat(sidecar): add inline placeholder substitution
test(sidecar): assert markdown serialize/parse round-trip
feat(sidecar): chunk blocks by structural boundaries
feat(db): add initial schema migration
feat(rust): add llama-server HTTP client with SSE streaming
feat(ui): add chunk progress table
```

Esempi di commit **sbagliati**: `update files`, `wip`, `fix stuff`, o un unico commit che
contiene sidecar + UI + migrazioni.

---

## Struttura del repository

```
crates/app/     Tauri 2 (Rust) — control plane: DB, coda, LLM, orchestrazione
ui/             Frontend React + TS + Vite — solo presentazione
sidecar/        Python — data plane: formati, Markdown IR, chunking, Pandoc
prompts/        Template Jinja2 editabili dall'utente
pandoc/         Template, filtri Lua, CSS/LaTeX
tools/          Script di supporto (fake llama-server, generator di fixture)
```

**Il confine è architetturale, non stilistico.** Rust è il *control plane*: possiede stato,
concorrenza e orchestrazione. Python è il *data plane*: le sue funzioni sono **pure**
(`input → output`), non toccano mai il database e non conoscono la coda. Se ti accorgi di aver
bisogno di stato condiviso o di I/O sul DB nel sidecar, il design è sbagliato: segnalalo.

---

## Contratto IPC (congelato — non modificarlo senza aggiornare `PLAN.md`)

### Sidecar → JSON-RPC 2.0, NDJSON su stdio

Una richiesta per riga, una risposta per riga. `id` numerico progressivo.

```
→ {"jsonrpc":"2.0","id":1,"method":"ping","params":{}}
← {"jsonrpc":"2.0","id":1,"result":{"pong":true,"version":"0.1.0","python":"3.12.13"}}
← {"jsonrpc":"2.0","method":"progress","params":{"job_id":"...","done":12,"total":340}}
```

Errori: `{"jsonrpc":"2.0","id":N,"error":{"code":-32602,"message":"...","data":{...}}}`.
Codici: `-32700` parse, `-32600` invalid request, `-32601` method not found,
`-32602` invalid params, `-32603` internal, `1001` ingestion failure, `1002` pandoc failure,
`1003` missing dependency.

| Metodo | Params | Result |
|---|---|---|
| `ping` | `{}` | `{pong, version, python, platform}` |
| `detect_format` | `{path}` | `{format: "epub"\|"pdf"\|"markdown", backends: [str]}` |
| `ingest` | `{path, work_dir, pdf_backend?}` | `{markdown_path, metadata{}, chapters[{title,level,order}], warnings[str]}` |
| `parse_document` | `{markdown_path}` | `{blocks[Block], chapters[Chapter]}` |
| `build_chunks` | `{blocks[], budget_tokens}` | `{chunks[Chunk]}` |
| `prepare_text` | `{block_ids?, text}` | `{llm_text, placeholders[[n,literal]], used_blocks[int]}` |
| `reinject` | `{text, placeholders, expected_blocks}` | `{blocks_md[], placeholders_ok, missing[int], duplicated[int], block_count_ok}` |
| `qa_check` | `{source_text, target_text, glossary{}, placeholders[[n,literal]]}` | `{findings[Finding]}` |
| `pandoc_build` | `{units[{path,title}], metadata{}, output_path, output_format, template?, css?}` | `{output_path, log, duration_ms}` |
| `estimate_tokens` | `{texts[]}` | `{counts[int]}` |

`Block` = `{id, chapter_id, order, kind, level, source_md, source_text, translatable, attrs{}, content_hash}`
`Chunk` = `{id, chapter_id, order, block_ids[], source_md, token_estimate, context_carrier{}, flags[]}`
`Chapter` = `{id, order, title, level, block_first, block_last}`
`Finding` = `{kind, severity, block_id?, details{}}`

Tipi di `kind` per i blocchi:
`heading|para|list|blockquote|table|code|figure|footnote_def|hr|html`.

Il sidecar è **senza stato**: nessuna cache tra chiamate, nessun DB, nessun file temporaneo
oltre a quelli dichiarati in `work_dir`. Ogni metodo deve essere ripetibile senza effetti
collaterali — è ciò che rende sicuro riavviare il sidecar e ri-inviare le richieste in volo.

### UI → Tauri

Commands: `project_list`, `project_create`, `project_get`, `project_delete`,
`endpoint_list`, `endpoint_upsert`, `endpoint_delete`, `endpoint_test`, `endpoint_models`,
`role_binding_list`, `role_binding_set`, `ingest_start`, `translation_start`, `translation_pause`,
`translation_cancel`, `job_list`, `chunk_list`, `chunk_get`, `metrics_get`, `sidecar_status`,
`export_build`, `open_path`.

Events: `job://progress`, `log://line`, `metrics://tick`, `sidecar://status`, `export://progress`.

---

## Convenzioni per linguaggio

### Python (`sidecar/`)

- **Python 3.12** (pin in `.python-version`). Non 3.14: `marker`/`torch` non hanno wheel.
- Gestione progetto: `uv`. `uv sync --all-groups`, `uv run <cmd>`.
- Lint/format: `ruff` con `select = ["ALL"]`, `ignore = ["D", "COM812"]`.
- Tipi: `pyright` in modalità **strict**. Nessun `Any` non giustificato, nessun `# type: ignore`
  senza commento che spieghi perché.
- Test: `pytest` + `pytest-cov`.
- `from __future__ import annotations` in ogni modulo.
- Docstring solo dove il *perché* non è ovvio; niente commenti che ripetono il codice.
- Struttura: pacchetto `llmtranslator_sidecar`, test in `sidecar/tests/`.

### Rust (`crates/app/`)

- `cargo fmt` + `cargo clippy -- -D warnings` puliti.
- `sqlx` con query **runtime-checked** (`sqlx::query`), non le macro compile-time: evitano di
  richiedere un database attivo in fase di build. Migrazioni embedded con `sqlx::migrate!`.
- Errori: `thiserror` per gli errori di dominio, `anyhow` solo al bordo dei command handler.
- Async su `tokio`. Nessun `unwrap()`/`expect()` su percorsi che possono fallire a runtime
  (I/O, rete, parsing): solo dove l'invariante è garantita da costruzione, con commento.
- Log con `tracing`, strutturato, mai `println!`.

### TypeScript (`ui/`)

- TypeScript strict, nessun `any`.
- Nessuna chiamata di rete diretta: tutto passa da `invoke`/`listen` di `@tauri-apps/api`.
- UI in **italiano**, codice e commenti in inglese.

---

## Vincoli di prodotto (non negoziabili)

- **Nessuna telemetria.** Nessuna chiamata di rete eccetto gli endpoint `llama-server`
  configurati dall'utente. Nessun analytics, nessun crash reporter, nessun font o CDN remoto.
- **Tutto offline.** L'app deve funzionare senza connessione.
- **Nessun segreto nel database.** Le API key stanno nel keyring di sistema; nel DB solo un
  riferimento.
- Prompt e template Pandoc sono **dati dell'utente**: esternalizzati, copiati nello snapshot di
  progetto, editabili. Mai hardcodarli nel codice.
- Non introdurre dipendenze che richiedono una GPU o modelli scaricati nel percorso di default
  (l'eccezione `marker` è un extra opzionale e resta fuori dal bundle).

---

## Definition of done

Un'unità di lavoro è finita quando **tutti** questi passano:

```sh
make lint      # ruff check + pyright + cargo clippy
make test      # pytest + cargo test
make check     # lint + test + build UI + cargo check
```

Nessuna eccezione. Non lasciare test rossi, non lasciare errori di tipo, non committare con
`--no-verify`. Se un test esistente si rompe per una tua modifica, aggiustalo nello stesso
commit o in uno immediatamente successivo — mai lasciarlo rotto.
