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
the user can add to the guide. The glossary is editable in the profile panel; only approved
terms reach a prompt (candidates wait for your decision), and only the terms a chunk actually contains are injected, so a book with
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

## Web app (headless)

The same UI runs in a browser, served by a headless engine:

```sh
make build-ui
cargo run -p local-llm-translator-server -- serve   # http://127.0.0.1:4321
```

`llmtz serve` starts the control plane (database, queue, worker pool, sidecar supervisor) and
serves the built UI, `POST /api/<command>` with exactly the arguments `invoke()` sends, and
`GET /api/events` (SSE) with the same events. It binds loopback by default; a non-loopback
`--host` requires `--token <secret>` and refuses to start without one. `--data-dir` defaults to
the desktop app's data directory, so the browser shows the same books; `LLMTZ_DATA_DIR`,
`LLMTZ_UI_DIR` and `LLMTRANSLATOR_SIDECAR` override the defaults.

The desktop app is untouched: `ui/src/lib/ipc.ts` and `ui/src/lib/events.ts` pick
invoke/listen or fetch/SSE at runtime. Picking a file in the browser uploads it to
`<data-dir>/uploads/` and the path-based commands receive the stored path; exports and bundles are
downloaded from `/api/download`, which only serves files under the data directory. See issue #17.

The same pipeline is scriptable:

```sh
make build-ui
cargo run -p local-llm-translator-server -- translate libro.epub --to it \
  --endpoint http://127.0.0.1:8080 --model qwen2.5-32b-instruct
# stdout: /home/utente/.local/share/org.localllmtranslator.app/projects/<id>/output/libro.epub

cargo run -p local-llm-translator-server -- export <project-id> --format pdf
```

`--project <id>` resumes instead of creating; `--no-export` stops after translating; progress
and warnings go to stderr and the exit code is non-zero on failure.

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

### Reasoning models (thinking)

The editor, the reconnaissance and the summarizer ask for **JSON** and cap the answer
(900–1500 tokens by default). A reasoning model streams its thinking first, so the budget can be
spent before the JSON starts: the call then comes back with no answer at all and the job fails
with `the … answer contains no JSON object (finish_reason=length, answer=0 chars, reasoning=…
chars, max_tokens=…)`. The fixed pass is named in the parentheses; the thinking of every call is
kept in `llm_call.reasoning_text` for the same diagnosis. Two ways out, both per role in
**Modelli → Parametri di generazione** of a binding:

```json
{ "temperature": 0.2, "top_p": 0.95, "chat_template_kwargs": { "enable_thinking": false } }
```

```json
{ "temperature": 0.2, "top_p": 0.95, "max_tokens": 8192 }
```

The JSON roles (editor, proofreader, orchestrator) already send `enable_thinking: false` when
the binding does not mention it, so the first form matters for the translator, or to turn the
thinking back **on** for a JSON role with `"enable_thinking": true` and a larger `max_tokens`.
The object is forwarded to `llama-server` as the request's `chat_template_kwargs`. Verified on the Gemma-4 aliases, `enable_thinking: false`
is the key that switches it off; a per-request `reasoning_effort` does **not** override the
server's `--chat-template-kwargs`, so such a role keeps thinking at the server's effort whatever
the binding says. The second form gives the thinking and the JSON room to coexist.

A structured pass already retries once and doubles the app's own budget when the first attempt
ran out, but a `max_tokens` you set yourself is respected, never overridden.

### Worked example: one 16 GB card, a fast small model and a slower big one

Two models resident on the same card: a 4B-class one that is quick, and a 12B-class one that is
better but slower. The assignment below follows from where each pass spends its compute, not from
which model is "best":

| Role | Model | Why |
|---|---|---|
| `translator` | small (4B) | it is the pass that **decodes** the most (its answer is as long as the chunk), so its speed is felt on every chunk of the book |
| `editor` | big (12B) | its prompt is the source **and** the translation of the chunk — almost all prefill, a short answer — and it is where judgement matters: a noisy editor is time spent reviewing |
| `proofreader` | small (4B) | monolingual polish; every change is a suggestion you accept or reject |
| `orchestrator` | big (12B) | it runs rarely (reconnaissance once, summaries every few chunks) and its output — style guide, synopsis, glossary terms, once you confirm them — is read by the translator on **every** chunk |

