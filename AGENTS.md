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

## IPC contract (frozen)

The sidecar JSON-RPC methods and the Tauri commands and events are specified in **`PLAN.md`
§12**, their single source: change them only together with that section. The rules that bite
most often:

- the sidecar is **stateless**: no database, no cache between calls, no files outside the
  declared `work_dir`; every method is repeatable, which is what makes restarting it safe;
- every Tauri command is declared `#[tauri::command(rename_all = "snake_case")]`: argument keys
  are the Rust parameter names (`{ project_id }`, `{ req: {...} }`), and
  `ui/src/lib/ipc.test.ts` rejects a camelCase key;
- the UI never talks to the network: everything goes through `invoke`/`listen` in
  `ui/src/lib/ipc.ts` and `ui/src/lib/events.ts`.

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
make check     # lint + typecheck + test (python, rust, ui) + build UI + cargo check
```

No exceptions. Do not leave failing tests, do not leave type errors, do not commit with
`--no-verify`. If an existing test breaks because of your change, fix it in the same commit or
in the one immediately after — never leave it broken.
