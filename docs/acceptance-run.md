# Acceptance run — one real book, end to end

Checklist for [issue #15](https://github.com/Nihmar/LocalLLMTranslator/issues/15): the last
open item of the stabilization epic. Everything else is covered by `make check` and the
offline tests; this run proves the core loop on a **real book with a real `llama-server`**.

> **Feature freeze while #15 is open.** Do not start new features from this document. Only
> record what the run exposes, and file one issue per defect found.

Reference book: `D'un monde à l'autre` (fr → en). Any real novel of a similar size is fine;
record which one you used.

## What "pass" means

| # | Criterion | Pass condition |
|---|---|---|
| C1 | Translation coverage | zero chunks in scope with no usable translation |
| C2 | Job health | zero `failed` jobs for the project at the end |
| C3 | Review inbox | fewer than ~50 pending suggestions, and every one is acceptable/rejectable in under a minute |
| C4 | Export | EPUB (and PDF) opens, chapters/TOC/footnotes/images intact, **no `⟦n⟧` token in the output** |
| C5 | Screen vs database | every counter shown in the UI matches the query in §6 |
| C6 | Resume | a pause/cancel + relaunch + resume loses no structure and does not re-translate finished chunks |
| C7 | Crash recovery | killing the app mid-run and restarting recovers the queue without manual SQL |

Record the result of every criterion in the [results table](#7-results).

---

## 0. Prerequisites and baseline

- [ ] Python 3.12 (`uv`), Rust stable, Node 22+, Pandoc 3.x installed.
- [ ] `make check` is green **on the exact commit under test**.
      ```sh
      git rev-parse HEAD          # record the commit in the results table
      make check
      ```
- [ ] `llama-server` running with a model that fits the machine (see the README for the
      recommended flags). Note the model, context size and `--parallel` value:
      ```sh
      curl -s http://127.0.0.1:8080/health
      curl -s http://127.0.0.1:8080/props | head -c 400
      curl -s http://127.0.0.1:8080/v1/models
      ```
- [ ] The book file is available locally (`libro.epub`, or PDF/Markdown).

## 1. Configure the endpoint and the roles

Desktop app → **Modelli**, or headless `llmtz serve` → **Modelli**:

- [ ] Add the endpoint (`http://127.0.0.1:8080`), run **Verifica**, confirm health/models/`props`.
- [ ] Use **Configurazione rapida** to bind every role to the model (or bind translator,
      editor, proofreader and orchestrator individually).
- [ ] Confirm the resource gauge shows the expected parallelism, or states the reason for
      serial degradation.

## 2. Create and ingest

Desktop app → **Nuovo libro** → "Crea e importa", or:

```sh
# Headless, control plane only (no translation yet):
cargo run -p local-llm-translator-server -- translate libro.epub --to en --from fr \
  --endpoint http://127.0.0.1:8080 --model <model> --no-export
# stdout is the project id; export it for the queries below.
export PROJECT_ID=<the id printed above>
```

- [ ] Ingestion finishes without a `failed` job.
- [ ] **Importa** shows the chapters and the expected number of chunks; warnings (if any) are
      plausible (e.g. missing cover), not structural errors.

## 3. Prepare the book profile

Desktop app → **Prepara**:

- [ ] Run the reconnaissance on the orchestrator role; it returns a candidate profile.
- [ ] Confirm the fields you trust (synopsis, style guide, proper nouns). Accepted proper nouns
      become approved glossary terms; everything else stays a candidate.
- [ ] Choose the **dialogue convention** (`keep` or `quotes`) *before* translating.
- [ ] Skim the glossary: every approved term has a different `source` and `target` (a term where
      they are identical is either a `do_not_translate` entry or a bad proposal).

## 4. Translate

Desktop app → **Traduci** → «Avvia / Riprendi»; follow the chunk table, the chapter outline and
the live log.

- [ ] The run completes with no `failed` chunk.
- [ ] Every chunk ends `done`, or `needs_review` **with** text (never `needs_review` with an empty
      body, never `running`).
- [ ] The chapter previews match the source chapter by chapter.
- [ ] **Resume test (C6):** press «Pausa», close the app, reopen it, press «Avvia / Riprendi».
      Finished chunks are not translated again (the `llm_call` count of §6 stays stable for them).
- [ ] **Crash test (C7):** `kill -9` the app (or `llmtz serve`) mid-run, restart it. The
      interrupted chunk returns to `pending`, the job is retried, and the output is complete at
      the end.
- [ ] If a chunk is `needs_review`, open it: the raw rejected answer is in the review view, and
      the accepted/stored text is the previous good one.

## 5. Review

Desktop app → **Rivedi**:

- [ ] Use «Riprova falliti» only if there were failures; otherwise run the editor and proofreader
      passes from the review page.
- [ ] Work the inbox: critical/major first, then minor. Every item is a real defect; "Rifiuta"
      on a stylistic preference and "Accetta" on a defect.
- [ ] Check **Controlli QA**: `untranslated`, `glossary_mismatch`, `placeholder_broken`,
      `markdown_malformed`, `length_anomaly`, `duplicate`, `empty`, `latin_leftover`,
      `glossary_conflict`. Every open finding is either fixed or explicitly ignored.
- [ ] The **Storico** lists the decisions in the order you made them.

## 6. Export

Desktop app → **Esporta**, or:

```sh
cargo run -p local-llm-translator-server -- export "$PROJECT_ID" --format epub
cargo run -p local-llm-translator-server -- export "$PROJECT_ID" --format pdf
```

- [ ] `export_preview` and the UI agree with what is built; the untranslated-chunk guard does
      **not** appear (if it does, go back to §4).
- [ ] The EPUB opens in a reader: correct title/author/language, table of contents, chapters in
      order, footnotes reachable, images present, no `#`/`*`/`⟦` artefact.
- [ ] The PDF opens and is readable (fonts, chapter divisions).
- [ ] Re-run the export without changes: it reports the build as reused (`from_cache`), and with
      `--force` it rebuilds.

### Verification queries

The database is `<data-dir>/app.sqlite` (default Linux
`~/.local/share/org.localllmtranslator.app`, or `$LLMTZ_DATA_DIR`). Use `sqlite3`, or the Python
one-liner below if `sqlite3` is not installed.

```sh
sqlite3 "$DATA_DIR/app.sqlite" "SELECT ... ;"
# or, no sqlite3 binary:
python3 - "$DATA_DIR/app.sqlite" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
for q in [
    "SELECT COUNT(*) FROM chunk c JOIN document d ON d.id=c.document_id "
    "WHERE d.project_id='REPLACE_ME' AND (c.target_md IS NULL OR trim(c.target_md)='')",
]:
    print(q, "=>", db.execute(q).fetchone()[0])
PY
```

Run these with `$PROJECT_ID` substituted:

- [ ] **C1 — untranslated chunks in scope** (must be `0`):
      ```sql
      SELECT COUNT(*) FROM chunk c
        JOIN document d ON d.id = c.document_id
       WHERE d.project_id = '<id>' AND (c.target_md IS NULL OR trim(c.target_md) = '');
      ```
- [ ] **C2 — failed jobs** (must be `0`):
      ```sql
      SELECT COUNT(*) FROM job WHERE project_id = '<id>' AND state = 'failed';
      ```
- [ ] **C4 — placeholder leakage** (must be `0`):
      ```sql
      SELECT COUNT(*) FROM chunk c
        JOIN document d ON d.id = c.document_id
       WHERE d.project_id = '<id>' AND c.target_md LIKE '%⟦%';
      ```
- [ ] **C3 — pending review inbox** (target `< ~50`):
      ```sql
      SELECT s.severity, COUNT(*)
        FROM suggestion s
        JOIN chunk c ON c.id = s.chunk_id
        JOIN document d ON d.id = c.document_id
       WHERE d.project_id = '<id>' AND s.status = 'pending'
       GROUP BY s.severity;
      ```
- [ ] **C5 — chunk status counters** (compare with the UI):
      ```sql
      SELECT c.status, COUNT(*)
        FROM chunk c JOIN document d ON d.id = c.document_id
       WHERE d.project_id = '<id>' GROUP BY c.status;
      ```
- [ ] **Open QA findings** (compare with "Controlli QA"):
      ```sql
      SELECT kind, severity, COUNT(*)
        FROM qa_finding
       WHERE project_id = '<id>' AND status = 'open'
       GROUP BY kind, severity;
      ```
- [ ] **C6 — idempotent resume** (run before/after a resume; `llm_call` must not grow for
      chunks that were already `done`):
      ```sql
      SELECT COUNT(*) FROM llm_call
       WHERE chunk_id IN (SELECT c.id FROM chunk c JOIN document d ON d.id = c.document_id
                           WHERE d.project_id = '<id>');
      ```
- [ ] **Media and chapters in the EPUB** (optional):
      ```sh
      unzip -l output/libro.epub | head -40
      ```
      and open the file in a reader.

## 7. Results

| Field | Value |
|---|---|
| Commit under test | |
| Book | |
| Source → target | fr → en |
| Model / endpoint / `n_ctx` / `--parallel` | |
| Chunks total | |
| C1 untranslated in scope | |
| C2 failed jobs | |
| C3 pending suggestions (critical/major/minor) | |
| C4 placeholder tokens in output | |
| C5 counters vs UI | |
| C6 resume | |
| C7 crash recovery | |
| Time from import to EPUB | |
| Diagnostics bundle attached (if a failure) | |

- [ ] **Pass:** all of C1–C7 hold.
- [ ] **Fail:** file one issue per defect, with the query output, the chunk/job id and — when the
      failure is a crash or a wedge — the diagnostics bundle (`diagnostics_export`, or
      `<data-dir>/diagnostics/`). Quote the book only inside the issue if it is unavoidable;
      prefer the job/chunk ids.

## Notes for the operator

- The desktop app and `llmtz serve` share the same data directory: you can start a run in the
  browser and resume it in the desktop app, but do not run both against the same data directory
  at the same time.
- `LLMTRANSLATOR_SIDECAR` / `LLMTRANSLATOR_PANDOC` point at the sidecar and the pandoc binary; a
  missing sidecar shows up as a `sidecar://status` failure, not as a silent hang.
- Logs: `<data-dir>/logs/llmtz.<date>.log`. They must not contain book text; if they do, that is
  a defect ([#27](https://github.com/Nihmar/LocalLLMTranslator/issues/27)).
- When a chunk is slow, check `/props` and `/metrics` before blaming the app: a serial endpoint
  (one slot, low VRAM) is the configured behaviour, not a bug.