```sh
# Small model: translator + proofreader
llama-server -m models/small-4b.gguf --host 127.0.0.1 --port 8080 \
  -c 16384 --parallel 1 --cont-batching --cache-reuse 256 --jinja \
  --n-gpu-layers 999 --flash-attn --cache-type-k q8_0 --cache-type-v q8_0

# Big model: editor + orchestrator
llama-server -m models/mid-12b.gguf --host 127.0.0.1 --port 8081 \
  -c 32768 --parallel 1 --cont-batching --cache-reuse 256 --jinja \
  --n-gpu-layers 999 --flash-attn --cache-type-k q8_0 --cache-type-v q8_0
```

The context sizes are not free choices:

- The **translator's** `n_ctx` decides the chunk size, because `ingest` may use 60% of it: 16384
  gives ~9.8k tokens of budget, so chunks of roughly 7-9k tokens once the rules, glossary and
  summaries have taken their share.
- The **editor's** `n_ctx` must hold that chunk **twice** plus the schema, and nothing in the app
  checks it against the endpoint: with ~9k-token chunks, 16384 would be truncated silently, so
  give it 32768.
- The **proofreader** refuses more than 24 000 characters of translated text per call (~7k
  tokens), which the translator's 16384 covers.
- Weights plus KV cache have to fit: about 7-7.5 GB for a 12B at Q4_K_M, 4-5 GB for a 4B, and
  1.5-2.5 GB of quantised KV for these contexts. Keep ~1.5 GB for the compositor and the app
  window itself; if VRAM runs short, lower the editor to 24576 before lowering the model
  precision.

Three traps worth knowing:

1. **A `llama-server` router does not report `n_ctx`** to the app (`/props` has no top-level
   `n_ctx`), so the chunk budget silently falls back to the documented 6000 tokens whatever
   `--ctx-size` the models were given: bind the *translator* to a direct `llama-server` if you
   want bigger chunks.
2. **`--parallel N` divides the context**: with `--parallel 4 -c 32768` every request sees 8192
   tokens however the app is configured. On one card, with both models resident, prefer
   `--parallel 1` on each server and let the two endpoints carry the concurrency — a translator
   call on one and an editor call on the other run at the same time, each with its whole context.
3. **A thinking model in a JSON role** spends the answer budget before the JSON starts; see the
   previous section. Check `reasoning_chars` in the `llm call completed` log line (or
   `llm_call.reasoning_text`) after a chapter: high values on `editor` or `orchestrator` mean the
   `chat_template_kwargs` are missing.

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
crates/server/  headless `llmtz` — HTTP command API, SSE events, serves the UI
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
| W1 | Headless web app (`llmtz serve`): HTTP command API, SSE events, browser transport, upload/download | ✅ |

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
positives than `onefile`), then `cargo tauri build`. `make build BUNDLES=appimage` limits the
bundle to one target (deb/rpm need `dpkg-deb`/`rpmbuild` on the machine, and the AppImage
bundler's own `strip` is older than recent glibc libraries, so the target sets `NO_STRIP=1`;
stripping is optional there). The directory is tracked through a
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
  the progress bars use them, no remote origin is reachable.
- Prompts and Pandoc templates are **user data**: externalised, copied into the project
  snapshot, editable from the UI.
- Nothing that requires a GPU or downloaded models on the default path: the `marker` exception
  is an optional extra and stays out of the bundle.

## Diagnostics and logs

Everything worth keeping is written to a daily file under the app data directory:
`logs/llmtz.<date>.log` (on Linux `~/.local/share/org.localllmtranslator.app/logs/`). It holds
the job transitions (start, outcome, duration), one line per model call (role, model, tokens,
answer and reasoning sizes, latency, outcome), the sidecar's state transitions and **the errors
the UI showed you**. Starting the app from a terminal prints the same lines there; `RUST_LOG=debug`
raises the level.

Book text, prompts, responses and glossary values **never** reach the logs: they stay in the
local database's `llm_call` table, which never leaves the machine.

From the **Job** dashboard ("Diagnostica" panel) you can:

- **Apri cartella log** to open the directory holding the files;
- **Esporta diagnostica**: writes `<data>/diagnostics/llmtz-diagnostics-<timestamp>.zip` with the
  five newest logs (last 5 MB each) and a `report.json` carrying versions, sidecar and worker
  state, the queue, failed jobs and the errors of the model calls. It is meant to be attached to
  a report: it holds no database and no book content.

When something goes wrong: export the bundle (or take the day's log file) and attach it together
with a description of what you were doing.

## License

MIT
