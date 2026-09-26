# AGENTS.md — project conventions

## What this project is

`LocalLLMTranslator`: a desktop application to translate long documents (novels, essays,
manuals) with LLMs running **locally** on `llama.cpp`. Complete pipeline:
ingestion → conversion → translation → review → typesetting.

**Read `PLAN.md` before writing code.** It holds the architecture, the database schema, the
IPC contract, the prompt templates, the chunking strategy and the milestones. It is the source
of truth: if the code and `PLAN.md` diverge, stop and report the divergence instead of deciding
on your own.

---

## Language policy

**Everything that lands in the repository is written in English**: commit messages, code
comments, docstrings and documentation (`README.md`, `PLAN.md`, `AGENTS.md`, anything under
`docs/`). The single exception is the **UI copy**, which is in Italian — see the TypeScript
section. Never leave Italian prose in commits, comments or docs.

## Rule number one: frequent, meaningful commits

**Make as many meaningful commits as you can.** Do not pile work up into one giant commit at
the end of a session. One commit = one logical unit of work, complete and coherent.

Operating guidelines:

- Commit **as soon as** a unit is complete and verified (`ruff`/`pyright`/`pytest` or
  `cargo fmt`/`clippy`/`test` pass). Do not wait until the end of the task.
- A new module = one commit. A new test for that module = another commit. A schema migration =
  yet another commit.
- **Do not mix** reformatting, renames and behaviour changes in the same commit. If
  `ruff format` touches unrelated files, commit them separately.
- Never commit code that does not compile or tests that fail. If you are in a dead end, use
  `git stash` or a branch, not a "WIP" commit on the main branch.
- Small, frequent commits beat large, rare ones. When in doubt, split.

Format: **Conventional Commits**, message in English, imperative mood, with a scope.

```
<type>(<scope>): <short description>

<optional body: why, not what>

Co-authored-by: CommandCodeBot <noreply@commandcode.ai>
```

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `perf`, `build`, `ci`.
Scopes: `sidecar`, `rust`, `ui`, `tools`, `prompts`, `pandoc`, `db`, `docs`, `ci`.

Examples of **good** commits:

```
feat(sidecar): parse markdown into stable-id blocks
feat(sidecar): add inline placeholder substitution
test(sidecar): assert markdown serialize/parse round-trip
feat(sidecar): chunk blocks by structural boundaries
feat(db): add initial schema migration
feat(rust): add llama-server HTTP client with SSE streaming
feat(ui): add chunk progress table
```

Examples of **bad** commits: `update files`, `wip`, `fix stuff`, or a single commit holding
sidecar + UI + migrations.

---

## Repository structure

```
crates/app/     Tauri 2 (Rust) — control plane: DB, queue, LLM, orchestration
ui/             React + TS + Vite frontend — presentation only
sidecar/        Python — data plane: formats, Markdown IR, chunking, Pandoc
prompts/        Jinja2 templates, editable by the user
pandoc/         Templates, Lua filters, CSS/LaTeX
tools/          Support scripts (fake llama-server, fixture generator)
```

**The boundary is architectural, not stylistic.** Rust is the *control plane*: it owns state,
concurrency and orchestration. Python is the *data plane*: its functions are **pure**
(`input → output`), they never touch the database and do not know about the queue. If you find
yourself needing shared state or database I/O in the sidecar, the design is wrong: report it.

---

## IPC contract (frozen — do not change it without updating `PLAN.md`)

### Sidecar → JSON-RPC 2.0, NDJSON over stdio

One request per line, one response per line. Progressive numeric `id`.

```
→ {"jsonrpc":"2.0","id":1,"method":"ping","params":{}}
← {"jsonrpc":"2.0","id":1,"result":{"pong":true,"version":"0.1.0","python":"3.12.13"}}
← {"jsonrpc":"2.0","method":"progress","params":{"job_id":"...","done":12,"total":340}}
```

Errors: `{"jsonrpc":"2.0","id":N,"error":{"code":-32602,"message":"...","data":{...}}}`.
Codes: `-32700` parse, `-32600` invalid request, `-32601` method not found,
`-32602` invalid params, `-32603` internal, `1001` ingestion failure, `1002` pandoc failure,
`1003` missing dependency.

