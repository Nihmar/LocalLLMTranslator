# LocalLLMTranslator

App desktop per tradurre documenti lunghi — romanzi, saggi, manuali — con LLM eseguiti
**in locale** su `llama.cpp`. Copre l'intero ciclo: ingestione → conversione → traduzione →
revisione → impaginazione.

Tutto funziona **offline**. Nessuna telemetria, nessun analytics, nessuna chiamata di rete
eccetto gli endpoint `llama-server` che configuri tu.

> **Stato: M0 completato, M1 in corso.** Il modello documentale, il chunking e il layer dei
> placeholder sono implementati e coperti da test. Il control plane Rust compila, passa clippy
> e i suoi test unitari. La UI compila e type-checka. L'app **non è ancora eseguibile
> end-to-end**: mancano gli extractor EPUB/PDF, il bridge Pandoc e il server RPC del sidecar.
> Vedi [Milestone](#milestone).

---

## Architettura

```
┌────────────────────────────── Tauri 2 (Rust) ──────────────────────────────┐
│  control plane: DB SQLite, coda job, scheduler, client LLM, orchestrazione  │
└───────────┬──────────────────────────┬──────────────────────┬──────────────┘
            │                          │                      │
      pandoc (subprocess)      llama-server ×N          sidecar Python
                               (avviati da te)         (stdio, JSON-RPC 2.0)
                                                              │
                                          extractors · markdown_ir · chunker
                                          placeholders · pandoc bridge · qa
```

**Il confine è architetturale.** Rust è il *control plane*: possiede stato, concorrenza e
orchestrazione. Python è il *data plane*: le sue funzioni sono pure (`input → output`), non
toccano il database e non conoscono la coda. Conseguenza pratica: riavviare il sidecar è
sempre sicuro, perché ogni richiesta è ripetibile senza effetti collaterali.

### Il modello documentale

Il testo piatto non attraversa mai la pipeline: attraversa sempre una struttura indicizzata.

- **Blocchi con ID stabili** (`b000417`), deterministici dall'ordine nel documento. Il
  parser conserva le slice esatte del Markdown, con l'invariante
  `serialize(parse(md)) == md` byte per byte — è ciò che impedisce che i blocchi *non*
  tradotti vengano riscritti.
- **Chunk strutturali**: un chunk è sempre una lista di blocchi interi, mai un taglio a
  numero fisso di caratteri. Tabelle e blocchi di codice non vengono mai spezzati.
- **Placeholder inline** (`⟦1⟧`): URL, codice inline, matematica e note vengono rimossi dal
  testo prima di inviarlo al modello e reinseriti dopo. Un URL non può essere tradotto perché
  il modello non lo vede mai. È anche il meccanismo su cui si basa il controllo QA dei
  "placeholder rotti".
- **Prefisso stabile**: il system message è byte-identico per ogni chunk dello stesso libro,
  così `llama-server` riusa la propria KV cache e dal secondo chunk il prefill è quasi gratis.

L'architettura completa, lo schema del database e il contratto IPC sono in [`PLAN.md`](./PLAN.md).
Le convenzioni di sviluppo sono in [`AGENTS.md`](./AGENTS.md).

---

## Prerequisiti

| Componente | Versione | Note |
|---|---|---|
| Python | 3.12 | gestito da `uv`; non 3.14, `marker`/`torch` non hanno wheel |
| Rust | stable | con `rustfmt` e `clippy` |
| Node | 22+ | solo per la UI |
| Pandoc | 3.x | per l'export EPUB/PDF/DOCX |
| `llama-server` | recente | **lo avvii tu** — vedi sotto |
| Dipendenze Tauri Linux | `webkit2gtk-4.1`, `gtk+-3.0`, `libsoup-3.0`, `javascriptcoregtk-4.1` | |

## Avvio rapido

```sh
make setup     # uv sync + npm install
make check     # lint + typecheck + test + build UI + cargo check
make dev       # avvia l'app desktop
```

## `llama-server`

L'app **non avvia** i server: li rileva su endpoint che configuri tu, ne legge salute, modelli,
numero di slot e `n_ctx`. Il tuning della VRAM resta quindi sotto il tuo controllo.

```sh
# Traduttore — contesto ampio, 4 slot paralleli
llama-server -m models/qwen2.5-32b-instruct-q5_k_m.gguf \
  --host 127.0.0.1 --port 8080 -c 32768 --parallel 4 --cont-batching \
  --cache-reuse 256 --jinja --n-gpu-layers 999 --flash-attn --metrics

# Editor / proofreader — modello più piccolo, secondo endpoint
llama-server -m models/qwen2.5-14b-instruct-q6_k.gguf \
  --host 127.0.0.1 --port 8081 -c 16384 --parallel 2 --cont-batching --jinja
```

`--parallel N` è ciò che abilita i sub-agenti: la concorrenza dell'app è limitata dagli slot
che il server espone. Se `/props` non è raggiungibile o la VRAM non è determinabile, il
scheduler **degrada a esecuzione seriale** senza errori, dichiarandone il motivo nella UI.

---

## Sviluppo

```sh
make lint       # ruff format --check + ruff check + cargo fmt --check + cargo clippy
make typecheck  # pyright strict (sul sidecar)
make test       # pytest + cargo test
make build-ui   # build del frontend (necessario prima di cargo build/check)
make check      # tutto quanto sopra
```

Test in dettaglio:

```sh
cd sidecar && uv run pytest tests/test_markdown_ir.py -v   # round-trip del parser
cd sidecar && uv run pytest tests/test_placeholders.py -v  # sostituzione e reinserimento
cd sidecar && uv run pytest tests/test_chunker.py -v       # invarianti del chunker
cargo test                                                 # coda, lease, budget, SSE, RPC
```

### Verifica offline

`tools/fake_llama_server.py` è un rimpiazzo deterministico di `llama-server`: implementa
`/health`, `/props`, `/v1/models`, `/tokenize` e `/v1/chat/completions` con framing SSE
corretto, e una pseudo-traduzione che **preserva rigorosamente la struttura** (righe,
marcatori di lista, livelli di heading, pipe delle tabelle, placeholder). Serve perché un test
che asserisce "la struttura è sopravvissuta" verifichi la pipeline, non il finto modello.

Include iniezione di guasti per esercitare i rami di errore:

```sh
uv run --project sidecar python tools/fake_llama_server.py --port 8080 \
  --drop-placeholder 2 --truncate 0.5 --fail-rate 0.1
```

Fixture di prova (EPUB, Markdown, PDF, più un EPUB da ~1M caratteri):

```sh
uv run --project sidecar python tools/make_fixtures.py
```

---

## Struttura del repository

```
crates/app/     Tauri 2 (Rust) — control plane: DB, coda, LLM, orchestrazione
ui/             React + TS + Vite — sola presentazione, nessuna chiamata di rete diretta
sidecar/        Python — data plane: formati, Markdown IR, chunking, Pandoc
prompts/        Template Jinja2 editabili dall'utente
pandoc/         Template, filtri Lua, CSS/LaTeX
tools/          Fake llama-server e generator di fixture
```

## Milestone

| # | Contenuto | Stato |
|---|---|---|
| **M0** | Repo, CI, finestra Tauri, sidecar, migrazioni SQLite | ✅ |
| **M1** | Walking skeleton: EPUB → Markdown → blocchi → chunk → traduzione → Pandoc | 🚧 |
| M2 | Ingestione robusta EPUB/PDF, note, tavole, immagini | ⬜ |
| M3 | Glossario, sinossi, riassunti rolling, cache a due livelli | ⬜ |
| M4 | Revisione bilingue (editor + proofreader), diff, report QA | ⬜ |
| M5 | Export e impaginazione con template e filtri Lua | ⬜ |
| M6 | Sub-agenti paralleli con budget VRAM e degradazione seriale | ⬜ |
| M7 | Packaging (PyInstaller + bundle Tauri) | ⬜ |

Cosa manca per chiudere M1: gli extractor EPUB/PDF, il bridge Pandoc, i controlli QA, il
server RPC del sidecar, e il collegamento end-to-end con i command Tauri.

## Vincoli di prodotto

- **Nessuna telemetria**, nessun crash reporter, nessun font o CDN remoto.
- **Nessun segreto nel database**: le API key stanno nel keyring di sistema.
- Prompt e template Pandoc sono **dati dell'utente**: esternalizzati, copiati nello snapshot
  di progetto, editabili dalla UI.
- Niente che richieda una GPU o modelli scaricati nel percorso di default: l'eccezione
  `marker` è un extra opzionale e resta fuori dal bundle.

## Licenza

MIT
