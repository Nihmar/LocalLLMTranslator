# LocalLLMTranslator

Desktop app to translate long documents — novels, essays, manuals — with LLMs running
**locally** on `llama.cpp`. It covers the whole cycle: ingestion → conversion → translation →
review → typesetting.

Everything works **offline**. No telemetry, no analytics, no network calls other than the
`llama-server` endpoints you configure.

> **Status: M0–M4 complete.** The skeleton runs end to end: a real EPUB goes through the Python
> sidecar (extract → blocks → chunks → placeholders), is translated against a `llama-server`
> endpoint, persisted, resumed after a crash and exported to EPUB and PDF with its images,
> footnotes and tables intact. Memory is in place too: book reconnaissance produces a candidate
> profile the user confirms field by field, the confirmed style guide and synopsis head every
> prompt, the glossary is filtered to the terms a chunk actually contains, rolling chapter
> summaries feed the following chapters, and repeated chunks are served from the translation
> memory without calling the model. The review step adds the bilingual editor and the
> proofreader, a three-column diff with accept/reject per proposal, and a filterable QA report.
> Export produces a readable PDF and EPUB with a table of contents, footnotes and images:
> per-chapter units, `metadata.yaml`, the shipped templates and Lua filters, a content preview
> and a build history, with unchanged builds skipped and single chapters buildable standalone.
> Concurrency is per endpoint: the scheduler claims a job only when its role has a free slot,
> degrades to serial with an explicit reason when VRAM or slots are unknown, retries sink behind
> fresh work, and concurrent glossary proposals never overwrite a rendering. Packaging ships the
> sidecar as a PyInstaller `onedir` executable, bundles it with the prompts and the pandoc assets
> as a Tauri resource, and lets a project travel as a `.llmtz` archive. See
> [Milestones](#milestones).

---

## Architecture

```
┌────────────────────────────── Tauri 2 (Rust) ──────────────────────────────┐
│  control plane: SQLite DB, job queue, scheduler, LLM client, orchestration  │
└───────────┬──────────────────────────┬──────────────────────┬──────────────┘
            │                          │                      │
      pandoc (subprocess)      llama-server ×N          sidecar Python
                               (started by you)        (stdio, JSON-RPC 2.0)
                                                              │
                                          extractors · markdown_ir · chunker
                                          placeholders · pandoc bridge · qa
```

**The boundary is architectural.** Rust is the *control plane*: it owns state, concurrency and
orchestration. Python is the *data plane*: its functions are pure (`input → output`), they
never touch the database and do not know about the queue. Practical consequence: restarting
the sidecar is always safe, because every request is repeatable with no side effects.

### The document model

Plain text never travels through the pipeline: an indexed structure always does.

- **Blocks with stable IDs** (`b000417`), deterministic from the position in the document. The
  parser preserves the exact slices of the Markdown, with the invariant
  `serialize(parse(md)) == md` byte for byte — that is what prevents *non-translated* blocks
  from being rewritten.
- **Structural chunks**: a chunk is always a list of whole blocks, never a cut at a fixed
  character count. Tables and code blocks are never split.
- **Inline placeholders** (`⟦1⟧`): URLs, inline code, math and footnotes are removed from the
  text before sending it to the model and reinserted afterwards. A URL cannot be translated
  because the model never sees it. It is also the mechanism the QA check for "broken
  placeholders" is built on.
- **Stable prefix**: the system message is byte-identical for every chunk of the same book, so
  `llama-server` reuses its KV cache and from the second chunk on the prefill is almost free.

### Book profile and memory

An empty "style guide" box is a bad interface, so M3 opens with *book reconnaissance*. The
`orchestrator` role receives local evidence only — the extractor metadata, the incipit and the
opening paragraphs of the chapters, and optionally text you pasted yourself — and returns a
**candidate** profile (genre, audience, era, narrative voice, register, style notes, themes,
synopsis, proper nouns) with the basis of every field. Nothing enters the translation prompts
until you confirm the fields: then the style guide and synopsis land in the project memory the
context builder already reads, and the accepted names become glossary terms. With no
orchestrator model bound the fields stay editable by hand. The app never fetches a page.

While translation runs, the same role maintains the memory: a summary of the chapter so far
every five chunks, a final summary when the chapter is complete (used as context by the next
chapters), candidate terms that never demote an existing rendering, and style-note candidates
the user can add to the guide. The glossary is editable in the profile panel; rejected terms
never reach a prompt, and only the terms a chunk actually contains are injected, so a book with
hundreds of entries does not eat the context.

The context budget is not a constant: `ingest` and the translator read `n_ctx` from `/props`
and may use 60% of it, and the prompt pieces are counted with `/tokenize` when the server
exposes it, falling back to the documented heuristic when it does not.

The full architecture, the database schema and the IPC contract are in [`PLAN.md`](./PLAN.md).
The development conventions are in [`AGENTS.md`](./AGENTS.md).

---

## Prerequisites

| Component | Version | Notes |
|---|---|---|
| Python | 3.12 | managed by `uv`; not 3.14, `marker`/`torch` have no wheels |
| Rust | stable | with `rustfmt` and `clippy` |
| Node | 22+ | UI only |
| Pandoc | 3.x | for EPUB/PDF/DOCX export |
| `llama-server` | recent | **you start it** — see below |
| Tauri Linux dependencies | `webkit2gtk-4.1`, `gtk+-3.0`, `libsoup-3.0`, `javascriptcoregtk-4.1` | |

## Quick start

```sh
make setup     # uv sync + npm install
make check     # lint + typecheck + test + build UI + cargo check
make dev       # start the desktop app
```

## `llama-server`

The app does **not** start the servers: it detects them on endpoints you configure, and reads
health, models, slot count and `n_ctx` from them. VRAM tuning therefore stays under your
control.

```sh
# Translator — wide context, 4 parallel slots
llama-server -m models/qwen2.5-32b-instruct-q5_k_m.gguf \
  --host 127.0.0.1 --port 8080 -c 32768 --parallel 4 --cont-batching \
  --cache-reuse 256 --jinja --n-gpu-layers 999 --flash-attn --metrics

# Editor / proofreader — smaller model, second endpoint
llama-server -m models/qwen2.5-14b-instruct-q6_k.gguf \
  --host 127.0.0.1 --port 8081 -c 16384 --parallel 2 --cont-batching --jinja
```

`--parallel N` is what enables the sub-agents: the app's concurrency is capped by the slots the
server exposes. If `/props` is unreachable or VRAM cannot be determined, the scheduler
**degrades to serial execution** without errors, stating the reason in the UI.

---

## Development

```sh
make lint       # ruff format --check + ruff check + cargo fmt --check + cargo clippy
make typecheck  # pyright strict (sidecar)
make test       # pytest + cargo test
make build-ui   # frontend build (required before cargo build/check)
make check      # all of the above
```

Tests in detail:

```sh
cd sidecar && uv run pytest tests/test_markdown_ir.py -v   # parser round-trip
cd sidecar && uv run pytest tests/test_placeholders.py -v  # substitution and reinjection
cd sidecar && uv run pytest tests/test_chunker.py -v       # chunker invariants
cd sidecar && uv run pytest tests/test_extractors.py -v    # epub/pdf/markdown ingestion
cd sidecar && uv run pytest tests/test_rpc.py -v           # json-rpc transport over stdio
cargo test                                                 # queue, leases, budget, SSE, RPC
cargo test --test recon -- --nocapture                     # book reconnaissance end to end
cargo test --test summarize -- --nocapture                 # rolling memory end to end
cargo test --test review -- --nocapture                    # editor/proofreader passes and accept
cargo test --test qa -- --nocapture                        # QA findings, real sidecar
cargo test --test walking_skeleton -- --nocapture          # end-to-end, real sidecar
cd ui && npm run test                                      # IPC unit tests
```

### Offline verification

`tools/fake_llama_server.py` is a deterministic replacement for `llama-server`: it implements
`/health`, `/props`, `/v1/models`, `/tokenize` and `/v1/chat/completions` with correct SSE
framing, and a pseudo-translation that **strictly preserves the structure** (lines, list
markers, heading levels, table pipes, placeholders). It exists so that a test asserting "the
structure survived" verifies the pipeline, not the fake model.

By default it returns only the translated passage, like a compliant
instruction-following model. It includes fault injection to exercise the error branches:

```sh
uv run --project sidecar python tools/fake_llama_server.py --port 8080 \
  --drop-placeholder 2 --truncate 0.5 --fail-rate 0.1
```

`--echo-prompt-prefix` (or `FAKE_LLAMA_ECHO_PROMPT_PREFIX=1`) restores the opposite
behaviour — echoing the non-translatable preface ahead of the passage — so the
pipeline's `needs_review` branch stays reproducible.

Test fixtures (EPUB, Markdown, PDF, plus a ~1M character EPUB):

```sh
uv run --project sidecar python tools/make_fixtures.py
```

---

## Repository structure

```
crates/app/     Tauri 2 (Rust) — control plane: DB, queue, LLM, orchestration
ui/             React + TS + Vite — presentation only, no direct network calls
sidecar/        Python — data plane: formats, Markdown IR, chunking, Pandoc
prompts/        Jinja2 templates, editable by the user
pandoc/         Templates, Lua filters, CSS/LaTeX
tools/          Fake llama-server and fixture generator
```

## Milestones

| # | Content | Status |
|---|---|---|
| **M0** | Repo, CI, Tauri window, sidecar, SQLite migrations | ✅ |
| **M1** | Walking skeleton: EPUB → Markdown → blocks → chunks → translation → Pandoc | ✅ |
| M2 | Robust EPUB/PDF ingestion, footnotes, tables, images | ✅ |
| M3 | Glossary, synopsis, rolling summaries, two-level cache, book reconnaissance | ✅ |
| M4 | Bilingual review (editor + proofreader), diff, QA report | ✅ |
| M5 | Export and typesetting with templates and Lua filters | ✅ |
| M6 | Parallel sub-agents with VRAM budget and serial degradation | ✅ |
| M7 | Packaging (PyInstaller + Tauri bundle) | ✅ |

Known gaps, worth knowing rather than blocking: the sidecar's `estimate_tokens` route is
intentionally unused because the control plane counts exactly via `/tokenize` with a built-in
heuristic fallback; PDF extraction is only as good as `pymupdf4llm` on a given document; the
bundled sidecar is large (~190 MB compressed) because PyMuPDF, NumPy and ONNX Runtime travel
with it; and the release bundle has only been built and inspected on Linux, so the macOS and
Windows packaging still needs a real run on those machines.

## Packaging

```sh
make build    # sidecar onedir (PyInstaller) + Tauri bundle
```

`make build` runs `uv run --extra package python -m build_sidecar`, which produces
`sidecar/packaging/llmtranslator_sidecar/` (onedir: faster start-up and fewer antivirus false
positives than `onefile`), then `cargo tauri build`. The directory is tracked through a
`.gitkeep`, so `cargo check` works before the sidecar was ever built. The Tauri resources declared
in `crates/app/tauri.conf.json` ship that directory, `prompts/` and the `pandoc/` template/
filter/style directories, so a packaged app finds the sidecar, the prompt defaults and the
typesetting assets without any environment variable. Verified on Linux: a debug `deb` bundle
contains all three and the packaged sidecar answers a JSON-RPC `ping`. macOS and Windows still
need a real bundling run on those machines.

On a development machine the app falls back to `python -m llmtranslator_sidecar`
(`LLMTRANSLATOR_SIDECAR` overrides the path) and resolves the pandoc assets from the repository,
so `make dev` needs no packaging step.

### Project bundles (`.llmtz`)

Each project card offers **Esporta .llmtz**, and the header offers **Importa .llmtz**. The archive
is a ZIP with `manifest.json` (format version, app version, export time), a `VACUUM INTO` snapshot
of the database, the project's `work/` (Markdown and extracted media), its `output/` and the
`prompts/` snapshot. Import extracts it under the data directory, copies the project-owned rows
and rewrites the absolute paths to the local ones; the transient queue and the export cache do
not travel, and an existing project id is rejected instead of overwritten.

## Product constraints

- **No telemetry**, no crash reporter, no remote fonts or CDNs.
- **No secrets in the database**: API keys live in the system keyring.
- The webview runs under a strict Content Security Policy (`tauri.conf.json`): only the bundled
  assets, the Tauri IPC channel and `asset:`/`data:` images; inline styles are allowed because
  CodeMirror and the progress bars use them, no remote origin is reachable.
- Prompts and Pandoc templates are **user data**: externalised, copied into the project
  snapshot, editable from the UI.
- Nothing that requires a GPU or downloaded models on the default path: the `marker` exception
  is an optional extra and stays out of the bundle.

## Diagnostica e log

Ogni evento utile è registrato in un file giornaliero sotto la cartella dati dell'app:
`logs/llmtz.<data>.log` (su Linux `~/.local/share/org.localllmtranslator.app/logs/`).
Contiene transizioni dei job (avvio, esito, durata), una riga per ogni chiamata al modello
(ruolo, modello, token, latenza, esito), le transizioni di stato del sidecar e **gli errori
che l'interfaccia ti ha mostrato**. Lanciando l'app da terminale i log si vedono anche lì;
`RUST_LOG=debug` alza il livello.

Il testo dei libri, i prompt, le risposte e i valori del glossario **non** finiscono nei
log: quelli restano nella tabella `llm_call` del database locale, che non esce mai dalla
macchina.

Dal dashboard **Job** (pannello "Diagnostica") puoi:

- **Apri cartella log** per vedere i file;
- **Esporta diagnostica**: crea `<dati>/diagnostics/llmtz-diagnostics-<timestamp>.zip` con i
  cinque log più recenti (ultimi 5 MB ciascuno) e un `report.json` con versioni, stato del
  sidecar e del worker, coda, job falliti ed errori delle chiamate. È pensato per essere
  allegato a una segnalazione e non contiene database né contenuti del libro.

Quando qualcosa non funziona: esporta il bundle (o prendi il file di log del giorno) e
allegalo alla descrizione di cosa stavi facendo.

## License

MIT