| Method | Params | Result |
|---|---|---|
| `ping` | `{}` | `{pong, version, python, platform}` |
| `detect_format` | `{path}` | `{format: "epub"\|"pdf"\|"markdown", backends: [str]}` || `ingest` | `{path, work_dir, pdf_backend?}` | `{markdown_path, metadata{}, chapters[{title,level,order}], warnings[str], assets_dir?, assets[]}` |
| `parse_document` | `{markdown_path}` | `{blocks[Block], chapters[Chapter]}` |
| `build_chunks` | `{blocks[], budget_tokens}` | `{chunks[Chunk]}` |
| `prepare_text` | `{block_ids?, text}` | `{llm_text, placeholders[[n,literal]], used_blocks[int]}` |
| `reinject` | `{text, placeholders, expected_blocks}` | `{blocks_md[], placeholders_ok, missing[int], duplicated[int], block_count_ok}` |
| `qa_check` | `{source_text, target_text, glossary{}, placeholders[[n,literal]]}` | `{findings[Finding]}` |
| `pandoc_build` | `{units[{path,title}], metadata{}, output_path, output_format, template?, css?, resource_path[], toc?, lua_filters?, top_level_division?}` | `{output_path, log, duration_ms}` |
| `estimate_tokens` | `{texts[]}` | `{counts[int]}` |

`Block` = `{id, chapter_id, order, kind, level, source_md, source_text, translatable, attrs{}, content_hash}`
`Chunk` = `{id, chapter_id, order, block_ids[], source_md, token_estimate, context_carrier{}, flags[]}`
`Chapter` = `{id, order, title, level, block_first, block_last}`
`Finding` = `{kind, severity, block_id?, details{}}`

Block `kind` values:
`heading|para|list|blockquote|table|code|figure|footnote_def|frontmatter|hr|html`.

`ingest` extracts embedded media (images) into `<work_dir>/assets/`, rewrites the markdown to
reference them as `assets/<name>` relative to `document.md`, and reports `assets_dir` (absolute,
`null` when there is none) plus the rewritten hrefs in `assets`. `pandoc_build` receives
`resource_path` so the writer can resolve those hrefs.

The sidecar is **stateless**: no cache between calls, no DB, no temporary files beyond those
declared in `work_dir`. Every method must be repeatable with no side effects — that is what
makes it safe to restart the sidecar and re-send in-flight requests.

### UI → Tauri

Commands: `project_list`, `project_create`, `project_get`, `project_delete`,
`project_export`, `project_import`,
`endpoint_list`, `endpoint_upsert`, `endpoint_delete`, `endpoint_test`, `endpoint_models`,
`role_binding_list`, `role_binding_set`, `ingest_start`, `translation_start`, `translation_pause`,
`translation_cancel`, `recon_start`, `recon_get`, `recon_confirm`,
`glossary_list`, `glossary_upsert`, `glossary_delete`,
`review_start`, `suggestion_list`, `suggestion_accept`, `suggestion_reject`, `qa_report`,
`job_list`, `chunk_list`, `chunk_get`, `metrics_get`, `sidecar_status`,
`export_build`, `export_preview`, `export_history`, `open_path`.

`glossary_upsert` takes `{req: {id?, project_id, source, target, kind, note?, status?, source_lang?,
 target_lang?, expected_revision?}}` and returns the persisted row; when `id` and
`expected_revision` are both given the write is optimistic: a row changed by another writer in the
meantime is rejected instead of overwritten. `glossary_delete` takes the row id; `glossary_list`
takes `{project_id}`. Candidates proposed by the reconnaissance and the summarizer are approved,
edited or rejected here; the translator prompt only ever sees non-rejected terms, and a proposal
that conflicts with an existing rendering surfaces as `status='conflict'` plus a
`qa_finding(kind='glossary_conflict')`.

`review_start` takes `{req: {project_id, chunk_ids?, chapter_id?, pass?, with_qa?}}`, where `pass`
is `editor` (default `both`, also `proofreader`), and enqueues the matching `edit_chunk` /
`proofread_chunk` / `qa_scan` jobs for the eligible chunks; it returns `{enqueued}`.
`suggestion_list` takes `{project_id, chunk_id?, pass?, status?}` and returns `suggestion` rows;
`suggestion_accept` and `suggestion_reject` take the suggestion id. Accepting applies the proposed
correction to the block translation (origin `editor` or `proofreader`) and recomposes the chunk's
`target_md`, so an export right after a review sees the accepted text. `qa_report` takes
`{project_id, kind?, severity?, chunk_id?}` and returns the `qa_finding` rows.

`recon_start` runs the `book_recon` job (candidate book profile, PLAN.md §9.4); `recon_get`
reads it back together with the confirmed memory values and the glossary; `recon_confirm` writes
the confirmed fields into `project_memory` and the accepted proper nouns into `glossary_term`.
No new event: the job lifecycle is announced on `job://progress`.

`export_build` takes `{req: {project_id, output_format, output_path?, template?, css?, toc?,
chapter_id?, force?}}`: with `chapter_id` it builds that chapter standalone, `force` bypasses the
unchanged-build skip. It returns the outcome (including `from_cache`, `changed_units` and
`reused_units`) and emits `export://progress` while it runs. `export_preview` takes
`{req: {project_id, chapter_id?}}` and returns the composed markdown units plus the rendered
`metadata.yaml` without invoking Pandoc. `export_history` takes `{project_id}` and returns the
last ten build records.

`project_export` takes `{project_id, output_path?}` and writes the `.llmtz`
(ZIP with `manifest.json`, `project.sqlite`, `work/`, `output/`, `prompts/`), returning
`{output_path, bytes, files}`; an absent `output_path` uses the project output directory.
`project_import` takes `{archive_path}` and returns the imported `Project`.

Events: `job://progress`, `log://line`, `metrics://tick`, `sidecar://status`,
`sidecar://progress`, `export://progress`.

`job://progress` carries the serialized `job` row at every transition the control plane owns
(enqueue, claim, done, failed, cancelled); views treat it as an invalidation trigger and refetch
through commands. `sidecar://progress` forwards the sidecar's out-of-band `progress`
notifications unchanged. `log://line` is `{ts, level, source, message}` — the control plane does
not stamp a project on it.

---

## Per-language conventions

### Python (`sidecar/`)

- **Python 3.12** (pinned in `.python-version`). Not 3.14: `marker`/`torch` have no wheels.
- Project management: `uv`. `uv sync --all-groups`, `uv run <cmd>`.
- Lint/format: `ruff` with `select = ["ALL"]`, `ignore = ["D", "COM812"]`.
- Types: `pyright` in **strict** mode. No unjustified `Any`, no `# type: ignore` without a
  comment explaining why.
- Tests: `pytest` + `pytest-cov`.
- `from __future__ import annotations` in every module.
- Docstrings only where the *why* is not obvious; no comments that restate the code.
- Layout: `llmtranslator_sidecar` package, tests in `sidecar/tests/`.

### Rust (`crates/app/`)

- `cargo fmt` + `cargo clippy -- -D warnings` clean.
- `sqlx` with **runtime-checked** queries (`sqlx::query`), not the compile-time macros: they
  avoid requiring a live database at build time. Embed migrations with `sqlx::migrate!`.
- Errors: `thiserror` for domain errors, `anyhow` only at the edge of command handlers.
- Async on `tokio`. No `unwrap()`/`expect()` on paths that can fail at runtime (I/O, network,
  parsing): only where the invariant is guaranteed by construction, with a comment.
- Logging with `tracing`, structured, never `println!`.

### TypeScript (`ui/`)

- Strict TypeScript, no `any`.
- No direct network calls: everything goes through `invoke`/`listen` from `@tauri-apps/api`.
- UI copy in **Italian**; code and comments in English.

---

## Product constraints (non-negotiable)

- **No telemetry.** No network calls other than the `llama-server` endpoints configured by the
  user. No analytics, no crash reporter, no remote fonts or CDNs.
- **Fully offline.** The app must work without a connection.
- **No secrets in the database.** API keys live in the system keyring; the DB only holds a
  reference.
- Prompts and Pandoc templates are **user data**: externalised, copied into the project
  snapshot, editable. Never hardcode them.
- Do not introduce dependencies that need a GPU or downloaded models on the default path (the
  `marker` exception is an optional extra and stays out of the bundle).

---

## Definition of done

A unit of work is finished when **all** of these pass:

```sh
make lint      # ruff check + pyright + cargo clippy
make test      # pytest + cargo test
make check     # lint + test + build UI + cargo check
```

No exceptions. Do not leave failing tests, do not leave type errors, do not commit with
`--no-verify`. If an existing test breaks because of your change, fix it in the same commit or
in the one immediately after — never leave it broken.
